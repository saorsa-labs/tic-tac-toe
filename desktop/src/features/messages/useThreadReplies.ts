import { useQuery, useQueryClient } from "@tanstack/react-query";

import {
  threadRepliesKey,
  sortMessages,
} from "@/features/messages/lib/messageQueryKeys";
import { fetchNativeThreadReplies } from "@/features/messages/lib/nativeMessaging";
import type { Channel, RelayEvent } from "@/shared/api/types";

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
  const channelId = activeChannel?.id ?? "none";
  const rootId = openThreadRootId ?? "none";
  const queryClient = useQueryClient();
  const queryKey = threadRepliesKey(channelId, rootId);
  return useQuery({
    queryKey,
    enabled:
      activeChannel !== null &&
      activeChannel.channelType !== "forum" &&
      openThreadRootId !== null,
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
  });
}
