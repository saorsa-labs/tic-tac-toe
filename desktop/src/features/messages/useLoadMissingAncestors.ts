import * as React from "react";
import { useQueryClient } from "@tanstack/react-query";

import { channelMessagesKey } from "@/features/messages/lib/messageQueryKeys";
import { mergeMessages } from "@/features/messages/hooks";
import { getThreadReference } from "@/features/messages/lib/threading";
import { fetchNativeMessagesById } from "@/features/messages/lib/nativeMessaging";
import type { Channel, RelayEvent } from "@/shared/api/types";

export function useLoadMissingAncestors(
  activeChannel: Channel | null,
  resolvedMessages: RelayEvent[],
) {
  const queryClient = useQueryClient();
  const requestedAncestorIdsRef = React.useRef<Set<string>>(new Set());
  const previousChannelIdRef = React.useRef<string | null>(null);

  React.useEffect(() => {
    const activeChannelId = activeChannel?.id ?? null;
    if (previousChannelIdRef.current === activeChannelId) {
      return;
    }
    previousChannelIdRef.current = activeChannelId;
    requestedAncestorIdsRef.current.clear();
  }, [activeChannel?.id]);

  React.useEffect(() => {
    if (!activeChannel || activeChannel.channelType === "forum") {
      return;
    }

    const knownEvents = new Map(
      resolvedMessages.map((message) => [message.id, message]),
    );
    const missingAncestorIds = new Set<string>();

    for (const message of resolvedMessages) {
      const thread = getThreadReference(message.tags);

      for (const eventId of [thread.parentId, thread.rootId]) {
        if (
          !eventId ||
          knownEvents.has(eventId) ||
          requestedAncestorIdsRef.current.has(eventId)
        ) {
          continue;
        }

        missingAncestorIds.add(eventId);
      }
    }

    if (missingAncestorIds.size === 0) {
      return;
    }

    for (const eventId of missingAncestorIds) {
      requestedAncestorIdsRef.current.add(eventId);
    }

    const maxRequestedAncestors = 500;
    if (requestedAncestorIdsRef.current.size > maxRequestedAncestors) {
      const excess =
        requestedAncestorIdsRef.current.size - maxRequestedAncestors;
      let removed = 0;
      for (const id of requestedAncestorIdsRef.current) {
        if (removed >= excess) {
          break;
        }
        requestedAncestorIdsRef.current.delete(id);
        removed++;
      }
    }

    let isCancelled = false;

    void fetchNativeMessagesById(activeChannel, missingAncestorIds)
      .then((events) => {
        if (isCancelled) return;
        for (const event of events) {
          queryClient.setQueryData<RelayEvent[]>(
            channelMessagesKey(activeChannel.id),
            (current = []) => mergeMessages(current, event),
          );
        }
      })
      .catch((error) => {
        console.error(
          "Failed to load native thread ancestors",
          [...missingAncestorIds],
          error,
        );
      });

    return () => {
      isCancelled = true;
    };
  }, [activeChannel, queryClient, resolvedMessages]);
}
