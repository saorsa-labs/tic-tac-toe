import { useSyncExternalStore } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";

import { channelsQueryKey, useChannelsQuery } from "@/features/channels/hooks";
import {
  resolveNativeHistoryScope,
  searchNativeMessages,
} from "@/features/messages/lib/nativeMessaging";
import {
  getResolvedHistoryScope,
  subscribeHistoryScope,
} from "@/features/messages/lib/nativeHistoryScopeStore";
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

  const channels = useChannelsQuery().data ?? [];

  // Fold the resolved durable-history scopes into the key: a search made before
  // a group's scope resolved (searchNativeMessages skips unresolved groups) is
  // never cached as a complete result — the key changes when any scope arrives
  // and the query re-runs over the now-resolvable channels.
  const scopesKey = useSyncExternalStore(
    subscribeHistoryScope,
    () =>
      channels
        .map(
          (channel) =>
            `${channel.id}:${getResolvedHistoryScope(channel.id) ?? ""}`,
        )
        .join(","),
    () =>
      channels
        .map(
          (channel) =>
            `${channel.id}:${getResolvedHistoryScope(channel.id) ?? ""}`,
        )
        .join(","),
  );

  // A channel-scoped group search whose scope is unresolved must hold (never
  // cache an empty result as complete). Global search re-runs via scopesKey.
  const targetChannel = channelId
    ? (channels.find((channel) => channel.id === channelId) ?? null)
    : null;
  const awaitingTargetScope =
    targetChannel !== null &&
    targetChannel.channelType !== "dm" &&
    resolveNativeHistoryScope(targetChannel) === null;

  return useQuery({
    queryKey: [
      "search-messages",
      trimmedQuery,
      limit,
      channelId ?? null,
      scopesKey,
    ],
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
    enabled: enabled && trimmedQuery.length >= 2 && !awaitingTargetScope,
    staleTime: 30_000,
    gcTime: 5 * 60 * 1_000,
  });
}
