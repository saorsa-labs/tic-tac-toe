import { useSyncExternalStore } from "react";
import { useQueries } from "@tanstack/react-query";

import type { Channel } from "@/shared/api/types";
import { x0xHistoryList } from "@/shared/api/tauriNativeX0x";
import { resolveNativeHistoryScope } from "@/features/messages/lib/nativeMessaging";
import { subscribeHistoryScope } from "@/features/messages/lib/nativeHistoryScopeStore";

/**
 * Root-existence status for a thread-draft's parent message.
 *
 * - `checking`  — query in flight. Treated as available/counted (optimistic)
 *                 so a slow daemon does not blink drafts out of the badge.
 * - `available` — the root resolves: confirmed present in the bounded native
 *                 history window, or optimistic when undecidable (older rows
 *                 may exist beyond the window).
 * - `deleted`   — the bounded window EXHAUSTED the scope's history and the root
 *                 was absent, so the daemon has no such message. Never emitted
 *                 as a false positive: when older rows may still exist beyond
 *                 the window, the root stays optimistic instead. (`classifyError`
 *                 also maps the legacy relay "event not found" string.)
 * - `error`     — a transport/daemon rejection. Treated as available/counted;
 *                 NOT labeled deleted. Re-checked on next panel open.
 */
export type RootStatus = "checking" | "available" | "deleted" | "error";

/**
 * A thread-draft root paired with the channel used to resolve its native
 * history scope. The channel may be `null` when the draft's channel is no
 * longer cached — the root then resolves optimistically.
 */
export type DraftRootRef = {
  rootId: string;
  channel: Channel | null;
};

/**
 * Bounded recent-history window probed per draft root. Intentionally small:
 * this is an id-match existence check, not a full-history payload scan. A
 * definitive `deleted` is only emitted when this window exhausts the scope.
 */
const DRAFT_ROOT_LOOKUP_LIMIT = 100;

/**
 * Resolve one draft root's status against native durable history.
 *
 * This REPLACES the legacy relay `get_event` lookup, which misfired for native
 * messages (their identity is a BLAKE3 `msgId`, not a Nostr event id) and
 * produced false `deleted` flags.
 *
 * Behaviour:
 * - No resolvable scope (unknown channel / ambiguous DM) → `available`
 *   (optimistic; we cannot check, so we never block the draft).
 * - Root present in the bounded window → `available` (confirmed).
 * - Root absent AND the window exhausted the scope (`hasMore === false`) →
 *   `deleted` (the daemon holds no such message in this scope).
 * - Root absent but older rows may exist beyond the window → `available`
 *   (optimistic). A native get-by-msgId endpoint would make this authoritative
 *   without paging; it is the remaining daemon API blocker.
 * - Transport/daemon rejection → throws; the caller classifies it via
 *   `classifyError` (→ `error`, never `deleted` for native failures).
 */
export async function resolveNativeRootStatus(
  rootId: string,
  channel: Channel | null,
): Promise<RootStatus> {
  if (!channel) {
    return "available";
  }
  // Group durable history requires the daemon-resolved stable scope. When it is
  // not yet known the root cannot be verified, so fail closed — NEVER return
  // optimistic "available", which would enable a reply against unverified
  // history. The hook keeps an unresolved group in the pending (`checking`)
  // state and re-evaluates on scope arrival; a direct caller gets an honest
  // error. An unresolvable DM (ambiguous peer) has no deterministic scope and
  // keeps its sanctioned optimistic-available treatment.
  const scope = resolveNativeHistoryScope(channel);
  if (scope === null) {
    if (channel.channelType !== "dm") {
      throw new Error(
        `Cannot verify thread-draft root ${rootId}: durable-history scope for group ${channel.id} is not resolved.`,
      );
    }
    return "available";
  }
  const page = await x0xHistoryList({ scope, limit: DRAFT_ROOT_LOOKUP_LIMIT });
  const present = page.rows.some(
    (row) => row.msgId.toLowerCase() === rootId.toLowerCase(),
  );
  if (present) {
    return "available";
  }
  // Absent from the bounded window. Only call it `deleted` when that window
  // exhausted the scope — otherwise older rows beyond the window may still
  // hold the root, and a false `deleted` is exactly the relay-path bug.
  return page.hasMore ? "available" : "deleted";
}

