import { useSyncExternalStore } from "react";
import { useQuery } from "@tanstack/react-query";

import { useChannelsQuery } from "@/features/channels/hooks";
import { buildNativeHomeFeed } from "@/features/home/lib/nativeHomeFeed";
import {
  getResolvedHistoryScope,
  subscribeHistoryScope,
} from "@/features/messages/lib/nativeHistoryScopeStore";

/**
 * Home inbox feed, derived from native x0x durable history (M3 cutover).
 *
 * Replaces the unregistered relay `get_feed` invoke. The query fans out one
 * bounded `x0x_history_list` request per active channel (via the shared
 * `channels` query), classifies native mentions/activity honestly, and leaves
 * relay-only buckets (`needsAction`, `agentActivity`) empty rather than
 * fabricated. Native daemon errors propagate so the inbox surfaces them.
 *
 * `currentAgentId` participates in the query key so mention classification is
 * cached per-identity; both call sites (Home inbox, notifications) pass the
 * same agent id, so the feed is fetched once and shared.
 */
export function useHomeFeedQuery(options?: { currentAgentId?: string }) {
  const channelsQuery = useChannelsQuery();
  const channels = channelsQuery.data;
  const currentAgentId = options?.currentAgentId;

  // The channel-id list in the key refetches the feed when the joined channel
  // set changes; `null` while channels are unresolved so the first resolved
  // snapshot triggers exactly one build.
  const channelKey = channels ? channels.map((c) => c.id).join(",") : null;
  // A group's durable-history scope is resolved asynchronously by the live
  // subscriptions and the feed can only query resolved scopes. Participate the
  // resolved-scopes token in the key so the feed refetches the moment any
  // group's stable historyScope arrives — instead of staying empty until the
  // next 30s interval. (Primitive string snapshot; value-stable under
  // useSyncExternalStore.)
  const resolvedScopesKey = useSyncExternalStore(
    subscribeHistoryScope,
    () =>
      channels
        ? channels
            .map((c) => `${c.id}:${getResolvedHistoryScope(c.id) ?? ""}`)
            .join(",")
        : "",
    () =>
      channels
        ? channels
            .map((c) => `${c.id}:${getResolvedHistoryScope(c.id) ?? ""}`)
            .join(",")
        : "",
  );

  const feedQuery = useQuery({
    queryKey: [
      "home-feed",
      "native",
      currentAgentId ?? null,
      channelKey,
      resolvedScopesKey,
    ],
    queryFn: () =>
      buildNativeHomeFeed({
        channels: channels ?? [],
        currentAgentId,
      }),
    // Wait for the channel list to resolve (snapshot or fetch) before deriving,
    // so an empty resolved list renders an honest empty feed rather than a
    // perpetual loading state.
    enabled: channels !== undefined,
    staleTime: 15_000,
    gcTime: 5 * 60 * 1_000,
    refetchInterval: 30_000,
  });

  // Surface channel-list loading as feed loading so the inbox paints its
  // loading state before any feed data can exist.
  return {
    ...feedQuery,
    isLoading: feedQuery.isLoading || channelsQuery.isLoading,
  };
}
