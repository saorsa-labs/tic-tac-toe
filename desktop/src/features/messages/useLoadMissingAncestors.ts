import * as React from "react";
import { useQueryClient } from "@tanstack/react-query";

import { channelMessagesKey } from "@/features/messages/lib/messageQueryKeys";
import { mergeMessages } from "@/features/messages/hooks";
import { getThreadReference } from "@/features/messages/lib/threading";
import { fetchNativeMessagesById } from "@/features/messages/lib/nativeMessaging";
import type { Channel, RelayEvent } from "@/shared/api/types";
import { useResolvedHistoryScope } from "@/features/messages/lib/useResolvedHistoryScope";

export function useLoadMissingAncestors(
  activeChannel: Channel | null,
  resolvedMessages: RelayEvent[],
) {
  const queryClient = useQueryClient();
  const requestedAncestorIdsRef = React.useRef<Set<string>>(new Set());
  const previousScopeIdentityRef = React.useRef<string>("");
  const resolvedScope = useResolvedHistoryScope(activeChannel?.id ?? null);

  // Clear the ancestor dedup set whenever the resolved-scope identity changes
  // — channel change OR scope transition (null->value, A->B) — so ancestors
  // marked requested under a prior/unresolved scope retry against the fresh
  // scope. Declared before the fetch effect so it runs first on transition.
  React.useEffect(() => {
    const scopeIdentity = `${activeChannel?.id ?? ""}:${resolvedScope ?? ""}`;
    if (previousScopeIdentityRef.current === scopeIdentity) {
      return;
    }
    previousScopeIdentityRef.current = scopeIdentity;
    requestedAncestorIdsRef.current.clear();
  }, [activeChannel?.id, resolvedScope]);

  React.useEffect(() => {
    if (!activeChannel || activeChannel.channelType === "forum") {
      return;
    }
    // Hold while the group's durable-history scope is unresolved: do NOT mark
    // ids requested (which would poison dedup against an unresolved-scope
    // failure) nor fetch. Re-evaluates when the scope arrives via resolvedScope.
    if (activeChannel.channelType !== "dm" && resolvedScope === null) {
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
  }, [activeChannel, queryClient, resolvedMessages, resolvedScope]);
}
