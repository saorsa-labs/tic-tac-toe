import type { RelayEvent } from "@/shared/api/types";
import { getResolvedHistoryScope } from "@/features/messages/lib/nativeHistoryScopeStore";

/**
 * The resolved durable-history scope participates in every history cache key so
 * that resolving (null -> stable) or rotating (identity change) the scope
 * produces a fresh cache partition: prior-scope/pending data can never be
 * displayed after the scope changes, and an identity clear orphans the old
 * partitions for GC. Group scopes resolve asynchronously via the live
 * subscription; DM scopes are deterministic and stay `null` here (a stable
 * partition for DMs).
 */
export function channelMessagesKey(channelId: string) {
  return [
    "channel-messages",
    channelId,
    getResolvedHistoryScope(channelId),
  ] as const;
}

export function channelWindowKey(channelId: string) {
  return [
    "channel-window",
    channelId,
    getResolvedHistoryScope(channelId),
  ] as const;
}

export function threadRepliesKey(channelId: string, rootId: string) {
  return [
    "thread-replies",
    channelId,
    rootId,
    getResolvedHistoryScope(channelId),
  ] as const;
}

export function dedupeMessagesById(messages: RelayEvent[]) {
  const seenIds = new Set<string>();
  const deduped: RelayEvent[] = [];

  for (let index = messages.length - 1; index >= 0; index -= 1) {
    const message = messages[index];

    if (seenIds.has(message.id)) {
      continue;
    }

    seenIds.add(message.id);
    deduped.push(message);
  }

  return deduped.reverse();
}

export function sortMessages(messages: RelayEvent[]) {
  return dedupeMessagesById(messages).sort((left, right) => {
    if (left.created_at !== right.created_at) {
      return left.created_at - right.created_at;
    }
    // Tiebreak same-second events on id so the merge order is deterministic.
    // Without this, two events sharing a created_at can land in a different
    // position depending on which REQ (history vs live-sub) delivered them
    // first — reading as a "missing"/shuffled message at a fixed scroll offset.
    return left.id < right.id ? -1 : left.id > right.id ? 1 : 0;
  });
}

export function normalizeTimelineMessages(messages: RelayEvent[]) {
  return sortMessages(messages);
}

function isOlderHistoryPage(current: RelayEvent[], history: RelayEvent[]) {
  if (current.length === 0 || history.length === 0) {
    return false;
  }

  const sortedCurrent = sortMessages(current);
  const sortedHistory = sortMessages(history);
  const newestHistory = sortedHistory[sortedHistory.length - 1]?.created_at;
  const oldestCurrent = sortedCurrent[0]?.created_at;

  if (newestHistory === undefined || oldestCurrent === undefined) {
    return false;
  }

  return newestHistory <= oldestCurrent;
}

function normalizeTimelineHistoryMessages(
  current: RelayEvent[],
  history: RelayEvent[],
) {
  return sortMessages([...current, ...history]);
}

export function mergeTimelineHistoryMessages(
  current: RelayEvent[],
  history: RelayEvent[],
) {
  if (isOlderHistoryPage(current, history)) {
    return normalizeTimelineHistoryMessages(current, history);
  }

  return normalizeTimelineMessages([...current, ...history]);
}
