// Native x0x direct-message adapter for managed-agent observer telemetry +
// control. Replaces the kind:24200 relay subscribe/publish/decrypt path.
//
// Producer side (owner desktop): subscribe to the owner daemon's `/ws/direct`
// (via `subscribeX0xLive`) + cold-load `dm:<child>` history, filter observer
// envelopes, validate owner/child identity, and yield ObserverEvent-shaped
// frames for the store. Control side: send a control envelope to a child via
// `x0xSendDirectMessage`.
//
// The DM plane is shared with chat; this adapter ingests ONLY observer
// envelopes (`kind === "observer"`) whose `owner` matches the local owner
// AgentId and whose `agent` is a known managed child — defense-in-depth on top
// of the daemon's authenticated transport. Chat frames never reach the store.

import {
  OBSERVER_CONTENT_TYPE,
  OBSERVER_KIND,
  type ObserverEnvelope,
  type ObserverFrame,
  parseObserverEnvelope,
} from "@/shared/api/observerEnvelope";
import { z } from "zod";
import {
  x0xHistoryList,
  x0xSendDirectMessage,
  subscribeX0xLive,
  type X0xHistoryRow,
  type X0xLiveDirectMessage,
  type X0xLiveFrame,
  type X0xLiveSubscription,
  type X0xScope,
} from "@/shared/api/tauriNativeX0x";

// ── ObserverEvent boundary schema ───────────────────────────────────────────
//
// The envelope `data` field is an ObserverEvent. Validated here with Zod
// (external, wire-shaped input) rather than an inline cast, per the
// `ts-no-inline-cast-access` rule. Mirrors the TS `ObserverEvent` type in
// `features/agents/ui/agentSessionTypes.ts`.

export const ObserverEventSchema = z.object({
  seq: z.number().int(),
  timestamp: z.string(),
  kind: z.string(),
  agentIndex: z.number().int().nullable(),
  channelId: z.string().nullable(),
  sessionId: z.string().nullable(),
  turnId: z.string().nullable(),
  startedAt: z.string().nullable().optional(),
  payload: z.unknown(),
});
export type NativeObserverEvent = z.infer<typeof ObserverEventSchema>;

/** A decoded observer frame ready for the store. */
export type NativeObserverFrame = {
  /** The emitting child AgentId (64-hex). The store maps this to its agent. */
  agent: string;
  /** Transport frame direction. */
  frame: ObserverFrame;
  /** The validated ObserverEvent payload. */
  observerEvent: NativeObserverEvent;
  /** Canonical durable msg_id (BLAKE3 hex) for cold↔live dedupe, if known. */
  msgId: string | null;
};

/** Decode a base64 payload string to UTF-8 text, or null on failure. */
function decodeBase64Utf8(b64: string): string | null {
  try {
    // Node/web base64 decode → bytes → UTF-8.
    const bytes = Uint8Array.from(atob(b64), (c) => c.charCodeAt(0));
    return new TextDecoder().decode(bytes);
  } catch {
    return null;
  }
}

/**
 * Validate an envelope for ingestion: must be an observer envelope addressed to
 * `ownerAgentId` and emitted by a `knownChildAgentIds` member. Returns the
 * narrowed event + frame, or null (drop, fail-closed).
 */
function admitEnvelope(
  envelope: ObserverEnvelope,
  ownerAgentId: string,
  knownChildAgentIds: Set<string>,
): { observerEvent: NativeObserverEvent; frame: ObserverFrame } | null {
  // Owner-auth (defense-in-depth): the envelope must be addressed to us.
  if (envelope.owner !== ownerAgentId) return null;
  // Child-auth: the emitter must be a managed agent we track.
  if (!knownChildAgentIds.has(envelope.agent)) return null;
  const parsed = ObserverEventSchema.safeParse(envelope.data);
  if (!parsed.success) return null;
  return { observerEvent: parsed.data, frame: envelope.frame };
}

/**
 * Decode a live `/ws/direct` frame into a NativeObserverFrame, or null if it is
 * not an observer telemetry/control_result frame from a known child addressed
 * to `ownerAgentId`. Chat and non-observer DMs return null (never reach store).
 */
export function decodeObserverFromLive(
  frame: X0xLiveFrame,
  ownerAgentId: string,
  knownChildAgentIds: Set<string>,
): NativeObserverFrame | null {
  if (frame.type !== "direct_message") return null;
  const dm: X0xLiveDirectMessage = frame;
  const text = decodeBase64Utf8(dm.payload);
  if (text === null) return null;
  const envelope = parseObserverEnvelope(text);
  if (envelope === null) return null;
  // Live frames carry no content_type; filter by envelope kind only. Only
  // ingest telemetry + control_result (control is owner→child, never inbound
  // here — but be explicit).
  if (envelope.frame !== "telemetry" && envelope.frame !== "control_result") {
    return null;
  }
  const admitted = admitEnvelope(envelope, ownerAgentId, knownChildAgentIds);
  if (admitted === null) return null;
  return {
    agent: envelope.agent,
    frame: admitted.frame,
    observerEvent: admitted.observerEvent,
    msgId: dm.msgId ?? null,
  };
}

/**
 * Decode a cold-loaded history row into a NativeObserverFrame, or null. Uses
 * the row's `contentType` for a cheap reject before parsing the payload.
 */
