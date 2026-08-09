import * as React from "react";

import {
  collectMessageAuthorPubkeys,
  collectMessageMentionPubkeys,
  collectReactionActorPubkeys,
} from "@/features/messages/lib/formatTimelineMessages";
import type { RelayEvent } from "@/shared/api/types";

export function useMessageEventProfilePubkeys(
  messages: RelayEvent[],
  threadReplies: RelayEvent[],
  eventAuthorityAgentId: string | null | undefined,
) {
  return React.useMemo(() => {
    const events = [...messages, ...threadReplies];
    return [
      ...new Set([
        ...collectMessageAuthorPubkeys(events, eventAuthorityAgentId),
        ...collectMessageMentionPubkeys(events),
        ...collectReactionActorPubkeys(events, eventAuthorityAgentId),
      ]),
    ];
  }, [messages, eventAuthorityAgentId, threadReplies]);
}
