import * as React from "react";
import { useQueryClient } from "@tanstack/react-query";

import { channelsQueryKey } from "@/features/channels/hooks";
import { mergeTimelineCacheMessages } from "@/features/messages/hooks";
import { channelMessagesKey } from "@/features/messages/lib/messageQueryKeys";
import {
  getChannelIdFromTags,
  isThreadReply,
} from "@/features/messages/lib/threading";
import { nativeScopeForChannel } from "@/features/messages/lib/nativeMessaging";
import {
  clearAllResolvedHistoryScopes,
  clearResolvedHistoryScope,
  setResolvedHistoryScope,
} from "@/features/messages/lib/nativeHistoryScopeStore";
import { shouldNotifyForEvent } from "@/features/notifications/lib/shouldNotify";
import {
  liveDirectMessageToRelayEvent,
  liveMessageToRelayEvent,
} from "@/shared/api/nativeMessageAdapter";
import {
  type ManagedMentionWakeDependencies,
  wakeManagedAgentsForStructuredMention,
} from "@/shared/api/managedAgentMentionIdentity";
import {
  subscribeX0xLive,
  type X0xLiveSubscription,
  type X0xScope,
} from "@/shared/api/tauriNativeX0x";
import { CHANNEL_MESSAGE_EVENT_KINDS } from "@/shared/constants/kinds";
import type { Channel, RelayEvent } from "@/shared/api/types";
import {
  createTrailingDebounce,
  type TrailingDebounce,
} from "@/shared/lib/trailingDebounce";

import { isDmNotifiableKind } from "./isDmNotifiableKind";
import { refreshChannelsWhenIdle } from "./refreshChannelsWhenIdle";

export type UseLiveChannelUpdatesOptions = {
  currentAgentId?: string;
  /**
   * When true, DM notifications also fire for the channel the user is
   * currently viewing (normally suppressed).
   */
  notifyForActiveChannel?: boolean;
  onDmMessage?: (event: RelayEvent, channel: Channel) => void;
  onLiveMention?: () => void;
  /**
   * Fired for live "new content" events in a member channel authored by
   * someone other than the current user. Thread replies also fire
   * onThreadReplyNotification so Home inbox activity stays in sync. Used to
   * drive the observed unread-event map that powers sidebar unread state.
   * See `UNREAD_TRIGGER_KINDS` for the exact kind set.
   */
  onChannelMessage?: (channelId: string, event: RelayEvent) => void;
  /**
   * Fired for thread replies that should be surfaced as Home inbox activity.
   */
  onThreadReplyNotification?: (channelId: string, event: RelayEvent) => void;
  /**
   * Fired for external thread replies that do not match the locally-known
   * interest sets. Callers can perform an async backfill and then decide
   * whether to surface the event.
   */
  onThreadReplyCandidate?: (channelId: string, event: RelayEvent) => void;
  /**
   * Fired for replies in threads the user authored, participated in, or
   * follows (non-DM channels only — the DM path owns those). Follows the DM
   * active-channel rule: suppressed for the channel being viewed unless
   * notifyForActiveChannel opts in.
   */
  onThreadReplyDesktopNotification?: (
    channelId: string,
    event: RelayEvent,
  ) => void;
  onSelfChannelMessage?: (event: RelayEvent) => void;
  participatedRootIds?: ReadonlySet<string>;
  followedRootIds?: ReadonlySet<string>;
  authoredRootIds?: ReadonlySet<string>;
  mutedRootIds?: ReadonlySet<string>;
  mutedChannelIds?: ReadonlySet<string>;
};

const LIVE_SUBSCRIPTION_RETRY_BASE_MS = 1_000;
const LIVE_SUBSCRIPTION_RETRY_MAX_MS = 30_000;

// Channel-list revalidation is O(groups); incoming traffic for non-active
// channels arrives in bursts, so coalesce the refetch into a single trailing
// invalidation instead of one per event.
const CHANNELS_INVALIDATE_DEBOUNCE_MS = 500;

// Only "new content" kinds should bump unread state. Shared with the
// catch-up query in useUnreadChannels so the two paths stay in lockstep.
const UNREAD_TRIGGER_KINDS = new Set<number>(CHANNEL_MESSAGE_EVENT_KINDS);

export const EMPTY_SET: ReadonlySet<string> = new Set();

export function isChannelUnreadTriggerKind(kind: number, isDmChannel: boolean) {
  return isDmChannel
    ? isDmNotifiableKind(kind)
    : UNREAD_TRIGGER_KINDS.has(kind);
}

