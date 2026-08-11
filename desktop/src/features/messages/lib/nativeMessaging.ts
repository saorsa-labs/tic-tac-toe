import type {
  Channel,
  Identity,
  RelayEvent,
  SearchMessagesResponse,
} from "@/shared/api/types";
import {
  buildChannelMessagePayload,
  historyRowToRelayEvent,
} from "@/shared/api/nativeMessageAdapter";
import {
  x0xHistoryGet,
  x0xHistoryList,
  x0xHistorySearch,
  x0xScope,
  x0xSendDirectMessage,
  x0xSendGroupMessage,
  type X0xAgentId,
  type X0xHistoryPage,
  type X0xHistoryRow,
  type X0xScope,
} from "@/shared/api/tauriNativeX0x";
import type {
  ChannelWindowCursor,
  ChannelWindowPage,
  ChannelWindowThreadSummary,
} from "@/features/messages/lib/channelWindowStore";
import { getResolvedHistoryScope } from "@/features/messages/lib/nativeHistoryScopeStore";
import { KIND_STREAM_MESSAGE } from "@/shared/constants/kinds";

const NATIVE_HISTORY_PAGE_SIZE = 200;
const MAX_NATIVE_HISTORY_PAGES = 500;
const AGENT_ID_PATTERN = /^[0-9a-f]{64}$/i;

/** Superseded by ADR-0029 (x0x v0.36.0); retained for historical reference. */
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

/**
 * Native messaging write-path capabilities — what the x0xd transport exposes
 * TODAY. The message UI consults this to hide/disable controls BEFORE submit
 * rather than surfacing the `NATIVE_*_BLOCKER` errors at submit time.
 *
 * Each flag mirrors a blocker above. When a capability flips to `true` the
 * matching blocker becomes an unreachable defensive guard and its control is
 * re-enabled — flipping a flag is the single place transport readiness enters
 * the UI.
 */
export const nativeMessageCapabilities = {
  /** Reply in a thread (publish with threadRoot/threadParent). */
  canReplyInThread: true,
  /** Edit a sent message (native replace-key publish). */
  canEditMessage: false,
  /** Delete / tombstone a sent message. */
  canDeleteMessage: false,
  /** Toggle a reaction on a message. */
  canToggleReaction: false,
} as const;

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

/**
 * Resolve the durable-history REST scope for a channel, or `null` when it is
 * not yet authoritatively known.
 *
 * `nativeScopeForChannel` yields the *live WS backfill* scope — for groups the
 * transient id derived from `channel.id`. Durable history (`x0x_history_list` /
 * `_search` / `_get`) is bound to the daemon-resolved *stable* group id,
 * captured at subscribe time in the history-scope registry, which may differ
 * from the transient id. This resolver returns that authoritative scope ONLY;
 * it never falls back to the REST-derived transient id, so a group whose ids
 * diverge never loads against the wrong scope. Callers must fail, hold, or skip
 * honestly when this returns `null`. DM scopes are deterministic (derived from
 * the single peer AgentId); an unresolvable DM also yields `null`.
 */
export function resolveNativeHistoryScope(channel: Channel): X0xScope | null {
  if (channel.channelType === "dm") {
    try {
      return nativeScopeForChannel(channel);
    } catch {
      return null;
    }
  }
  return getResolvedHistoryScope(channel.id);
}

/**
 * Require the durable-history scope for a single-channel history consumer
 * (cold-load / pagination / thread / get), throwing honestly when it is not
 * yet resolved rather than targeting the wrong scope. The cold-load path holds
 * on the registry (see `useChannelMessagesQuery`), so a resolved scope is known
 * by the time these run in the normal flow; this throw surfaces a genuine
 * unresolved state as a visible failure instead of silent wrong results.
 */
function requireHistoryScope(channel: Channel): X0xScope {
  const scope = resolveNativeHistoryScope(channel);
  if (scope === null) {
    throw new Error(
      `Native durable-history scope for channel ${channel.id} is not resolved; waiting for the live subscription to surface the stable historyScope.`,
    );
  }
  return scope;
}

/**
 * Resolve the single peer AgentId for a one-to-one DM channel. Native DM
 * projections use the peer AgentId as the channel id; the compatibility
 * projection may instead carry it in participants/participantPubkeys. The
 * local agent's own id is excluded so a self-id in the roster is harmless.
 * Throws when the channel does not identify exactly one peer (group-DMs have
 * no native contract — the UI must limit DM open to one recipient).
 */
export function nativeDmRecipientAgentId(
  channel: Channel,
  ownAgentId: string | null | undefined,
): X0xAgentId {
  const own = ownAgentId?.trim().toLowerCase();
  const fromId =
    AGENT_ID_PATTERN.test(channel.id) && channel.id.toLowerCase() !== own
      ? channel.id.toLowerCase()
      : null;
  if (fromId) {
    return fromId;
  }
  const candidates = [
    ...(channel.participantPubkeys ?? []),
    ...(channel.participants ?? []),
  ]
    .filter((value) => AGENT_ID_PATTERN.test(value))
    .map((value) => value.toLowerCase())
    .filter((value) => value !== own);
  const unique = [...new Set(candidates)];
  if (unique.length !== 1) {
    throw new Error(
      "Cannot resolve the native DM recipient: the channel does not identify exactly one peer AgentId.",
    );
  }
  return unique[0];
}

