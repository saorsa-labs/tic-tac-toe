import {
  MSG_PREFIX,
  THREAD_PREFIX,
} from "@/features/channels/readState/readStateFormat";
import {
  readStoredReadState,
  writeStoredReadState,
} from "@/features/channels/readState/readStateStorage";
import { setLocalStorageItemWithRecovery } from "@/shared/lib/localStorageQuota";
import { truncatePubkey } from "@/shared/lib/pubkey";

const CLIENT_ID_KEY_PREFIX = "buzz.nip-rs.client-id";
const SLOT_ID_KEY_PREFIX = "buzz.nip-rs.slot-id";

function generateHex(bytes: number): string {
  const arr = new Uint8Array(bytes);
  crypto.getRandomValues(arr);
  return Array.from(arr, (b) => b.toString(16).padStart(2, "0")).join("");
}

function getOrCreatePersisted(key: string, generator: () => string): string {
  let value = localStorage.getItem(key);
  if (!value) {
    value = generator();
    setLocalStorageItemWithRecovery(key, value);
  }
  return value;
}

function clientIdKey(pubkey: string): string {
  return `${CLIENT_ID_KEY_PREFIX}:${pubkey}`;
}

function slotIdKey(pubkey: string): string {
  return `${SLOT_ID_KEY_PREFIX}:${pubkey}`;
}

export type ApplyRemoteContextResult = "unchanged" | "advanced";

export type ContextParentResolver = (contextId: string) => string | null;

/**
 * NIP-RS Hierarchical Frontier Rule (NIP-RS.md:141-167):
 * `effective(ctx) = max(merged[ctx], effective(parent(ctx)))`.
 *
 * The thread→channel relationship is NOT serialized into the blob
 * (NIP-RS.md:136-139); it is derived from the event graph at evaluation time
 * via `parentResolver`. When the resolver yields no parent (channels, or an
 * unresolvable thread root), the frontier degrades to the context's own merged
 * value alone (NIP-RS.md:165-167). Returns null when the context has never been
 * read and no parent term covers it.
 */
export function resolveEffectiveTimestamp(args: {
  effectiveState: Map<string, number>;
  contextId: string;
  parentResolver: ContextParentResolver | null;
}): number | null {
  const { effectiveState, contextId, parentResolver } = args;
  const own = effectiveState.get(contextId) ?? null;

  const parentId = parentResolver?.(contextId) ?? null;
  if (parentId === null) return own;

  const parent = effectiveState.get(parentId) ?? null;
  if (parent === null) return own;
  if (own === null) return parent;
  return Math.max(own, parent);
}

function resolveRemoteContextTimestamp(args: {
  current: number;
  timestamp: number;
}): { next: number; result: ApplyRemoteContextResult } {
  const next = Math.max(args.current, args.timestamp);
  return {
    next,
    result: next === args.current ? "unchanged" : "advanced",
  };
}

export function applyRemoteContextTimestamp(args: {
  effectiveState: Map<string, number>;
  contextSourceCreatedAt: Map<string, number>;
  contextId: string;
  timestamp: number;
  eventCreatedAt: number;
}): ApplyRemoteContextResult {
  const {
    effectiveState,
    contextSourceCreatedAt,
    contextId,
    timestamp,
    eventCreatedAt,
  } = args;
  const sourceCreatedAt = contextSourceCreatedAt.get(contextId) ?? 0;
  const current = effectiveState.get(contextId) ?? 0;
  const { next, result } = resolveRemoteContextTimestamp({
    current,
    timestamp,
  });

  if (result === "advanced") {
    effectiveState.set(contextId, next);
  }
  if (eventCreatedAt > sourceCreatedAt) {
    contextSourceCreatedAt.set(contextId, eventCreatedAt);
  }
  return result;
}

/**
 * Result of a `splitContextsIntoBudgetedSlots` call.
 */
export interface SlotSplitResult {
  /** Contexts record for each slot (primary slot first). */
  slots: Array<Record<string, number>>;
  /**
   * Extra slot IDs allocated beyond the first. Length is `slots.length - 1`.
   * The caller is responsible for persisting these.
   */
  extraSlotIds: string[];
}

/**
 * Partition `channelEntries` across slots so each slot's blob fits within
 * `maxBytes`. Thread/msg entries are added to the primary slot (index 0) and
 * trimmed to budget.
 *
 * `initialSlotCount` is the number of slots already available (≥ 1). If the
 * initial distribution doesn't fit, new slot IDs are generated via
 * `slotIdGenerator` until everything fits or `maxSlots` is reached.
 *
 * Returns `{ slots, extraSlotIds }` on success, or `null` when even `maxSlots`
 * slots can't accommodate all channel keys.
 *
 * Exported for unit testing; callers should prefer `splitContextsIntoSlots()`.
 */