const EVENT_NOT_FOUND_MESSAGE = "event not found";

/**
 * Classify a lookup rejection into a `RootStatus`. Only the legacy relay
 * "event not found" string maps to `deleted`; every native transport/daemon
 * failure maps to `error` (treated as optimistic-available by the badge).
 */
export function classifyError(err: unknown): RootStatus {
  if (typeof err === "string" && err.includes(EVENT_NOT_FOUND_MESSAGE)) {
    return "deleted";
  }
  if (err instanceof Error && err.message.includes(EVENT_NOT_FOUND_MESSAGE)) {
    return "deleted";
  }
  return "error";
}

/**
 * Resolves root-existence status for a set of thread-draft roots via native
 * durable history.
 *
 * **Query semantics:**
 * - `staleTime: 0` — every query is always considered stale.
 * - `refetchOnMount: "always"` — re-fetch every time this hook mounts (i.e.
 *   every time the Drafts panel opens).
 * - Query key = `["draftRootStatus", rootId]` — root id only, no timestamp.
 *   React Query dedupes concurrent lookups within one open.
 * - Enabled only when `isOpen` is true so we don't burn daemon RTTs when the
 *   Drafts panel is closed.
 *
 * @param refs    Deduplicated thread-root refs (root id + resolving channel).
 * @param isOpen  Whether the Drafts panel is currently visible.
 * @returns       A `Map<rootId, RootStatus>` — one entry per input root id.
 */
export function useDraftRootStatus(
  refs: DraftRootRef[],
  isOpen: boolean,
): Map<string, RootStatus> {
  // Dedupe by root id; first channel wins (a root belongs to one scope).
  const byRootId = new Map<string, Channel | null>();
  for (const ref of refs) {
    if (ref.rootId.length > 0 && !byRootId.has(ref.rootId)) {
      byRootId.set(ref.rootId, ref.channel);
    }
  }
  const rootIds = [...byRootId.keys()];

  // A group's durable-history scope resolves asynchronously via the live
  // subscriptions. Subscribe to the scope registry so this hook re-renders the
  // instant any relevant group's stable historyScope arrives, and fold the
  // resolved-scopes token into each query key so a held root re-evaluates on
  // scope arrival rather than staying stuck. (Primitive string snapshot; value-
  // stable under useSyncExternalStore.)
  const getScopesKey = () =>
    rootIds
      .map((rootId) => {
        const channel = byRootId.get(rootId);
        return channel
          ? `${rootId}=${resolveNativeHistoryScope(channel) ?? ""}`
          : `${rootId}=`;
      })
      .join("|");
  const scopesKey = useSyncExternalStore(
    subscribeHistoryScope,
    getScopesKey,
    getScopesKey,
  );

  const results = useQueries({
    queries: rootIds.map((rootId) => {
      const channel = byRootId.get(rootId) ?? null;
      // Hold a draft root whose GROUP scope is not yet resolved in the pending
      // (`checking`) state — never optimistic "available" — so a reply cannot
      // be enabled against unverified history; it re-evaluates on scope arrival
      // via `scopesKey`. DMs are deterministic; a missing channel keeps the
      // sanctioned optimistic path.
      const awaitingGroupScope =
        channel !== null &&
        channel.channelType !== "dm" &&
        resolveNativeHistoryScope(channel) === null;
      return {
        queryKey: ["draftRootStatus", rootId, scopesKey] as const,
        queryFn: () => resolveNativeRootStatus(rootId, channel),
        staleTime: 0,
        refetchOnMount: "always" as const,
        retry: false,
        enabled: isOpen && rootId.length > 0 && !awaitingGroupScope,
      };
    }),
  });

  const statusMap = new Map<string, RootStatus>();
  rootIds.forEach((rootId, index) => {
    const result = results[index];
    if (!result) {
      statusMap.set(rootId, "checking");
      return;
    }
    if (result.isPending || result.isFetching) {
      statusMap.set(rootId, "checking");
    } else if (result.isError) {
      statusMap.set(rootId, classifyError(result.error));
    } else {
      statusMap.set(rootId, result.data ?? "available");
    }
  });

  return statusMap;
}
