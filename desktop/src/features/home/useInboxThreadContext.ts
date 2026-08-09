import * as React from "react";

import { isInboxThreadContextEvent } from "@/features/home/lib/inboxViewHelpers";
import { relayEventFromFeedItem } from "@/features/home/lib/inbox";
import { getThreadReference } from "@/features/messages/lib/threading";
import type { FeedItem, RelayEvent } from "@/shared/api/types";

type InboxThreadContextResult = {
  events: RelayEvent[];
};

function dedupeEvents(events: RelayEvent[]): RelayEvent[] {
  const eventsById = new Map<string, RelayEvent>();
  for (const event of events) {
    eventsById.set(event.id, event);
  }
  return [...eventsById.values()].sort((a, b) => a.created_at - b.created_at);
}

function getThreadRootId(event: RelayEvent): string {
  const thread = getThreadReference(event.tags);
  return thread.rootId ?? thread.parentId ?? event.id;
}

/**
 * Thread context for the inbox detail view.
 *
 * M3 cutover: the relay ancestor/descendant fetches and the kind:7 reaction
 * aux hydrate are gone — there is no relay event source. Thread context is now
 * derived synchronously from the loaded channel-message window (the native
 * history projection). Reactions for the rendered messages already ride in that
 * same channel cache, so HomeView merges them from `channelMessages` directly.
 */
export function useInboxThreadContext(
  item: FeedItem | null,
  channelMessages: RelayEvent[] | undefined,
): InboxThreadContextResult {
  const selectedEvent = React.useMemo(
    () => (item ? relayEventFromFeedItem(item) : null),
    [item],
  );

  const selectedThreadRootId = selectedEvent
    ? getThreadRootId(selectedEvent)
    : null;
  const selectedParentId = selectedEvent
    ? getThreadReference(selectedEvent.tags).parentId
    : null;
  const selectedChannelId = item?.channelId ?? null;

  const events = React.useMemo(() => {
    if (!selectedEvent || !selectedThreadRootId) {
      return [];
    }

    const localContext = (channelMessages ?? []).filter((event) =>
      isInboxThreadContextEvent(event, {
        selectedChannelId,
        selectedEventId: selectedEvent.id,
        selectedParentId,
        selectedThreadRootId: selectedThreadRootId,
      }),
    );

    return dedupeEvents([selectedEvent, ...localContext]);
  }, [
    channelMessages,
    selectedChannelId,
    selectedEvent,
    selectedParentId,
    selectedThreadRootId,
  ]);

  return { events };
}