export function splitContextsIntoBudgetedSlots(args: {
  channelEntries: [string, number][];
  threadMsgEntries: [string, number][];
  clientId: string;
  initialSlotCount: number;
  maxSlots: number;
  maxBytes: number;
  slotIdGenerator: () => string;
}): SlotSplitResult | null {
  const {
    channelEntries,
    threadMsgEntries,
    clientId,
    initialSlotCount,
    maxSlots,
    maxBytes,
    slotIdGenerator,
  } = args;

  const encoder = new TextEncoder();
  const blobFor = (c: Record<string, number>) =>
    JSON.stringify({ v: 1, client_id: clientId, contexts: c });

  let slotCount = initialSlotCount;
  const extraSlotIds: string[] = [];

  // Distribute channel keys and check fit. Grow slot count until all fit.
  const distribute = (count: number): Array<Record<string, number>> => {
    const slotContexts: Array<Record<string, number>> = Array.from(
      { length: count },
      () => ({}),
    );
    for (let i = 0; i < channelEntries.length; i++) {
      const [key, ts] = channelEntries[i];
      slotContexts[i % count][key] = ts;
    }
    return slotContexts;
  };

  let slotContexts = distribute(slotCount);
  while (
    slotContexts.some((c) => encoder.encode(blobFor(c)).length > maxBytes) &&
    slotCount < maxSlots
  ) {
    extraSlotIds.push(slotIdGenerator());
    slotCount++;
    slotContexts = distribute(slotCount);
  }

  if (slotContexts.some((c) => encoder.encode(blobFor(c)).length > maxBytes)) {
    return null;
  }

  // Add thread/msg entries to the primary slot and trim to budget.
  for (const [key, ts] of threadMsgEntries) {
    slotContexts[0][key] = ts;
  }
  trimContextsToBudget(slotContexts[0], clientId, maxBytes);

  return { slots: slotContexts, extraSlotIds };
}

/**
 * Result of a `trimContextsToBudget` call.
 */
export interface TrimResult {
  /** Number of entries removed from `contexts`. */
  evicted: number;
  /** True when the serialized blob fits within `maxBytes` after trimming. */
  fitsAfterTrim: boolean;
}

/**
 * Trim a contexts map to fit within `maxBytes` when serialized as the JSON
 * blob `{v:1, client_id, contexts}`. Evicts oldest `msg:` entries first
 * (lowest timestamp), then oldest `thread:` entries. Channel keys are never
 * evicted. Mutates `contexts` in place.
 *
 * Returns `{ evicted, fitsAfterTrim }`. `fitsAfterTrim` is false when the
 * remaining blob (channel keys only) still exceeds `maxBytes` — the caller
 * must not publish in that case.
 *
 * Exported for unit testing; callers should prefer `currentContexts()`.
 */
export function trimContextsToBudget(
  contexts: Record<string, number>,
  clientId: string,
  maxBytes: number,
): TrimResult {
  const encoder = new TextEncoder();
  const blobFor = (c: Record<string, number>) =>
    JSON.stringify({ v: 1, client_id: clientId, contexts: c });

  let currentBytes = encoder.encode(blobFor(contexts)).length;
  if (currentBytes <= maxBytes) {
    return { evicted: 0, fitsAfterTrim: true };
  }

  const msgEntries: [string, number][] = [];
  const threadEntries: [string, number][] = [];
  for (const [key, ts] of Object.entries(contexts)) {
    if (key.startsWith(MSG_PREFIX)) {
      msgEntries.push([key, ts]);
    } else if (key.startsWith(THREAD_PREFIX)) {
      threadEntries.push([key, ts]);
    }
  }
  // Oldest-first within each tier.
  msgEntries.sort((a, b) => a[1] - b[1]);
  threadEntries.sort((a, b) => a[1] - b[1]);

  // O(n) pass: subtract each entry's byte contribution from currentBytes and
  // collect entries to evict. The per-entry estimate is `,"key":timestamp`
  // (key.length + 3 bytes for `"`, `"`, `:` plus 1 comma) + timestamp digits.
  // This is an approximation — the final encode below is the authoritative check.
  const toEvict: string[] = [];
  for (const [key, ts] of [...msgEntries, ...threadEntries]) {
    if (currentBytes <= maxBytes) break;
    // Contribution: `,"key":timestamp` — comma + quoted key + colon + value
    currentBytes -= key.length + 3 + String(ts).length + 1;
    toEvict.push(key);
  }

  for (const key of toEvict) {
    delete contexts[key];
  }

  // Final authoritative check — handles JSON comma-accounting edge cases
  // (e.g. last-entry comma disappears) that the per-entry estimate ignores.
  const fitsAfterTrim = encoder.encode(blobFor(contexts)).length <= maxBytes;
  return { evicted: toEvict.length, fitsAfterTrim };
}

