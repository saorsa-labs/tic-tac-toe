import * as React from "react";

import { getCachedSearchHitEvent } from "@/app/navigation/searchHitEventCache";
import { useChannelsQuery } from "@/features/channels/hooks";
import { ChannelScreen } from "@/features/channels/ui/ChannelScreen";
import {
  getThreadReference,
  isBroadcastReply,
} from "@/features/messages/lib/threading";
import { fetchNativeMessagesById } from "@/features/messages/lib/nativeMessaging";
import { useProfileQuery } from "@/features/profile/hooks";
import { useIdentityQuery } from "@/shared/api/hooks";
import type { Channel, RelayEvent } from "@/shared/api/types";
import { ViewLoadingFallback } from "@/shared/ui/ViewLoadingFallback";
import { useResolvedHistoryScope } from "@/features/messages/lib/useResolvedHistoryScope";

type ChannelRouteScreenProps = {
  autoSendDraftKey: string | null;
  channelId: string;
  targetMessageId: string | null;
  targetThreadRootId: string | null;
};

const MAX_ROUTE_ANCESTOR_HOPS = 50;

async function resolveNativeRouteEvent(
  channel: Channel | null,
  eventId: string,
): Promise<RelayEvent | null> {
  if (!channel) {
    return null;
  }
  try {
    // Native messages are keyed by BLAKE3 msgId, not a Nostr event id, so the
    // relay get_event lookup cannot resolve them. Resolve via the scoped native
    // history surface instead (id match). fetchNativeMessagesById returns as
    // soon as the id is found, so a recent target is not a full-history scan;
    // a native get-by-msgId endpoint would remove the paging entirely.
    const events = await fetchNativeMessagesById(channel, new Set([eventId]));
    return (
      events.find(
        (event) => event.id.toLowerCase() === eventId.toLowerCase(),
      ) ?? null
    );
  } catch (error) {
    console.error("Failed to load native route event", eventId, error);
    return null;
  }
}

function getReplyParentId(event: RelayEvent): string | null {
  if (isBroadcastReply(event.tags)) {
    return null;
  }

  return getThreadReference(event.tags).parentId;
}

async function fetchRouteTargetEvents(
  channel: Channel | null,
  eventIds: string[],
  targetMessageId: string | null,
  targetThreadRootId: string | null,
): Promise<RelayEvent[]> {
  const eventsById = new Map<string, RelayEvent>();
  const addEvent = (event: RelayEvent | null) => {
    if (event) {
      eventsById.set(event.id, event);
    }
  };

  const uniqueEventIds = [...new Set(eventIds)];
  const initialEvents = await Promise.all(
    uniqueEventIds.map((id) => resolveNativeRouteEvent(channel, id)),
  );
  for (const event of initialEvents) {
    addEvent(event);
  }

  const targetEvent = targetMessageId
    ? (eventsById.get(targetMessageId) ?? null)
    : null;
  if (!targetEvent) {
    return [...eventsById.values()];
  }

  const targetThreadRef = getThreadReference(targetEvent.tags);
  const threadRootId = targetThreadRootId ?? targetThreadRef.rootId ?? null;
  if (threadRootId && !eventsById.has(threadRootId)) {
    addEvent(await resolveNativeRouteEvent(channel, threadRootId));
  }

  let parentId = getReplyParentId(targetEvent);
  let guard = 0;
  while (
    parentId &&
    parentId !== threadRootId &&
    guard < MAX_ROUTE_ANCESTOR_HOPS
  ) {
    const parentEvent =
      eventsById.get(parentId) ??
      (await resolveNativeRouteEvent(channel, parentId));
    if (!parentEvent) {
      break;
    }

    eventsById.set(parentEvent.id, parentEvent);
    parentId = getReplyParentId(parentEvent);
    guard += 1;
  }

  return [...eventsById.values()];
}

