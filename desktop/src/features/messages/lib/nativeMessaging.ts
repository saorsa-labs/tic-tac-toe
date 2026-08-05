import type {
  Channel,
  RelayEvent,
  SearchMessagesResponse,
} from "@/shared/api/types";
import {
  historyRowToRelayEvent,
  decodeChannelMessageEnvelope,
} from "@/shared/api/nativeMessageAdapter";
import {
  x0xGetGroup,
  x0xHistoryList,
  x0xHistorySearch,
  x0xScope,
  type X0xHistoryPage,
  type X0xHistoryRow,
  type X0xScope,
} from "@/shared/api/tauriNativeX0x";
import type {
  ChannelWindowCursor,
  ChannelWindowPage,
  ChannelWindowThreadSummary,
} from "@/features/messages/lib/channelWindowStore";

const NATIVE_HISTORY_PAGE_SIZE = 200;
const MAX_NATIVE_HISTORY_PAGES = 500;
const AGENT_ID_PATTERN = /^[0-9a-f]{64}$/i;

export const NATIVE_THREAD_WRITE_BLOCKER =
  "x0xd does not expose a publish API that accepts threadRoot/threadParent metadata; thread replies are unavailable until that native contract ships.";
export const NATIVE_EDIT_BLOCKER =
  "x0xd does not expose a native message-edit API or replace-key publish contract.";
export const NATIVE_DELETE_BLOCKER =
  "x0xd does not expose a native message-delete/tombstone API.";
export const NATIVE_REACTION_BLOCKER =
  "x0xd does not expose a native message-reaction API.";
export const NATIVE_RICH_MESSAGE_BLOCKER =
  "x0xd native publish does not yet expose typed attachment/custom-emoji metadata; refusing to send relay tags as a fallback.";
export const NATIVE_DM_SEND_BLOCKER =
  "x0xd supports POST /direct/send, but the native Tauri messaging transport does not expose it yet.";

/** Resolve the daemon's canonical durable-history scope for a Buzz channel. */
export function nativeScopeForChannel(channel: Channel): X0xScope {
  if (channel.channelType !== "dm") {
    return x0xScope("group", channel.id);
  }

  // Native channel projections use the peer AgentId as the DM channel id. The
  // compatibility projection may instead carry a single participant id; accept
  // that only when it is unambiguous. Never guess between multiple identities.
  if (AGENT_ID_PATTERN.test(channel.id)) {
    return x0xScope("dm", channel.id.toLowerCase());
  }
  const candidates = [
    ...(channel.participantPubkeys ?? []),
    ...(channel.participants ?? []),
  ]
    .filter((value) => AGENT_ID_PATTERN.test(value))
    .map((value) => value.toLowerCase());
  const unique = [...new Set(candidates)];
  if (unique.length !== 1) {
    throw new Error(
      "Cannot resolve the native x0xd DM scope: the channel does not identify exactly one peer AgentId.",
    );
  }
  return x0xScope("dm", unique[0]);
}

/** Resolve the named group's authenticated chat topic for native publish. */
export async function nativePublishTopic(channel: Channel): Promise<string> {
  if (channel.channelType === "dm") {
    throw new Error(NATIVE_DM_SEND_BLOCKER);
  }
  const group = await x0xGetGroup(channel.id);
  if (!group.chatTopic.trim()) {
    throw new Error(`x0xd group ${channel.id} has no chat topic.`);
  }
  return group.chatTopic;
}

function historyCursor(page: X0xHistoryPage): ChannelWindowCursor | null {
  const beforeId = page.nextCursor?.beforeId;
  const oldest = page.rows.at(-1);
  if (beforeId === undefined || !oldest) return null;
  return {
    createdAt: Math.floor(oldest.seenAtMs / 1_000),
    eventId: oldest.msgId,
    beforeId,
  };
}

function threadSummary(
  rootId: string,
  rows: X0xHistoryRow[],
): ChannelWindowThreadSummary | null {
  const replies = rows.filter(
    (row) => row.threadRoot === rootId && row.msgId !== rootId,
  );
  if (replies.length === 0) return null;
  return {
    replyCount: replies.length,
    descendantCount: replies.length,
    lastReplyAt: Math.max(...replies.map((row) => row.seenAtMs / 1_000)),
    participantPubkeys: [
      ...new Set(
        replies.flatMap((row) => (row.authorAgent ? [row.authorAgent] : [])),
      ),
    ],
  };
}

