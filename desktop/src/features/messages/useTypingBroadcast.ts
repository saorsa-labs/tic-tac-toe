import { useCallback } from "react";

/**
 * Publishes typing indicators for the current user.
 *
 * M3 cutover: typing broadcast has no native x0x contract. The relay
 * kind:20002 publish is removed; this hook returns a no-op so the composer
 * keeps its call site without reaching a relay transport. Typing indicators
 * simply do not propagate in the native workspace.
 */
export function useTypingBroadcast(
  _channelId: string | null | undefined,
  _parentEventId?: string | null,
  _rootEventId?: string | null,
) {
  return useCallback(() => {}, []);
}