export function ChannelRouteScreen({
  autoSendDraftKey,
  channelId,
  targetMessageId,
  targetThreadRootId,
}: ChannelRouteScreenProps) {
  const channelsQuery = useChannelsQuery();
  const identityQuery = useIdentityQuery();
  const profileQuery = useProfileQuery();
  const channels = channelsQuery.data ?? [];
  const activeChannel =
    channels.find((channel) => channel.id === channelId) ?? null;
  // Hold route/target resolution until the group's durable-history scope is
  // resolved so the deep-linked message is fetched from the correct (stable)
  // scope, not the transient REST id; re-evaluates on scope arrival.
  const resolvedRouteScope = useResolvedHistoryScope(channelId);
  const isAwaitingGroupScope =
    activeChannel !== null &&
    activeChannel.channelType !== "forum" &&
    activeChannel.channelType !== "dm" &&
    resolvedRouteScope === null;
  const [targetMessageEvents, setTargetMessageEvents] = React.useState<
    RelayEvent[]
  >(() => {
    const cachedTarget = getCachedSearchHitEvent(targetMessageId);
    return cachedTarget ? [cachedTarget] : [];
  });

  // Reset spliced target events when the channel context changes. Tied to
  // channel identity rather than the route target so clearing the `messageId`
  // param mid-channel keeps the deep-linked row in view. Seeded with the mount
  // key so the initial cache-seeded events survive first commit; only a
  // genuine channel change clears them. Declared before the fetch effect so a
  // channel switch clears stale events before the new target is fetched.
  const previousChannelIdRef = React.useRef<string>(channelId);
  React.useEffect(() => {
    if (previousChannelIdRef.current === channelId) return;
    previousChannelIdRef.current = channelId;
    setTargetMessageEvents([]);
  }, [channelId]);

  React.useEffect(() => {
    let isCancelled = false;

    // Don't wipe already-spliced target events just because the route target
    // cleared (e.g. `onTargetReached` clears the `messageId` URL param once the
    // row is centered). In a channel whose feed doesn't already contain the
    // deep-linked message, the spliced event is the only copy — dropping it on
    // param-clear blanks the timeline. Here we only fetch when there's a target.
    if (!targetMessageId && !targetThreadRootId) {
      return () => {
        isCancelled = true;
      };
    }

    // Hold target resolution while the group scope is unresolved; retry when it
    // arrives (isAwaitingGroupScope flips false on scope resolution).
    if (isAwaitingGroupScope) {
      return () => {
        isCancelled = true;
      };
    }

    const cachedTarget = getCachedSearchHitEvent(targetMessageId);
    if (cachedTarget) {
      setTargetMessageEvents((currentEvents) =>
        currentEvents.some((event) => event.id === cachedTarget.id)
          ? currentEvents
          : [...currentEvents, cachedTarget],
      );
    }

    const eventIds = [
      targetMessageId,
      targetThreadRootId && targetThreadRootId !== targetMessageId
        ? targetThreadRootId
        : null,
    ].filter((eventId): eventId is string => eventId !== null);

    void fetchRouteTargetEvents(
      activeChannel,
      eventIds,
      targetMessageId,
      targetThreadRootId,
    ).then((events) => {
      if (!isCancelled) {
        setTargetMessageEvents((currentEvents) => {
          const eventsById = new Map<string, RelayEvent>();
          for (const event of [...currentEvents, ...events]) {
            eventsById.set(event.id, event);
          }
          return Array.from(eventsById.values());
        });
      }
    });

    return () => {
      isCancelled = true;
    };
  }, [
    activeChannel,
    targetMessageId,
    targetThreadRootId,
    isAwaitingGroupScope,
  ]);

  if (channelsQuery.isPending && !activeChannel) {
    return <ViewLoadingFallback includeHeader kind="channel" />;
  }

  return (
    <ChannelScreen
      activeChannel={activeChannel}
      autoSendDraftKey={autoSendDraftKey}
      currentIdentity={identityQuery.data}
      currentProfile={profileQuery.data}
      targetMessageEvents={targetMessageEvents}
      targetMessageId={targetMessageId}
    />
  );
}