/**
 * Send a one-to-one direct message over `POST /direct/send`.
 *
 * Builds the typed content envelope (same shape as topic publishes), resolves
 * the recipient AgentId, and forwards optional native thread ancestry
 * (`threadRoot`/`threadParent`, 64-hex canonical msg_ids) which x0xd validates
 * to 32 bytes. Returns the envelope's `clientId` so the caller can key the
 * optimistic row and reconcile it with the canonical (msgId-keyed) row when
 * `/history` cold-loads the durable `dm:<peer>` scope — `/ws/direct` does not
 * echo the sender's own outbound.
 */
export async function sendNativeDirectMessage(input: {
  channel: Channel;
  content: string;
  identity: Identity;
  mentionPubkeys?: string[];
  threadRoot?: string | null;
  threadParent?: string | null;
}): Promise<{ clientId: string; createdAt: number }> {
  const recipient = nativeDmRecipientAgentId(
    input.channel,
    input.identity.agentId,
  );
  const native = buildChannelMessagePayload({
    text: input.content.trim(),
    mentions: input.mentionPubkeys,
  });
  await x0xSendDirectMessage({
    agentId: recipient,
    payload: native.payload,
    threadRoot: input.threadRoot ?? null,
    threadParent: input.threadParent ?? null,
  });
  return { clientId: native.clientId, createdAt: native.createdAt };
}

/** Send one native durable message and return its optimistic timeline row. */
export async function sendNativeMessage(input: {
  channel: Channel;
  content: string;
  identity: Identity;
  mentionPubkeys?: string[];
  threadRoot?: string | null;
  threadParent?: string | null;
}): Promise<RelayEvent> {
  const content = input.content.trim();
  const native = buildChannelMessagePayload({
    text: content,
    mentions: input.mentionPubkeys,
  });

  let canonicalId: string | null = null;
  if (input.channel.channelType === "dm") {
    const recipient = nativeDmRecipientAgentId(
      input.channel,
      input.identity.agentId,
    );
    await x0xSendDirectMessage({
      agentId: recipient,
      payload: native.payload,
      threadRoot: input.threadRoot ?? null,
      threadParent: input.threadParent ?? null,
    });
  } else {
    canonicalId = await x0xSendGroupMessage({
      groupId: input.channel.id,
      body: new TextDecoder().decode(native.payload),
      kind: "chat",
      threadRoot: input.threadRoot ?? null,
      threadParent: input.threadParent ?? null,
    });
  }

  const tags: string[][] = [
    ["h", input.channel.id],
    ["p", input.identity.agentId],
  ];
  for (const agentId of input.mentionPubkeys ?? []) {
    if (agentId !== input.identity.agentId) tags.push(["p", agentId]);
  }
  if (input.threadRoot) {
    tags.push(["e", input.threadRoot, "", "root"]);
    if (input.threadParent) {
      tags.push(["e", input.threadParent, "", "reply"]);
    }
  }

  return {
    id: canonicalId ?? native.clientId,
    localKey: native.clientId,
    pubkey: input.identity.agentId,
    created_at: Math.floor(native.createdAt / 1_000),
    kind: KIND_STREAM_MESSAGE,
    tags,
    content,
    sig: "",
  };
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
    scope: requireHistoryScope(channel),
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
      scope: requireHistoryScope(channel),
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

/**
 * Resolve specific native messages by canonical msg_id via the daemon's
 * indexed `x0x_history_get` point lookup — no history paging, no payload
 * scan. Lookups run in parallel; a canonical id is globally unique within one
 * daemon's store, so each resolves to at most one local row.
 *
 * Scope-scoping is preserved: only rows whose `scope` matches this channel's
 * canonical scope are projected (a hit in another scope is a different
 * conversation). Rows that are not renderable channel messages are skipped.
 */
export async function fetchNativeMessagesById(
  channel: Channel,
  messageIds: Set<string>,
): Promise<RelayEvent[]> {
  if (messageIds.size === 0) return [];
  const expectedScope = requireHistoryScope(channel);
  const rows = await Promise.all(
    [...messageIds].map((id) => x0xHistoryGet(id.toLowerCase())),
  );
  const events: RelayEvent[] = [];
  for (const row of rows) {
    if (!row) continue;
    if (row.scope !== expectedScope) continue;
    const event = historyRowToRelayEvent(row, channel.id);
    if (event) events.push(event);
  }
  return events;
}

function rowToSearchHit(row: X0xHistoryRow, channel: Channel) {
  const event = historyRowToRelayEvent(row, channel.id);
  if (!event) return null;
  return {
    eventId: row.msgId,
    content: event.content,
    kind: 9,
    pubkey: event.pubkey,
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
  // Only channels whose durable scope is authoritatively resolved can be
  // searched honestly; unresolved groups are skipped (never queried against the
  // transient REST id) so one unresolved channel cannot yield wrong/empty hits
  // or leak across scopes.
  const scoped: Array<{ channel: Channel; scope: X0xScope }> = [];
  for (const channel of channels) {
    const scope = resolveNativeHistoryScope(channel);
    if (scope !== null) scoped.push({ channel, scope });
  }
  if (scoped.length === 0) return { hits: [], found: 0 };
  const pages = await Promise.all(
    scoped.map(async ({ channel, scope }) => ({
      channel,
      page: await x0xHistorySearch({
        scope,
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
