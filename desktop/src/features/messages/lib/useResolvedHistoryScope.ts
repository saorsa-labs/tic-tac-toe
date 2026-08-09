import { useSyncExternalStore } from "react";

import {
  getResolvedHistoryScope,
  subscribeHistoryScope,
} from "@/features/messages/lib/nativeHistoryScopeStore";
import type { X0xScope } from "@/shared/api/tauriNativeX0x";

/**
 * Reactive read of a channel's authoritatively-resolved durable-history scope.
 *
 * Returns `null` until the live subscription surfaces the daemon-resolved
 * `historyScope`. Used to (a) HOLD group history consumers (cold-load, thread,
 * ancestors, route resolution) until the scope is known — so a group whose
 * stable id differs from its REST id never loads against the wrong scope — and
 * (b) force a re-render when the scope arrives, which recomputes the scope-aware
 * cache keys (see messageQueryKeys) so resolving/rotating the scope yields a
 * fresh cache partition. DM/forum channels have no registry entry (scopes are
 * deterministic / absent) and read `null` here; callers gate the hold on
 * `channelType` so DMs never block.
 */
export function useResolvedHistoryScope(
  channelId: string | null,
): X0xScope | null {
  return useSyncExternalStore(
    subscribeHistoryScope,
    () => (channelId ? getResolvedHistoryScope(channelId) : null),
    () => (channelId ? getResolvedHistoryScope(channelId) : null),
  );
}