/**
 * Route one live native event into managed-child collaboration wakeup.
 * Adapter `p` tags contain the signed envelope's explicit child AgentIds; the
 * author tag is excluded here before the identity router does its stricter
 * owned-child and stopped-local checks.
 */
export function wakeManagedAgentMentionFromLiveEvent(
  event: RelayEvent,
  dependencies?: ManagedMentionWakeDependencies,
): Promise<string[]> {
  const hasStructuredMention = event.tags.some(
    (tag) =>
      tag[0] === "p" &&
      typeof tag[1] === "string" &&
      tag[1].toLowerCase() !== event.pubkey.toLowerCase(),
  );
  return hasStructuredMention
    ? wakeManagedAgentsForStructuredMention(event, dependencies)
    : Promise.resolve([]);
}

export function withChannelTagFallback(
  event: RelayEvent,
  channelId: string,
): RelayEvent {
  return getChannelIdFromTags(event.tags)
    ? event
    : { ...event, tags: [...event.tags, ["h", channelId]] };
}

function isExternalMentionEvent(event: RelayEvent, currentAgentId: string) {
  return (
    currentAgentId.length > 0 && event.pubkey.toLowerCase() !== currentAgentId
  );
}

function trackSeenEvent(seenEventIds: Set<string>, eventId: string): boolean {
  if (seenEventIds.has(eventId)) {
    return false;
  }

  seenEventIds.add(eventId);
  if (seenEventIds.size > 200) {
    const oldestEventId = seenEventIds.values().next().value;
    if (oldestEventId) {
      seenEventIds.delete(oldestEventId);
    }
  }

  return true;
}

/**
 * M3 cutover: the live channel fan-out now subscribes to each member
 * channel's native x0x group scope (`subscribeX0xLive`) instead of opening a
 * Nostr relay REQ per channel. Native live `message` frames are projected to
 * the existing `RelayEvent` shape via `liveMessageToRelayEvent`, so the rest
 * of the unread/mention/thread pipeline is unchanged.
 *
 * DM channels are delivered via ONE shared `/ws/direct` stream (the daemon
 * auto-subscribes the session to every peer's inbound DM): each
 * `direct_message` frame is routed to its DM channel by `sender` AgentId, so
 * the per-channel unread/mention/notification pipeline is unchanged. The
 * active DM channel additionally consumes its own `/ws/direct` in
 * `useChannelSubscription` for low-latency timeline append.
 */