/** Adapt one x0xd history page to the existing Buzz window-store shape. */
export function nativeHistoryPageToChannelWindowPage(
  page: X0xHistoryPage,
  channel: Channel,
  startCursor: ChannelWindowCursor | null,
): ChannelWindowPage {
  const rows = page.rows.flatMap((row) => {
    const event = historyRowToRelayEvent(row, channel.id);
    if (!event) return [];
    // Replies live in the independent thread cache. A row is top-level when it
    // has no ancestry, or when the daemon explicitly marks it as its own root.
    if (row.threadRoot !== null && row.threadRoot !== row.msgId) return [];
    return [{ event, thread: threadSummary(row.msgId, page.rows) }];
  });
  const nextCursor = historyCursor(page);
  return {
    startCursor,
    rows,
    aux: [],
    nextCursor,
    hasMore: nextCursor !== null,
  };
}

export async function fetchNativeChannelWindow(
  channel: Channel,
  startCursor: ChannelWindowCursor | null = null,
  limit = 50,
): Promise<ChannelWindowPage> {
  const page = await x0xHistoryList({
    scope: nativeScopeForChannel(channel),
    beforeId: startCursor?.beforeId,
    limit,
  });
  return nativeHistoryPageToChannelWindowPage(page, channel, startCursor);
}

/** Page the native scope and return the complete reply subtree for one root. */
export async function fetchNativeThreadReplies(
  channel: Channel,
  rootId: string,
): Promise<RelayEvent[]> {
  const replies: RelayEvent[] = [];
  let beforeId: number | undefined;
  for (
    let pageNumber = 0;
    pageNumber < MAX_NATIVE_HISTORY_PAGES;
    pageNumber += 1
  ) {
    const page = await x0xHistoryList({
      scope: nativeScopeForChannel(channel),
      beforeId,
      limit: NATIVE_HISTORY_PAGE_SIZE,
    });
    for (const row of page.rows) {
      if (row.threadRoot !== rootId || row.msgId === rootId) continue;
      const event = historyRowToRelayEvent(row, channel.id);
      if (event) replies.push(event);
    }
    if (!page.nextCursor) {
      return replies.sort(
        (left, right) =>
          left.created_at - right.created_at || left.id.localeCompare(right.id),
      );
    }
    beforeId = page.nextCursor.beforeId;
  }
  throw new Error(
    `Native thread ${rootId} exceeded the history page safety limit.`,
  );
}

/** Resolve specific native messages without falling back to relay get_event. */
export async function fetchNativeMessagesById(
  channel: Channel,
  messageIds: Set<string>,
): Promise<RelayEvent[]> {
  const pending = new Set([...messageIds].map((id) => id.toLowerCase()));
  const found: RelayEvent[] = [];
  let beforeId: number | undefined;
  for (
    let pageNumber = 0;
    pageNumber < MAX_NATIVE_HISTORY_PAGES;
    pageNumber += 1
  ) {
    const page = await x0xHistoryList({
      scope: nativeScopeForChannel(channel),
      beforeId,
      limit: NATIVE_HISTORY_PAGE_SIZE,
    });
    for (const row of page.rows) {
      if (!pending.delete(row.msgId.toLowerCase())) continue;
      const event = historyRowToRelayEvent(row, channel.id);
      if (event) found.push(event);
    }
    if (pending.size === 0 || !page.nextCursor) return found;
    beforeId = page.nextCursor.beforeId;
  }
  return found;
}

function rowToSearchHit(row: X0xHistoryRow, channel: Channel) {
  const envelope = decodeChannelMessageEnvelope(row.payload);
  if (!envelope) return null;
  return {
    eventId: row.msgId,
    content: envelope.text,
    kind: 9,
    pubkey: row.authorAgent ?? "",
    channelId: channel.id,
    channelName: channel.name,
    createdAt: Math.floor(row.seenAtMs / 1_000),
    score: 0,
    threadRootId:
      row.threadRoot && row.threadRoot !== row.msgId ? row.threadRoot : null,
  };
}

/** Fan scoped x0xd FTS queries out across the resolved native channels. */
export async function searchNativeMessages(
  query: string,
  channels: Channel[],
  limit: number,
): Promise<SearchMessagesResponse> {
  const pages = await Promise.all(
    channels.map(async (channel) => ({
      channel,
      page: await x0xHistorySearch({
        scope: nativeScopeForChannel(channel),
        q: query,
        limit,
      }),
    })),
  );
  const hits = pages
    .flatMap(({ channel, page }) =>
      page.rows.flatMap((row) => {
        const hit = rowToSearchHit(row, channel);
        return hit ? [hit] : [];
      }),
    )
    .sort(
      (left, right) =>
        right.createdAt - left.createdAt ||
        left.eventId.localeCompare(right.eventId),
    )
    .slice(0, limit);
  return { hits, found: hits.length };
}
