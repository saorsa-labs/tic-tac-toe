import { useQuery, useQueryClient } from "@tanstack/react-query";

import { channelsQueryKey } from "@/features/channels/hooks";
import { searchNativeMessages } from "@/features/messages/lib/nativeMessaging";
import type { Channel } from "@/shared/api/types";

export function useSearchMessagesQuery(
  query: string,
  options?: {
    channelId?: string;
    enabled?: boolean;
    limit?: number;
  },
) {
  const queryClient = useQueryClient();
  const trimmedQuery = query.trim();
  const enabled = options?.enabled ?? true;
  const limit = options?.limit ?? 12;
  const channelId = options?.channelId;

  return useQuery({
    queryKey: ["search-messages", trimmedQuery, limit, channelId ?? null],
    queryFn: () => {
      const cachedChannels =
        queryClient.getQueryData<Channel[]>(channelsQueryKey) ?? [];
      const scopes = channelId
        ? cachedChannels.filter((channel) => channel.id === channelId)
        : cachedChannels;
      if (scopes.length === 0) {
        throw new Error(
          "Native x0xd search requires a resolved channel scope.",
        );
      }
      return searchNativeMessages(trimmedQuery, scopes, limit);
    },
    enabled: enabled && trimmedQuery.length >= 2,
    staleTime: 30_000,
    gcTime: 5 * 60 * 1_000,
  });
}
