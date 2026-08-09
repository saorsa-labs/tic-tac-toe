import type { SearchHit } from "@/shared/api/types";

export type SearchHitDestination = {
  kind: "channel";
  channelId: string;
  messageId?: string;
  threadRootId?: string | null;
};

export async function resolveSearchHitDestination(
  hit: SearchHit,
): Promise<SearchHitDestination | null> {
  if (!hit.channelId) {
    return null;
  }

  return {
    kind: "channel",
    channelId: hit.channelId,
    messageId: hit.eventId,
    threadRootId: hit.threadRootId ?? null,
  };
}
