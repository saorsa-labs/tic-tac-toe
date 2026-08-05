import type { QueryClient } from "@tanstack/react-query";

import {
  appendOlderChannelWindow,
  type ChannelWindowStore,
} from "@/features/messages/lib/channelWindowStore";
import { projectChannelWindowMessages } from "@/features/messages/lib/projectChannelWindow";
import { channelWindowKey } from "@/features/messages/lib/messageQueryKeys";
import { fetchNativeChannelWindow } from "@/features/messages/lib/nativeMessaging";
import type { Channel } from "@/shared/api/types";

const CHANNEL_WINDOW_PAGE_SIZE = 50;
export type PageOlderResult = { hasOlderMessages: boolean };
const inFlightPasses = new Map<string, Promise<PageOlderResult>>();

/** Fetch exactly one server-defined older window and append it atomically. */
export function pageOlderMessagesUntilRowFloor(
  queryClient: QueryClient,
  channel: Channel,
  shouldContinue: () => boolean,
): Promise<PageOlderResult> {
  const running = inFlightPasses.get(channel.id);
  if (running) return running;
  const pass = runPage(queryClient, channel, shouldContinue).finally(() => {
    inFlightPasses.delete(channel.id);
  });
  inFlightPasses.set(channel.id, pass);
  return pass;
}

async function runPage(
  queryClient: QueryClient,
  channel: Channel,
  shouldContinue: () => boolean,
): Promise<PageOlderResult> {
  const store = queryClient.getQueryData<ChannelWindowStore>(
    channelWindowKey(channel.id),
  );
  const tail = store?.pages[store.pages.length - 1];
  if (!store || !tail?.hasMore || !tail.nextCursor || !shouldContinue()) {
    return { hasOlderMessages: false };
  }

  const requestCursor = tail.nextCursor;
  const page = await fetchNativeChannelWindow(
    channel,
    requestCursor,
    CHANNEL_WINDOW_PAGE_SIZE,
  );
  if (!shouldContinue()) return { hasOlderMessages: true };
  const retained = queryClient.getQueryData<ChannelWindowStore>(
    channelWindowKey(channel.id),
  );
  if (!retained) return { hasOlderMessages: true };
  const next = appendOlderChannelWindow(retained, page);
  queryClient.setQueryData(channelWindowKey(channel.id), next);
  projectChannelWindowMessages(queryClient, channel.id);
  return { hasOlderMessages: page.hasMore };
}
