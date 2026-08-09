import { listen, type UnlistenFn } from "@tauri-apps/api/event";

/** Payload emitted by the Rust handler for `buzz://message?…`. */
export type MessageDeepLinkPayload = {
  channelId: string;
  messageId: string;
  threadRootId: string | null;
};

/** Register application message links inside the router tree. */
export function listenForMessageDeepLinks(
  onOpen: (payload: MessageDeepLinkPayload) => void,
): Promise<UnlistenFn> {
  return listen<MessageDeepLinkPayload>("deep-link-message", (event) => {
    onOpen(event.payload);
  });
}
