import type { MainTimelineEntry } from "./threadPanel";
import type { TimelineMessage } from "../types";
import type { Channel } from "@/shared/api/types";

/**
 * x0xd has no native reaction list/query surface. Deliberately do not hydrate
 * through RelayClient: that would make the supposedly native timeline depend
 * on bridge/Nostr state and resurrect reactions that native peers cannot see.
 */
export function useRenderScopedReactionHydration(input: {
  activeChannel: Channel | null;
  mainTimelineEntries: MainTimelineEntry[];
  threadHeadMessage: TimelineMessage | null;
  threadMessages: MainTimelineEntry[];
}) {
  void input;
}
