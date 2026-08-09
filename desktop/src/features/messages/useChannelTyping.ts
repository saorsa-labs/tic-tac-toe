import { useEffect, useMemo } from "react";

import type { Channel, RelayEvent } from "@/shared/api/types";

export type TypingIndicatorEntry = {
  pubkey: string;
  threadHeadId: string | null;
};

/**
 * Channel typing indicators.
 *
 * M3 cutover: typing presence has no native x0x contract. The relay
 * kind:20002 subscription and broadcast are removed; this hook now returns an
 * empty typer list so the UI simply shows no typing indicators rather than
 * reaching a relay transport. The signature is preserved for callers
 * (`ChannelScreen`) so a native typing surface can be wired here later
 * without touching call sites.
 */
export function useChannelTyping(
  _channel: Channel | null,
  _currentAgentId?: string,
  _latestMessageEvent?: RelayEvent | null,
  _eventAuthorityAgentId?: string | null,
): TypingIndicatorEntry[] {
  // Keep the hook reactive (satisfies react-hooks rules) without subscribing.
  useEffect(() => {}, []);
  return useMemo(() => [], []);
}