export function useLiveChannelUpdates(
  channels: Channel[],
  activeChannelId: string | null,
  options: UseLiveChannelUpdatesOptions = {},
) {
  const queryClient = useQueryClient();
  const normalizedCurrentAgentId =
    options.currentAgentId?.trim().toLowerCase() ?? "";
  const seenMentionEventIdsRef = React.useRef(new Set<string>());
  const channelsInvalidateRef = React.useRef<TrailingDebounce | null>(null);
  if (channelsInvalidateRef.current === null) {
    channelsInvalidateRef.current = createTrailingDebounce(() => {
      refreshChannelsWhenIdle({
        isFetching: () =>
          queryClient.isFetching({ queryKey: channelsQueryKey }),
        invalidate: () => {
          void queryClient.invalidateQueries({ queryKey: channelsQueryKey });
        },
        reArm: () => channelsInvalidateRef.current?.trigger(),
      });
    }, CHANNELS_INVALIDATE_DEBOUNCE_MS);
  }
  const invalidateChannelsDebounced = React.useCallback(() => {
    channelsInvalidateRef.current?.trigger();
  }, []);
  const liveChannelIds = React.useMemo(
    () => new Set(channels.map((channel) => channel.id)),
    [channels],
  );
  const dmChannelMap = React.useMemo(
    () =>
      new Map(
        channels
          .filter((channel) => channel.channelType === "dm")
          .map((channel) => [channel.id, channel]),
      ),
    [channels],
  );
  // Keep a live ref so the shared /ws/direct routing callback always sees the
  // latest DM roster without re-subscribing on every channel-list change.
  const dmChannelMapRef = React.useRef(dmChannelMap);
  dmChannelMapRef.current = dmChannelMap;
  const hasDmChannels = dmChannelMap.size > 0;
  const seenDmEventIdsRef = React.useRef(new Set<string>());
  const dmSubscriptionStartedAtRef = React.useRef(0);
  const liveSubsRef = React.useRef(
    new Map<string, { close: () => Promise<void> }>(),
  );
  // Live mirror of normalizedCurrentAgentId read inside subscription callbacks
  // as a stale-stream guard: a frame arriving after an identity rotation (but
  // before the run's cleanup closed the stream) is dropped when the live
  // identity no longer matches the run's captured identity.
  const currentAgentIdRef = React.useRef(normalizedCurrentAgentId);
  currentAgentIdRef.current = normalizedCurrentAgentId;

  // Identity change: NO live stream from the previous identity may survive.
  // Close every group subscription and clear the scope registry so (a) no
  // cross-identity WS stream lingers and (b) durable-history scopes recapture
  // from the new identity's subscriptions. The membership-sync effect and the
  // DM effect below (both keyed on normalizedCurrentAgentId) then re-subscribe
  // for the new identity, repopulating the registry.
  React.useEffect(() => {
    void normalizedCurrentAgentId;
    dmSubscriptionStartedAtRef.current = 0;
    for (const sub of liveSubsRef.current.values()) {
      void sub.close().catch(() => {});
    }
    liveSubsRef.current.clear();
    clearAllResolvedHistoryScopes();
  }, [normalizedCurrentAgentId]);

  // Effect deps use primitive keys so refetches that produce new refs with
  // identical contents don't churn subscriptions. The Set/array memos are
  // still handy for closure reads via useEffectEvent.
  const channelIdsKey = React.useMemo(
    () => [...new Set(channels.map((channel) => channel.id))].sort().join(","),
    [channels],
  );
  // Mirror `channels` into a ref so the subscription effect reads the latest
  // membership without re-subscribing on every unrelated channels refetch
  // (presence/unread/member-list updates emit a new array with the same ids).
  const channelsRef = React.useRef(channels);
  channelsRef.current = channels;

  const handleDmEvent = React.useEffectEvent((event: RelayEvent) => {
    // Only human-visible message kinds should fire DM notifications.
    if (!isDmNotifiableKind(event.kind)) {
      return;
    }

    // Suppress backlog events that predate our subscription — these are
    // historical replays, not live messages.
    if (event.created_at < dmSubscriptionStartedAtRef.current) {
      return;
    }

    const channelId = getChannelIdFromTags(event.tags);
    if (!channelId) {
      return;
    }

    if (!isExternalMentionEvent(event, normalizedCurrentAgentId)) {
      return;
    }

    const dmChannel = dmChannelMap.get(channelId);
    if (!dmChannel) {
      return;
    }

    if (!trackSeenEvent(seenDmEventIdsRef.current, event.id)) {
      return;
    }

    // Don't fire a notification for the channel the user is already viewing,
    // unless the notify-while-viewing setting opts in.
    if (channelId === activeChannelId && !options.notifyForActiveChannel) {
      return;
    }

    options.onDmMessage?.(event, dmChannel);
  });

  const handleIncomingMessage = React.useEffectEvent((event: RelayEvent) => {
    const channelId = getChannelIdFromTags(event.tags);
    if (!channelId) {
      return;
    }

    // Track DM events even for the active channel so the dedup set stays
    // current. The handler itself skips firing the notification callback
    // when the user is already viewing the DM (unless opted in via
    // notifyForActiveChannel).
    handleDmEvent(event);

    if (!liveChannelIds.has(channelId)) {
      if (channelId !== activeChannelId) {
        invalidateChannelsDebounced();
      }
      return;
    }

    const isDmChannel = dmChannelMap.has(channelId);
    const isUnreadTriggerKind = isChannelUnreadTriggerKind(
      event.kind,
      isDmChannel,
    );

    // Let the caller observe self-authored trigger events (e.g. to track
    // thread participation) before the author-exclusion guard filters them.
    if (
      isUnreadTriggerKind &&
      normalizedCurrentAgentId.length > 0 &&
      event.pubkey.toLowerCase() === normalizedCurrentAgentId
    ) {
      options.onSelfChannelMessage?.(event);
    }

    // Notify the unread tracker. Restricted to human-visible message kinds
    // and to events authored by someone other than the current user — your
    // own outgoing messages should never make a channel unread, and
    // reactions / edits / system messages aren't "new content".
    const isExternalTriggerEvent =
      isUnreadTriggerKind &&
      (normalizedCurrentAgentId.length === 0 ||
        event.pubkey.toLowerCase() !== normalizedCurrentAgentId);
    const isThreadedReply = isThreadReply(event.tags);

    if (isExternalTriggerEvent) {
      void wakeManagedAgentMentionFromLiveEvent(event).catch((error) => {
        console.error(
          "Failed to wake managed agent from child-authored mention",
          event.id,
          error,
        );
      });

      const shouldNotify = shouldNotifyForEvent(
        event,
        normalizedCurrentAgentId,
        {
          participatedRootIds: options.participatedRootIds ?? EMPTY_SET,
          followedRootIds: options.followedRootIds ?? EMPTY_SET,
          authoredRootIds: options.authoredRootIds ?? EMPTY_SET,
          mutedRootIds: options.mutedRootIds ?? EMPTY_SET,
          mutedChannelIds: options.mutedChannelIds ?? EMPTY_SET,
          channelId,
        },
      );

      if (!shouldNotify) {
        if (isThreadedReply) {
          options.onThreadReplyCandidate?.(channelId, event);
        }
      } else {
        // Native live messages carry mention p-tags in the projected event,
        // so the single message subscription also covers live mentions — no
        // separate relay mention REQ is needed.
        if (isExternalMentionEvent(event, normalizedCurrentAgentId)) {
          if (trackSeenEvent(seenMentionEventIdsRef.current, event.id)) {
            options.onLiveMention?.();
          }
        }
        options.onChannelMessage?.(channelId, event);
        if (isThreadedReply) {
          options.onThreadReplyNotification?.(channelId, event);
        }
      }

      if (shouldNotify && isThreadedReply) {
        if (
          !dmChannelMap.has(channelId) &&
          (channelId !== activeChannelId || options.notifyForActiveChannel)
        ) {
          options.onThreadReplyDesktopNotification?.(channelId, event);
        }
      }
    }

    // Merge into the timeline cache for the active channel.
    // useChannelSubscription also writes to this cache, but there's a
    // race window where it hasn't connected yet. Writes are idempotent
    // (mergeTimelineCacheMessages deduplicates by event ID).
    queryClient.setQueryData<RelayEvent[]>(
      channelMessagesKey(channelId),
      (current) => {
        if (!current) {
          return current;
        }

        return mergeTimelineCacheMessages(current, event);
      },
    );
  });

  React.useEffect(() => {
    let isCancelled = false;
    let retryTimeout: number | undefined;
    let retryAttempt = 0;
    // channelIdsKey is the membership diff signal that re-triggers this sync;
    // the live channel data is read from `channelsRef` so unrelated roster
    // refetches (same ids, new array ref) do not re-subscribe.
    void channelIdsKey;
    // Capture this run's identity; the frame callback below drops frames if the
    // live identity rotated before this run's cleanup closed the stream.
    const sessionAgentId = normalizedCurrentAgentId;

    const syncSubs = async (): Promise<boolean> => {
      const activeSubs = liveSubsRef.current;
      const targetChannels = channelsRef.current.filter(
        (channel) =>
          channel.channelType !== "dm" && channel.archivedAt === null,
      );
      const targetIds = new Set(targetChannels.map((channel) => channel.id));

      for (const [channelId, sub] of activeSubs) {
        if (!targetIds.has(channelId)) {
          activeSubs.delete(channelId);
          void sub.close().catch(() => {});
          // Drop this channel's resolved durable-history scope so it cannot
          // leak to a later channel reusing the id, or survive a roster leave.
          clearResolvedHistoryScope(channelId);
        }
      }

      if (targetIds.size > 0) {
        // Record the subscription start time so handleDmEvent can distinguish
        // backlog replays (created_at < startedAt) from live messages.
        dmSubscriptionStartedAtRef.current = Math.floor(Date.now() / 1000);
      }

      let anyFailed = false;
      const additions = targetChannels
        .filter((channel) => !activeSubs.has(channel.id))
        .map(async (channel) => {
          let scope: X0xScope;
          try {
            scope = nativeScopeForChannel(channel);
          } catch (error) {
            // No resolvable native scope for this channel — skip silently
            // rather than churning retry storms.
            console.debug(
              "Skipping native live subscription for channel without scope",
              channel.id,
              error,
            );
            return;
          }
          try {
            const subscription = await subscribeX0xLive({ scope }, (frame) => {
              if (isCancelled) return;
              if (currentAgentIdRef.current !== sessionAgentId) return;
              if (frame.type !== "message") return;
              const event = liveMessageToRelayEvent(frame, channel.id);
              if (!event) return;
              handleIncomingMessage(withChannelTagFallback(event, channel.id));
            });
            if (isCancelled) {
              void subscription.close().catch(() => {});
              return;
            }
            activeSubs.set(channel.id, subscription);
            // Capture the daemon-resolved durable history scope (the stable
            // group id, which may differ from the live backfill scope above) so
            // history REST consumers cold-load/page/search/thread against it.
            setResolvedHistoryScope(channel.id, subscription.historyScope);
          } catch (err) {
            anyFailed = true;
            console.error(
              "Failed to open native live subscription",
              channel.id,
              err,
            );
          }
        });
      await Promise.allSettled(additions);
      return !anyFailed;
    };

    const runSync = async () => {
      const ok = await syncSubs();
      if (isCancelled) return;
      if (ok) {
        retryAttempt = 0;
        return;
      }
      const delayMs = Math.min(
        LIVE_SUBSCRIPTION_RETRY_BASE_MS * 2 ** retryAttempt,
        LIVE_SUBSCRIPTION_RETRY_MAX_MS,
      );
      retryAttempt += 1;
      retryTimeout = window.setTimeout(() => {
        retryTimeout = undefined;
        void runSync();
      }, delayMs);
    };

    void runSync();

    return () => {
      isCancelled = true;
      if (retryTimeout !== undefined) {
        window.clearTimeout(retryTimeout);
      }
    };
    // channelIdsKey is the membership diff signal; channels are read via ref.
    // normalizedCurrentAgentId forces a full re-subscribe on identity change.
  }, [channelIdsKey, normalizedCurrentAgentId]);

  // ONE shared `/ws/direct` stream covers every DM channel: the daemon
  // delivers all peers' inbound DMs to the session, so route each
  // `direct_message` frame to its DM channel by `sender` AgentId. Keyed on
  // `hasDmChannels` (not the roster) so the stream does not churn as the DM
  // list changes; routing reads the live `dmChannelMapRef`.
  React.useEffect(() => {
    if (!hasDmChannels) {
      return;
    }
    let isCancelled = false;
    const sessionAgentId = normalizedCurrentAgentId;
    let subscription: X0xLiveSubscription | null = null;
    const firstDm = dmChannelMapRef.current.values().next().value;
    if (!firstDm) {
      return;
    }
    let scope: X0xScope;
    try {
      scope = nativeScopeForChannel(firstDm);
    } catch (error) {
      console.debug(
        "Skipping DM live subscription without resolvable scope",
        error,
      );
      return;
    }
    // Record the start time so handleDmEvent can distinguish backlog replays
    // (created_at < startedAt) from live messages.
    dmSubscriptionStartedAtRef.current = Math.floor(Date.now() / 1000);

    void subscribeX0xLive({ scope }, (frame) => {
      if (isCancelled) return;
      if (currentAgentIdRef.current !== sessionAgentId) return;
      if (frame.type === "error") {
        console.error("Native x0xd DM stream error", frame.message);
        return;
      }
      if (frame.type !== "direct_message") return;
      const peer = frame.sender.toLowerCase();
      const dmChannel = dmChannelMapRef.current.get(peer);
      if (!dmChannel) {
        // DM from a peer with no projected channel — refresh the roster so a
        // new conversation surfaces, then drop this frame.
        invalidateChannelsDebounced();
        return;
      }
      const event = liveDirectMessageToRelayEvent(frame, dmChannel.id);
      if (!event) return;
      handleIncomingMessage(withChannelTagFallback(event, dmChannel.id));
    })
      .then((opened) => {
        if (isCancelled) {
          void opened.close().catch(() => {});
        } else {
          subscription = opened;
        }
      })
      .catch((error) => {
        if (!isCancelled) {
          console.error("Failed to open native DM live subscription", error);
        }
      });

    return () => {
      isCancelled = true;
      void subscription?.close().catch(() => {});
    };
    // hasDmChannels is the only roster diff signal; invalidateChannelsDebounced
    // is stable; normalizedCurrentAgentId closes+reopens the stream on identity
    // change so no cross-identity DM stream survives.
  }, [hasDmChannels, invalidateChannelsDebounced, normalizedCurrentAgentId]);

  React.useEffect(() => {
    return () => {
      channelsInvalidateRef.current?.cancel();

      for (const sub of liveSubsRef.current.values()) {
        void sub.close().catch(() => {});
      }
      liveSubsRef.current.clear();
    };
  }, []);
}
