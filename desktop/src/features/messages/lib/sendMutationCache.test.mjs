import assert from "node:assert/strict";
import test from "node:test";
import { QueryClient } from "@tanstack/react-query";

import {
  acknowledgeOptimisticChannelWindowMessage,
  rollbackOptimisticMessageCache,
} from "../hooks.ts";
import { channelMessagesKey, channelWindowKey } from "./messageQueryKeys.ts";
import { projectChannelWindowMessages } from "./projectChannelWindow.ts";
import {
  emptyChannelWindowStore,
  mergeLiveChannelWindowEvent,
  replaceNewestChannelWindow,
} from "./channelWindowStore.ts";

const CHANNEL_ID = "b".repeat(64);

function message(id, { localKey = undefined, pending = false } = {}) {
  return {
    id,
    localKey,
    pubkey: "a".repeat(64),
    created_at: 1_786_374_720,
    kind: 9,
    tags: [["h", CHANNEL_ID]],
    content: "queued while the peer was offline",
    sig: "c".repeat(128),
    pending,
  };
}

function context(optimisticId) {
  return {
    optimisticId,
    previousMessages: [],
    previousWindow: undefined,
    previousThreadReplies: undefined,
    threadRootId: null,
    channelId: CHANNEL_ID,
    queryKey: channelMessagesKey(CHANNEL_ID),
  };
}

function newestPage(events) {
  return {
    startCursor: null,
    rows: events.map((event) => ({ event, thread: null })),
    aux: [],
    nextCursor: null,
    hasMore: false,
  };
}

test("failed offline send retry renders exactly one acknowledged row", () => {
  const queryClient = new QueryClient();
  const messagesKey = channelMessagesKey(CHANNEL_ID);
  const windowKey = channelWindowKey(CHANNEL_ID);
  const firstOptimistic = message("optimistic-first", {
    localKey: "optimistic-first",
    pending: true,
  });
  queryClient.setQueryData(
    windowKey,
    mergeLiveChannelWindowEvent(emptyChannelWindowStore(), firstOptimistic),
  );
  queryClient.setQueryData(messagesKey, [firstOptimistic]);

  rollbackOptimisticMessageCache(queryClient, context(firstOptimistic.id));

  const afterFirstRollback = queryClient.getQueryData(windowKey);
  assert.ok(afterFirstRollback, "rollback must keep the window store");
  assert.equal(
    afterFirstRollback.liveOverlay.some(
      (event) => event.id === firstOptimistic.id,
    ),
    false,
    "rollback must remove only the failed optimistic row",
  );
  assert.deepEqual(queryClient.getQueryData(messagesKey), []);

  const retryOptimistic = message("optimistic-retry", {
    localKey: "optimistic-retry",
    pending: true,
  });
  queryClient.setQueryData(
    windowKey,
    mergeLiveChannelWindowEvent(
      queryClient.getQueryData(windowKey) ?? emptyChannelWindowStore(),
      retryOptimistic,
    ),
  );
  queryClient.setQueryData(messagesKey, [retryOptimistic]);

  const accepted = message("d".repeat(64));
  const retryWindow = queryClient.getQueryData(windowKey);
  const withoutRetry = {
    ...retryWindow,
    liveOverlay: retryWindow.liveOverlay.filter(
      (event) => event.id !== retryOptimistic.id,
    ),
  };
  queryClient.setQueryData(
    windowKey,
    mergeLiveChannelWindowEvent(withoutRetry, {
      ...accepted,
      localKey: retryOptimistic.id,
    }),
  );
  projectChannelWindowMessages(queryClient, CHANNEL_ID);

  const rendered = queryClient.getQueryData(messagesKey);
  assert.equal(rendered.length, 1);
  assert.equal(rendered[0].id, accepted.id);
  assert.equal(rendered[0].localKey, retryOptimistic.id);
  assert.equal(rendered[0].pending, false);
  assert.equal(
    rendered.some((event) => event.id === firstOptimistic.id),
    false,
    "the first failed SENDING row must not survive the successful retry",
  );
});

test("failed send rollback keeps messages that arrived while the send was in flight", () => {
  const queryClient = new QueryClient();
  const messagesKey = channelMessagesKey(CHANNEL_ID);
  const windowKey = channelWindowKey(CHANNEL_ID);
  const inbound = message("inbound-during-send");
  const optimistic = message("optimistic-send", {
    localKey: "optimistic-send",
    pending: true,
  });
  queryClient.setQueryData(
    windowKey,
    mergeLiveChannelWindowEvent(
      mergeLiveChannelWindowEvent(emptyChannelWindowStore(), inbound),
      optimistic,
    ),
  );
  queryClient.setQueryData(messagesKey, [inbound, optimistic]);

  rollbackOptimisticMessageCache(queryClient, context(optimistic.id));

  const window = queryClient.getQueryData(windowKey);
  assert.equal(
    window.liveOverlay.some((event) => event.id === inbound.id),
    true,
    "a concurrent inbound row must survive rollback of the failed send",
  );
  assert.equal(
    window.liveOverlay.some((event) => event.id === optimistic.id),
    false,
  );
  const remaining = queryClient.getQueryData(messagesKey);
  assert.deepEqual(
    remaining.map((event) => event.id),
    [inbound.id],
  );
});

test("successful DM refresh replaces its receipt with one durable row", () => {
  const queryClient = new QueryClient();
  const messagesKey = channelMessagesKey(CHANNEL_ID);
  const windowKey = channelWindowKey(CHANNEL_ID);
  const optimistic = message("optimistic-send", {
    localKey: "optimistic-send",
    pending: true,
  });
  const optimisticWindow = mergeLiveChannelWindowEvent(
    emptyChannelWindowStore(),
    optimistic,
  );
  queryClient.setQueryData(windowKey, optimisticWindow);
  queryClient.setQueryData(messagesKey, [optimistic]);

  const clientId = "native-client-id";
  const receipt = message(clientId, { localKey: clientId });
  const acknowledgedWindow = acknowledgeOptimisticChannelWindowMessage(
    optimisticWindow,
    receipt,
    optimistic.id,
  );
  queryClient.setQueryData(windowKey, acknowledgedWindow);
  projectChannelWindowMessages(queryClient, CHANNEL_ID);

  const durable = message("e".repeat(64), { localKey: clientId });
  queryClient.setQueryData(
    windowKey,
    replaceNewestChannelWindow(acknowledgedWindow, newestPage([durable])),
  );
  projectChannelWindowMessages(queryClient, CHANNEL_ID);

  const rendered = queryClient.getQueryData(messagesKey);
  assert.equal(rendered.length, 1);
  assert.equal(rendered[0].id, durable.id);
  assert.equal(rendered[0].localKey, clientId);
  assert.equal(rendered[0].pending, false);
});