export function decodeObserverFromHistory(
  row: X0xHistoryRow,
  ownerAgentId: string,
  knownChildAgentIds: Set<string>,
): NativeObserverFrame | null {
  // Cheap reject: only observer-content rows can be observer frames.
  if (row.contentType !== OBSERVER_CONTENT_TYPE) {
    // Still allow a payload-shape probe for forward-compat (a daemon that did
    // not stamp content_type but recorded the envelope as durable).
    const probe = decodeBase64Utf8(row.payload);
    if (probe === null) return null;
    const env = parseObserverEnvelope(probe);
    if (env === null || env.kind !== OBSERVER_KIND) return null;
    const admitted = admitEnvelope(env, ownerAgentId, knownChildAgentIds);
    if (admitted === null) return null;
    return {
      agent: env.agent,
      frame: admitted.frame,
      observerEvent: admitted.observerEvent,
      msgId: row.msgId,
    };
  }
  const text = decodeBase64Utf8(row.payload);
  if (text === null) return null;
  const envelope = parseObserverEnvelope(text);
  if (envelope === null) return null;
  if (envelope.frame !== "telemetry" && envelope.frame !== "control_result") {
    return null;
  }
  const admitted = admitEnvelope(envelope, ownerAgentId, knownChildAgentIds);
  if (admitted === null) return null;
  return {
    agent: envelope.agent,
    frame: admitted.frame,
    observerEvent: admitted.observerEvent,
    msgId: row.msgId,
  };
}

// ── Live subscribe ──────────────────────────────────────────────────────────

/**
 * Subscribe to live observer frames from the owner daemon's `/ws/direct`. The
 * daemon delivers every inbound DM to the session; this filters to observer
 * envelopes from `knownChildAgentIds` addressed to `ownerAgentId`.
 *
 * `dmScopePeer` is the AgentId used to form the `dm:<peer>` scope that selects
 * the `/ws/direct` plan — peer filtering is performed here, not server-side.
 * Pass the owner's own AgentId (the receiver of child telemetry).
 */
export async function subscribeObserverLive(input: {
  ownerAgentId: string;
  knownChildAgentIds: Set<string>;
  /** AgentId forming the dm scope (selects /ws/direct). Defaults to owner. */
  dmScopePeer?: string;
  /** Optional backfill window (stored-DM replay, oldest→newest) before live. */
  backfillLimit?: number;
  onFrame: (frame: NativeObserverFrame | null, raw: X0xLiveFrame) => void;
}): Promise<X0xLiveSubscription> {
  const peer = input.dmScopePeer ?? input.ownerAgentId;
  const scope = `dm:${peer}` as X0xScope;
  return subscribeX0xLive(
    {
      scope,
      backfill: input.backfillLimit
        ? { limit: input.backfillLimit }
        : undefined,
    },
    (raw) => {
      const decoded = decodeObserverFromLive(
        raw,
        input.ownerAgentId,
        input.knownChildAgentIds,
      );
      input.onFrame(decoded, raw);
    },
  );
}

// ── Cold-load (replay) ──────────────────────────────────────────────────────

/**
 * One page of a child's observer history, returned oldest-first with the
 * keyset cursor the store needs to advance its scan.
 */
export type ColdLoadObserverHistoryPage = {
  /** Decoded observer frames, oldest-first (replay ordering). */
  frames: NativeObserverFrame[];
  /**
   * Keyset cursor for the next (older) page (`nextCursor.beforeId`).
   * `undefined` when the page was empty or the daemon signalled no further
   * rows — the child's history is exhausted for this scan.
   */
  nextBeforeId: number | undefined;
  /** Whether the daemon signalled more rows exist beyond this page. */
  hasMore: boolean;
};

/**
 * The single cold-replay primitive: cold-load one page of a child's observer
 * history (`dm:<childAgentId>` scope) and return decoded frames oldest-first
 * plus the keyset cursor for paging. Non-observer rows are filtered out.
 *
 * The store drives the per-child cursor (eager hydration + load-older-on-scroll);
 * this does the authenticated fetch + owner/child-auth-validating decode, so
 * there is exactly one observer-history decode boundary.
 */
export async function coldLoadObserverHistory(input: {
  childAgentId: string;
  ownerAgentId: string;
  knownChildAgentIds: Set<string>;
  limit?: number;
  beforeId?: number;
}): Promise<ColdLoadObserverHistoryPage> {
  const scope = `dm:${input.childAgentId}` as X0xScope;
  const page = await x0xHistoryList({
    scope,
    limit: input.limit ?? 200,
    beforeId: input.beforeId,
  });
  // History is newest-first; reverse for oldest-first replay ordering.
  const frames = page.rows
    .map((row) =>
      decodeObserverFromHistory(
        row,
        input.ownerAgentId,
        input.knownChildAgentIds,
      ),
    )
    .filter((f): f is NativeObserverFrame => f !== null)
    .reverse();
  return {
    frames,
    nextBeforeId: page.nextCursor?.beforeId,
    hasMore: page.hasMore,
  };
}

// ── Control producer (owner → child) ────────────────────────────────────────

/**
 * Send a control command to a managed child agent as an observer envelope over
 * x0x direct messaging. `controlPayload` is the existing control shape
 * (`{ type: "cancel_turn" | "switch_model", channelId, modelId? }`).
 *
 * Fire-and-forget on the send side: the outcome arrives asynchronously as a
 * `control_result` observer frame (consumed by `subscribeObserverLive`).
 */
export async function sendObserverControl(input: {
  childAgentId: string;
  ownerAgentId: string;
  controlPayload: unknown;
  session?: string | null;
  channelId?: string | null;
}): Promise<void> {
  const envelope: ObserverEnvelope = {
    v: 1,
    kind: OBSERVER_KIND,
    frame: "control",
    agent: input.childAgentId,
    owner: input.ownerAgentId,
    session: input.session ?? null,
    seq: 0,
    ts: null,
    channel: input.channelId ?? null,
    data: input.controlPayload,
  };
  const bytes = new TextEncoder().encode(JSON.stringify(envelope));
  await x0xSendDirectMessage({
    agentId: input.childAgentId,
    payload: bytes,
  });
}