// writeStoredReadState persists relay-sync artifacts (publishableContextIds
// and contextSourceCreatedAt) alongside the read markers. In local-only mode
// neither has meaning — the manager never tracks relay state — so empty
// collections are passed to honor the storage contract without reviving
// relay-only concepts. Shared frozen singletons avoid per-persist allocation.
const EMPTY_STRING_SET: ReadonlySet<string> = new Set();
const EMPTY_NUMBER_MAP: ReadonlyMap<string, number> = new Map();

export class ReadStateManager {
  private pubkey: string;
  private clientId: string;
  private slotId: string;
  private effectiveState = new Map<string, number>();
  private listeners = new Set<() => void>();
  private initialized = false;
  private destroyed = false;
  private parentResolver: ContextParentResolver | null = null;

  constructor(pubkey: string) {
    this.pubkey = pubkey;
    this.clientId = getOrCreatePersisted(clientIdKey(pubkey), () =>
      crypto.randomUUID(),
    );
    this.slotId = getOrCreatePersisted(slotIdKey(pubkey), () =>
      generateHex(16),
    );
  }

  async initialize(): Promise<void> {
    if (this.initialized || this.destroyed) return;
    console.debug(
      `[ReadStateManager] initialize pubkey=${truncatePubkey(this.pubkey)} clientId=${this.clientId.substring(0, 8)}… slotId=${this.slotId}`,
    );

    this.hydrateFromLocalStorage();

    this.initialized = true;
    console.debug(
      `[ReadStateManager] initialize complete contexts=${this.effectiveState.size}`,
    );
    this.notifyListeners();
  }

  markContextRead(contextId: string, unixTimestamp: number): void {
    this.advanceContext(contextId, unixTimestamp);
  }

  seedContextRead(contextId: string, unixTimestamp: number): void {
    this.advanceContext(contextId, unixTimestamp);
  }

  private advanceContext(contextId: string, unixTimestamp: number): void {
    const current = this.effectiveState.get(contextId) ?? 0;
    if (unixTimestamp <= current) {
      return;
    }

    this.effectiveState.set(contextId, unixTimestamp);
    this.persistLocalState();
    this.notifyListeners();
  }

  getEffectiveTimestamp(contextId: string): number | null {
    return resolveEffectiveTimestamp({
      effectiveState: this.effectiveState,
      contextId,
      parentResolver: this.parentResolver,
    });
  }

  /**
   * The context's OWN merged read marker, WITHOUT the hierarchical parent term.
   * Callers that evaluate a `thread:<root>` context outside the active channel
   * (e.g. the sidebar unread scan over background channels) must use this:
   * getEffectiveTimestamp folds in parentResolver, which is installed by the
   * active ChannelScreen and maps every thread to the *active* channel — using
   * it for a background channel's thread would borrow the wrong channel marker.
   */
  getOwnTimestamp(contextId: string): number | null {
    return this.effectiveState.get(contextId) ?? null;
  }

  /**
   * Inject the thread→channel parent resolver derived from the React event
   * graph (NIP-RS.md:136-139). The hierarchical max in getEffectiveTimestamp
   * is a no-op until this is set.
   */
  setContextParentResolver(resolver: ContextParentResolver | null): void {
    this.parentResolver = resolver;
  }

  subscribe(listener: () => void): () => void {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  }

  destroy(): void {
    this.destroyed = true;
    this.listeners.clear();
  }

  private hydrateFromLocalStorage(): void {
    const stored = readStoredReadState(this.pubkey);
    for (const [contextId, timestamp] of stored.contexts) {
      this.effectiveState.set(contextId, timestamp);
    }
    this.persistLocalState();
  }

  private persistLocalState(): void {
    writeStoredReadState(
      this.pubkey,
      this.effectiveState,
      EMPTY_STRING_SET,
      EMPTY_NUMBER_MAP,
    );
  }

  /**
   * Returns contexts advanced by a remote read-state sync since the last
   * drain. Local-only mode has no remote sync, so this is always empty; kept
   * as a stable no-op for the public API (useReadState/useUnreadChannels still
   * call it on every read-state invalidation).
   */
  drainSyncedAdvances(): ReadonlySet<string> {
    return EMPTY_STRING_SET;
  }

  private notifyListeners(): void {
    for (const listener of this.listeners) {
      try {
        listener();
      } catch (error) {
        console.debug("[ReadStateManager] listener threw:", error);
        // Don't let a broken listener break the manager
      }
    }
  }
}
