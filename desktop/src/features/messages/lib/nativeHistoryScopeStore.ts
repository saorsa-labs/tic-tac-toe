import type { X0xScope } from "@/shared/api/tauriNativeX0x";

/**
 * Authoritative per-channel durable-history scope registry.
 *
 * The x0xd daemon binds a group's REST durable history to its *stable* group
 * id, which it returns as `historyScope` on `subscribeX0xLive`. That stable id
 * may differ from the transient id used for the live WS backfill (the id
 * `nativeScopeForChannel` derives from `channel.id`). History REST consumers
 * (`x0x_history_list` / `_search` / `_get`) must therefore target the
 * daemon-resolved scope captured here, never the transient id — otherwise a
 * group whose ids diverge would cold-load, page, search, and thread-resolve
 * against the wrong scope (silent empty/wrong results, or cross-channel
 * leakage).
 *
 * This module is the single source of truth for that resolved scope. It is:
 *  - keyed by REST channel id, so channels never share a scope;
 *  - populated by the live subscription paths (the fan-out in
 *    `useLiveChannelUpdates` and the active-channel `useChannelSubscription`)
 *    the moment `historyScope` arrives;
 *  - cleared per-channel when a channel leaves the roster and wholesale on
 *    identity/session transition, so no stale scope outlives its channel or
 *    session.
 *
 * DM scopes are deterministic (derived from the single peer AgentId) and are
 * intentionally NOT stored here — only group scopes, which require the
 * daemon's resolution, live in this registry.
 */

const SCOPE_PATTERN = /^(?:dm|group|topic):.+$/;

type Listener = () => void;

const scopesByChannelId = new Map<string, X0xScope>();
const listeners = new Set<Listener>();

function emit(): void {
  for (const listener of listeners) {
    listener();
  }
}

/**
 * Record the authoritative durable-history scope for a channel. Idempotent:
 * re-setting the same value is a no-op (no notification). Malformed values are
 * ignored — the channel simply stays unresolved until a well-formed scope
 * arrives, rather than silently persisting a bad scope.
 */
export function setResolvedHistoryScope(
  channelId: string,
  scope: string | null | undefined,
): void {
  if (!channelId || !scope) return;
  // Only well-formed canonical scopes (`dm|group|topic:<id>`) are persisted;
  // a malformed daemon value leaves the channel unresolved instead of caching
  // a bad scope.
  const resolved = SCOPE_PATTERN.test(scope) ? (scope as X0xScope) : null;
  if (resolved === null) return;
  if (scopesByChannelId.get(channelId) === resolved) return;
  scopesByChannelId.set(channelId, resolved);
  emit();
}

/** Drop one channel's resolved scope (e.g. when it leaves the roster). */
export function clearResolvedHistoryScope(channelId: string): void {
  if (!channelId) return;
  if (!scopesByChannelId.delete(channelId)) return;
  emit();
}

/** Drop every resolved scope — used on identity/session transition. */
export function clearAllResolvedHistoryScopes(): void {
  if (scopesByChannelId.size === 0) return;
  scopesByChannelId.clear();
  emit();
}

/** Read the resolved durable-history scope for a channel, or `null` if unknown. */
export function getResolvedHistoryScope(channelId: string): X0xScope | null {
  return scopesByChannelId.get(channelId) ?? null;
}

/**
 * Subscribe to store changes for `useSyncExternalStore`. Returns an
 * unsubscribe function.
 */
export function subscribeHistoryScope(listener: Listener): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}
