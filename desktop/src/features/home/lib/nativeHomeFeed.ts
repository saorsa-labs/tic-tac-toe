/**
 * Native x0x history-derived Home feed (M3 cutover).
 *
 * Replaces the relay `get_feed` invoke. The Home inbox is now derived purely
 * from the daemon's durable-history surface — one bounded `x0x_history_list`
 * request per active channel — reusing the same channel-list, history-paging,
 * thread-metadata, mention, and profile-projection patterns the rest of the
 * messaging surface already binds to.
 *
 * Category honesty (ADR: no fabricated classification):
 * - `mentions`   — rows whose decoded envelope explicitly mentions the current
 *                  agent (native data supports this: the envelope carries
 *                  `mentions: AgentId[]`).
 * - `activity`   — every other renderable recent channel message.
 * - `needsAction`/`agentActivity` — relay-only server classifications with no
 *                  native-history signal. Left EMPTY rather than fabricated;
 *                  the inbox renders an honest empty state for those filters.
 *
 * Pagination is bounded: at most `MAX_FEED_CHANNELS` channels (most-recently-
 * active first), one bounded request each, with capped mention/activity
 * buckets — no N+1 fan-out beyond the single per-channel request.
 */
import type {
  Channel,
  FeedItem,
  FeedItemCategory,
  HomeFeedResponse,
  RelayEvent,
} from "@/shared/api/types";
import { historyRowToRelayEvent } from "@/shared/api/nativeMessageAdapter";
import {
  x0xHistoryList,
  type X0xHistoryRow,
  type X0xScope,
} from "@/shared/api/tauriNativeX0x";
import { resolveNativeHistoryScope } from "@/features/messages/lib/nativeMessaging";

/** Rows pulled per channel scope (newest-first). */
const DEFAULT_PER_CHANNEL_LIMIT = 20;
/** Safety cap on channels queried, most-recently-active first. */
const MAX_FEED_CHANNELS = 50;
/** Caps on the returned buckets, newest-first. */
const MAX_MENTIONS = 30;
const MAX_ACTIVITY = 80;

export type NativeHomeFeedInput = {
  channels: readonly Channel[];
  /** Current agent's AgentId — drives native mention classification. */
  currentAgentId?: string;
  /** Override the per-channel page size (tests / tuning). */
  perChannelLimit?: number;
};

type ScopedChannel = { channel: Channel; scope: X0xScope };

function channelRecency(channel: Channel): number {
  // `lastMessageAt` is an ISO timestamp string (or null when never active).
  const ms = channel.lastMessageAt ? Date.parse(channel.lastMessageAt) : NaN;
  return Number.isFinite(ms) ? ms : 0;
}

/**
 * Resolve the daemon durable-history scope for each active channel, newest-
 * activity first, capped at `MAX_FEED_CHANNELS`. Archived channels, groups
 * whose stable historyScope is not yet resolved by the live subscription, and
 * DMs whose peer AgentId cannot be unambiguously resolved are skipped — a
 * projection skip, never a fallback to the transient REST id, matching the
 * fail/hold/skip contract of the history paths.
 */
function resolveFeedScopes(channels: readonly Channel[]): ScopedChannel[] {
  const eligible = channels.filter((channel) => !channel.archivedAt);
  // Most-recently-active first so the cap drops the stale tail, not live feeds.
  const ordered = [...eligible].sort(
    (left, right) => channelRecency(right) - channelRecency(left),
  );

  const scoped: ScopedChannel[] = [];
  for (const channel of ordered) {
    if (scoped.length >= MAX_FEED_CHANNELS) break;
    const scope = resolveNativeHistoryScope(channel);
    if (scope !== null) scoped.push({ channel, scope });
  }
  return scoped;
}

/**
 * True when `agentId` is mentioned by (but not the author of) `event`. Native
 * mention p-tags are reconstructed from the decoded envelope inside
 * `historyRowToRelayEvent`, so this reads the same tag set the renderer uses.
 */
function eventMentionsAgent(
  event: RelayEvent,
  agentId: string | undefined,
): boolean {
  if (!agentId) return false;
  const me = agentId.toLowerCase();
  if (!event.pubkey || event.pubkey.toLowerCase() === me) return false;
  return event.tags.some(
    (tag) =>
      tag[0] === "p" &&
      typeof tag[1] === "string" &&
      tag[1].toLowerCase() === me,
  );
}

/** Project a durable-history row to a Home-feed item, or `null` to skip. */
function rowToFeedItem(
  row: X0xHistoryRow,
  channel: Channel,
  currentAgentId: string | undefined,
): FeedItem | null {
  // Reuse the single rendering-layer decode: non-text / undecodable rows and
  // non-message content types map to `null` and are dropped by the caller.
  const event = historyRowToRelayEvent(row, channel.id);
  if (!event) return null;

  const category: FeedItemCategory = eventMentionsAgent(event, currentAgentId)
    ? "mention"
    : "activity";

  return {
    id: event.id,
    kind: event.kind,
    pubkey: event.pubkey,
    content: event.content,
    createdAt: event.created_at,
    channelId: channel.id,
    channelName: channel.name,
    channelType: channel.channelType,
    tags: event.tags,
    category,
  };
}

/**
 * Derive the Home feed from native durable history. Issues one bounded
 * `x0x_history_list` request per active channel (concurrent); native daemon
 * errors propagate to the caller so the UI surfaces them honestly.
 */
export async function buildNativeHomeFeed(
  input: NativeHomeFeedInput,
): Promise<HomeFeedResponse> {
  const perChannelLimit = input.perChannelLimit ?? DEFAULT_PER_CHANNEL_LIMIT;
  const scoped = resolveFeedScopes(input.channels);
  if (scoped.length === 0) {
    return {
      feed: {
        mentions: [],
        needsAction: [],
        activity: [],
        agentActivity: [],
      },
      meta: { since: 0, total: 0, generatedAt: Math.floor(Date.now() / 1_000) },
    };
  }

  // One bounded request per active channel. A rejected request propagates as a
  // native error — the Home query surfaces it instead of falling back to relay.
  const pages = await Promise.all(
    scoped.map(async ({ channel, scope }) => {
      const page = await x0xHistoryList({ scope, limit: perChannelLimit });
      return { channel, rows: page.rows };
    }),
  );

  const mentions: FeedItem[] = [];
  const activity: FeedItem[] = [];
  for (const { channel, rows } of pages) {
    for (const row of rows) {
      const item = rowToFeedItem(row, channel, input.currentAgentId);
      if (!item) continue;
      if (item.category === "mention") {
        mentions.push(item);
      } else {
        activity.push(item);
      }
    }
  }

  const cappedMentions = mentions
    .sort((left, right) => right.createdAt - left.createdAt)
    .slice(0, MAX_MENTIONS);
  const cappedActivity = activity
    .sort((left, right) => right.createdAt - left.createdAt)
    .slice(0, MAX_ACTIVITY);
  const allItems = [...cappedMentions, ...cappedActivity];

  const since = allItems.length
    ? allItems.reduce((min, item) => Math.min(min, item.createdAt), Infinity)
    : 0;

  return {
    feed: {
      mentions: cappedMentions,
      // Native history carries no actionable-item signal; left empty honestly.
      needsAction: [],
      activity: cappedActivity,
      // Native history rows carry no bot/human split; left empty honestly.
      agentActivity: [],
    },
    meta: {
      since,
      total: allItems.length,
      generatedAt: Math.floor(Date.now() / 1_000),
    },
  };
}
