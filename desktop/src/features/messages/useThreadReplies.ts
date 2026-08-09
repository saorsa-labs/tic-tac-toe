import { useQuery, useQueryClient } from "@tanstack/react-query";

import {
  threadRepliesKey,
  sortMessages,
} from "@/features/messages/lib/messageQueryKeys";
import { fetchNativeThreadReplies } from "@/features/messages/lib/nativeMessaging";
import { useResolvedHistoryScope } from "@/features/messages/lib/useResolvedHistoryScope";
import type { Channel, RelayEvent } from "@/shared/api/types";

const NATIVE_THREAD_REFRESH_MS = 2_000;

/** Retained for cache/test callers; native history needs no aux-event query. */
export function collectThreadAuxMessageIds(
  threadRootId: string,
  replies: RelayEvent[],
): string[] {
  return [...new Set([threadRootId, ...replies.map((reply) => reply.id)])];
}

/** Fetch a thread subtree into a cache independent from channel window pages. */
export function useThreadReplies(
  activeChannel: Channel | null,
  openThreadRootId: string | null,
) {
  const channelId = activeChannel?.id ?? null;
  const rootId = openThreadRootId ?? "none";
  const queryClient = useQueryClient();
  // Hold a group thread while its durable-history scope is unresolved — the
  // scope-aware key also recomputes (and re-fetches) once it arrives. DMs are
  // deterministic and never block.
  const resolvedScope = useResolvedHistoryScope(channelId);
  const isAwaitingGroupScope =
    activeChannel !== null &&
    activeChannel.channelType !== "forum" &&
    activeChannel.channelType !== "dm" &&
    resolvedScope === null;
  const queryKey = threadRepliesKey(channelId ?? "none", rootId);
  return useQuery({
    queryKey,
    enabled:
      activeChannel !== null &&
      activeChannel.channelType !== "forum" &&
      openThreadRootId !== null &&
      !isAwaitingGroupScope,
    queryFn: async (): Promise<RelayEvent[]> => {
      if (!activeChannel || !openThreadRootId) return [];
      const cacheAtStart =
        queryClient.getQueryData<RelayEvent[]>(queryKey) ?? [];
      const idsAtStart = new Set(cacheAtStart.map((event) => event.id));
      const replies = await fetchNativeThreadReplies(
        activeChannel,
        openThreadRootId,
      );
      const current = queryClient.getQueryData<RelayEvent[]>(queryKey) ?? [];
      const receivedInFlight = current.filter(
        (event) => !idsAtStart.has(event.id),
      );
      return sortMessages([...replies, ...receivedInFlight]);
    },
    staleTime: 0,
    gcTime: 60 * 60 * 1_000,
    // Direct-fallback group delivery is committed to durable history without
    // a matching `/ws` topic frame, so keep an open thread converged from the
    // authoritative store as well as from live frames.
    refetchInterval: NATIVE_THREAD_REFRESH_MS,
  });
}
