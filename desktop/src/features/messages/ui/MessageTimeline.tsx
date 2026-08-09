import * as React from "react";

import {
  isDeferredTimelineSnapshotStale,
  isRenderedTimelineBehindHistoryPrepend,
  selectTimelineBodySurface,
  selectTimelineIntroSurface,
} from "@/features/messages/lib/timelineSnapshot";
import { preloadTimelineImages } from "@/features/messages/lib/timelineImagePreload";
import type { TimelineMessage } from "@/features/messages/types";
import type { MainTimelineEntry } from "@/features/messages/lib/threadPanel";
import type { ChannelWindowThreadSummary } from "@/features/messages/lib/channelWindowStore";
import { nativeMessageCapabilities } from "@/features/messages/lib/nativeMessaging";
import type { UserProfileLookup } from "@/features/profile/lib/identity";
import type { ChannelType } from "@/shared/api/types";
import { cn } from "@/shared/lib/cn";
import { channelChrome } from "@/shared/layout/chromeLayout";
import { Spinner } from "@/shared/ui/spinner";
import { TooltipProvider } from "@/shared/ui/tooltip";
import { UnreadPill, unreadCountLabel } from "@/shared/ui/UnreadPill";
import { ChannelIntroBlock, type ChannelIntro } from "./ChannelIntroBlock";
import { TimelineSkeleton, useTimelineSkeletonRows } from "./TimelineSkeleton";
import { TimelineMessageList } from "./TimelineMessageList";
import type { TimelineVirtualizerApi } from "./TimelineMessageList";
import { useAnchoredScroll } from "./useAnchoredScroll";
import { useLoadOlderOnScroll } from "./useLoadOlderOnScroll";
import { useBufferedTimelineMessages } from "./useBufferedTimelineMessages";
import {
  DirectMessageIntroAvatarStack,
  type DirectMessageIntroParticipant,
} from "./DirectMessageIntroAvatarStack";
import { useSettleGatedPrependMessages } from "./useSettleGatedPrependMessages";
import {
  initialSemanticBottomState,
  reduceSemanticBottom,
  type SemanticBottomEvent,
  type SemanticBottomState,
} from "./semanticBottomState";

export type MessageTimelineHandle = {
  scrollToBottomOnNextUpdate: () => void;
};

type MessageTimelineProps = {
  channelId?: string | null;
  channelIntro?: ChannelIntro | null;
  channelName?: string;
  channelType?: ChannelType | null;
  huddleMemberPubkeys?: readonly string[];
  huddleMemberPubkeysPending?: boolean;
  messages: TimelineMessage[];
  mainEntries?: MainTimelineEntry[];
  /** Relay thread summaries (root id → summary) for the deferred-pass entry
   *  fallback, so badge rows survive while a scrollback page commits. */
  threadSummaries?: ReadonlyMap<string, ChannelWindowThreadSummary>;
  directMessageIntro?: {
    displayName: string;
    participants: DirectMessageIntroParticipant[];
  } | null;
  isLoading?: boolean;
  entranceMessageId?: string | null;
  onEntranceMessageComplete?: (messageId: string) => void;
  emptyTitle?: string;
  emptyDescription?: string;
  currentAgentId?: string;
  fetchOlder?: () => Promise<void>;
  hasOlderMessages?: boolean;
  /**
   * True when the loaded window provably starts at the channel's beginning
   * (a resolved tail page with `hasMore: false`) — NOT merely the absence of
   * a paging signal. Gates the oldest loaded day's divider.
   */
  historyExhausted?: boolean;
  /** Optional external ref to the scroll container — used by the parent to
   *  observe scroll position or adjust padding dynamically. */
  scrollContainerRef?: React.RefObject<HTMLDivElement | null>;
  /** True when the timeline has the composer overlay below it. */
  hasComposerOverlay?: boolean;
  isFetchingOlder?: boolean;
  messageFooters?: Record<string, React.ReactNode>;
  /** Map from lowercase pubkey → persona display name for bot members. */
  personaLookup?: Map<string, string>;
  profiles?: UserProfileLookup;
  ownerProfiles?: UserProfileLookup;
  followThreadById?: (rootId: string) => void;
  isFollowingThreadById?: (rootId: string) => boolean;
  isMessageUnreadById?: (messageId: string) => boolean;
  onDelete?: (message: TimelineMessage) => void;
  onEdit?: (message: TimelineMessage) => void;
  onMarkUnread?: (message: TimelineMessage) => void;
  onMarkRead?: (message: TimelineMessage) => void;
  onReply?: (message: TimelineMessage) => void;
  isSendingVideoReviewComment?: boolean;
  onSendVideoReviewComment?: (
    message: TimelineMessage,
    content: string,
    mentionPubkeys: string[],
    mediaTags?: string[][],
    parentEventId?: string,
  ) => Promise<void>;
  unfollowThreadById?: (rootId: string) => void;
  onToggleReaction?: (
    message: TimelineMessage,
    emoji: string,
    remove: boolean,
  ) => Promise<void>;
  /** The message ID of the currently active find-in-channel match. */
  searchActiveMessageId?: string | null;
  /** Set of message IDs that match the current find-in-channel query. */
  searchMatchingMessageIds?: Set<string>;
  /** The current find-in-channel query string. */
  searchQuery?: string;
  targetMessageId?: string | null;
  onTargetReached?: (messageId: string) => void;
  splitThreadPanelOpen?: boolean;
  /** Event id of the oldest unread top-level message at channel open, or null. */
  firstUnreadMessageId?: string | null;
  /** Count of unread top-level messages at channel open. */
  unreadCount?: number;
  /** Per-thread unread counts keyed by thread root id. */
  threadUnreadCounts?: ReadonlyMap<string, number>;
};

/** Stable empty reference used as the `useDeferredValue` initial value so the
 *  first render on channel entry stays light instead of blocking on the full
 *  message list. Must be module-level so its identity never changes. */
const EMPTY_MESSAGES: TimelineMessage[] = [];

type TimelineSnapshot = {
  channelId: string | null;
  messages: TimelineMessage[];
  /**
   * History-exhaustion proof captured with the SAME rows it was derived from.
   * The oldest-day divider may only exist when this is true, and rows and
   * proof must travel every transport stage (deferral, buffering, settle
   * gating) as one value: delivering a fresh proof on the urgent render path
   * while the rows ride the deferred path lets an intermediate commit mint a
   * divider against the previous, partially-loaded oldest day — which breaks
   * Virtua's exact-suffix shift admission when the withheld same-day rows
   * finally land (the pass-1 tear, ledgered 2026-07-11).
   */
  historyExhausted: boolean;
};

const EMPTY_TIMELINE_SNAPSHOT: TimelineSnapshot = {
  channelId: null,
  messages: EMPTY_MESSAGES,
  historyExhausted: false,
};

const MessageTimelineBase = React.forwardRef<
  MessageTimelineHandle,
  MessageTimelineProps
>(function MessageTimeline(
  {
    channelId,
    channelIntro = null,
    directMessageIntro = null,
    messages,
    mainEntries,
    threadSummaries,
    isLoading = false,
    entranceMessageId = null,
    onEntranceMessageComplete,
    emptyTitle = "No messages yet",
    emptyDescription = "Send the first message to start the thread.",
    currentAgentId,
    fetchOlder,
    hasComposerOverlay = true,
    hasOlderMessages = true,
    historyExhausted = false,
    isFetchingOlder = false,
    followThreadById,
    huddleMemberPubkeys,
    huddleMemberPubkeysPending = false,
    isFollowingThreadById,
    isMessageUnreadById,
    messageFooters,
    personaLookup,
    profiles,
    ownerProfiles,
    onDelete,
    onEdit,
    onMarkUnread,
    onMarkRead,
    onReply,
    channelName,
    channelType,
    isSendingVideoReviewComment = false,
    onSendVideoReviewComment,
    onToggleReaction,
    unfollowThreadById,
    scrollContainerRef: externalScrollRef,
    searchActiveMessageId = null,
    searchMatchingMessageIds,
    searchQuery,
    targetMessageId = null,
    onTargetReached,
    splitThreadPanelOpen = false,
    firstUnreadMessageId = null,
    unreadCount = 0,
    threadUnreadCounts,
  }: MessageTimelineProps,
  ref,
) {
  const internalScrollRef = React.useRef<HTMLDivElement>(null);
  const scrollContainerRef = externalScrollRef ?? internalScrollRef;
  const contentRef = React.useRef<HTMLDivElement>(null);
  const topSentinelRef = React.useRef<HTMLDivElement>(null);
  const [virtualizerScrollParent, setVirtualizerScrollParent] =
    React.useState<HTMLDivElement | null>(null);
  const [virtualizerRenderVersion, bumpVirtualizerRenderVersion] =
    React.useReducer((version: number) => version + 1, 0);
  const [timelineVirtualizerApi, setTimelineVirtualizerApi] =
    React.useState<TimelineVirtualizerApi | null>(null);
  const useTimelineVirtualizer = true;
  const activeScrollContainerRef = React.useMemo(
    () => ({
      get current() {
        return virtualizerScrollParent ?? scrollContainerRef.current;
      },
    }),
    [scrollContainerRef, virtualizerScrollParent],
  );

  // Gate the heavy timeline render (each row runs a synchronous
  // react-markdown parse) behind React concurrency. `useDeferredValue` lets the
  // commit that rebuilds the message list yield to higher-priority work, so the
  // main thread stops freezing and the OS no longer shows the busy cursor when
  // entering a channel. We pass `initialValue: []` so even the FIRST render on
  // channel entry stays light — the heavy list streams in on a deferred commit
  // rather than blocking the initial paint. We deliberately drive BOTH the
  // scroll manager and the rendered list off the same deferred value —
  // scroll/autoscroll/deep-link logic reads the DOM (`scrollIntoView`,
  // ResizeObserver on the content), so it must stay consistent with what's
  // actually painted. You can't scroll to a row that hasn't committed yet.
  // Channel id travels with the deferred message snapshot. Without that guard, a
  // route change can paint the previous channel's deferred rows for a frame even
  // though the sidebar/header already moved to the new channel.
  const liveSnapshot = React.useMemo<TimelineSnapshot>(
    () => ({ channelId: channelId ?? null, messages, historyExhausted }),
    [channelId, historyExhausted, messages],
  );
  const deferredSnapshot = React.useDeferredValue(
    liveSnapshot,
    EMPTY_TIMELINE_SNAPSHOT,
  );
  const deferredMessages = deferredSnapshot.messages;
  const imagePreloadStateRef = React.useRef({
    activeImages: new Set<HTMLImageElement>(),
    requestedUrls: new Set<string>(),
  });
  React.useEffect(() => {
    preloadTimelineImages(messages, imagePreloadStateRef.current);
  }, [messages]);
  const isDeferredSnapshotStale = isDeferredTimelineSnapshotStale({
    deferredSnapshot,
    liveSnapshot,
  });
  const isRenderPending = deferredSnapshot !== liveSnapshot;
  const scrollRestorationId = targetMessageId
    ? `message-timeline:${channelId ?? "none"}:target:${targetMessageId}`
    : `message-timeline:${channelId ?? "none"}`;
  // Keep the scroll node's DOM lifetime scoped to a channel. TanStack Router's
  // scroll-restoration listener runs outside React and may write a saved
  // scrollTop into the current scroll element during navigation; reusing the
  // same node across channel routes can leave the newly-loaded message list
  // painted at a stale offset until the user's next scroll event forces layout.
  const scrollContainerDomKey = channelId ?? "none";

  React.useLayoutEffect(() => {
    // Re-read after `scrollContainerDomKey` swaps the keyed scroll DOM node.
    void scrollContainerDomKey;
    if (!useTimelineVirtualizer) {
      setVirtualizerScrollParent(scrollContainerRef.current);
    }
    setTimelineVirtualizerApi(null);
  }, [scrollContainerRef, scrollContainerDomKey]);

  const timelineBodySurface = selectTimelineBodySurface({
    deferredCount: deferredMessages.length,
    hasPersistentIntro: channelIntro !== null || directMessageIntro !== null,
    isLoading: isLoading || isDeferredSnapshotStale,
    liveCount: messages.length,
  });
  const showTimelineSkeleton = timelineBodySurface === "skeleton";
  const [isSemanticallyAtBottom, setIsSemanticallyAtBottom] =
    React.useState(true);
  // Zulip-style data semantics: once the reader leaves the bottom, keep the
  // virtualizer's logical tail frozen. Live arrivals accumulate behind the
  // "new messages" affordance instead of changing Virtua's item model under
  // the reading position. Prepends still flow through immediately and Virtua's
  // `shift` transaction preserves the stable keyed row.
  const bufferedTimeline = useBufferedTimelineMessages({
    channelId,
    isAtBottom:
      isSemanticallyAtBottom ||
      targetMessageId !== null ||
      searchActiveMessageId !== null,
    messages: deferredMessages,
  });
  // Hold older-page render commits until the scroller is at rest: WKWebView
  // can drop scrollTop compensation writes during live trackpad momentum.
  // Full rationale in useSettleGatedPrependMessages.
  //
  // The history-exhaustion proof rides through this gate as snapshot metadata
  // (`meta`), so while a prepend is withheld the rendered rows keep the proof
  // they were projected with. The buffering stage above cannot split the pair:
  // it only freezes the TAIL (live arrivals) and passes history prepends
  // through unchanged, so the oldest rows the proof speaks about are exactly
  // the deferred snapshot's oldest rows.
  const {
    messages: renderedMessages,
    meta: renderedHistoryExhausted,
    isHoldingPrepend,
  } = useSettleGatedPrependMessages({
    channelId,
    messages: bufferedTimeline.messages,
    meta: deferredSnapshot.historyExhausted,
    scrollElementRef: activeScrollContainerRef,
  });

  const {
    highlightedMessageId,
    isAtBottom,
    newMessageCount,
    onScroll,
    scrollToBottom,
    scrollToBottomOnNextUpdate,
    scrollToMessage,
    onVirtualizerAtBottomStateChange,
  } = useAnchoredScroll({
    channelId,
    contentRef,
    isLoading: showTimelineSkeleton,
    messages: renderedMessages,
    onTargetReached,
    scrollContainerRef: activeScrollContainerRef,
    splitPanelOpen: splitThreadPanelOpen,
    targetMessageId,
    virtualScrollToMessage: timelineVirtualizerApi?.scrollToMessage,
    virtualScrollToBottom: timelineVirtualizerApi?.scrollToBottom,
    virtualSettleAtBottom: timelineVirtualizerApi?.settleAtBottom,
    virtualizerOwnsPrependAnchoring: useTimelineVirtualizer,
    virtualizerRenderVersion,
  });

  const stateRef = React.useRef<SemanticBottomState>(
    initialSemanticBottomState(),
  );
  // Monotonic rAF clock. The reducer compares a virtualizer(true)'s frame
  // against `ignoreUntilFrame` to separate the immediate synthetic post-freeze
  // emission from a later genuine physical return. This counter advances only
  // on rAFs the component schedules (the deferred commit + settle ticks), so
  // the reducer's frame arithmetic stays deterministic — no wall-clock timers.
  const frameRef = React.useRef(0);
  const commitRafRef = React.useRef<number | null>(null);
  const settleRafRef = React.useRef<number | null>(null);
  // Advance the rAF clock past the ignore window armed by a freeze commit.
  // Self-terminating: it stops once frameRef reaches ignoreUntilFrame, so a
  // virtualizer(true) landing during the ticks reads before the boundary
  // (synthetic) while one afterwards reads at/after it (genuine return).
  // Bounded to FREEZE_IGNORE_FRAMES ticks — never an open rAF loop.
  const scheduleSettleTicks = React.useCallback(() => {
    const tick = () => {
      settleRafRef.current = null;
      frameRef.current += 1;
      if (frameRef.current < stateRef.current.ignoreUntilFrame) {
        settleRafRef.current = window.requestAnimationFrame(tick);
      }
    };
    settleRafRef.current = window.requestAnimationFrame(tick);
  }, []);
  // Reduce an event into the authoritative state and mirror the committed
  // semantic flag into React state (the buffer + unread pill read it). There
  // is no separate eager ref: the reducer's semanticAtBottom is the single
  // source of truth, so a render-time mirror can never clobber an eager write.
  // Callers follow with syncCommitRaf() so rAF scheduling derives purely from
  // reducer `pending`.
  const applyEvent = React.useCallback((event: SemanticBottomEvent) => {
    const prev = stateRef.current;
    const next = reduceSemanticBottom(prev, event);
    stateRef.current = next;
    if (next.semanticAtBottom !== prev.semanticAtBottom) {
      setIsSemanticallyAtBottom(next.semanticAtBottom);
    }
  }, []);
  // Derive the deferred-commit rAF solely from reducer `pending`: schedule one
  // frame when a freeze is queued, cancel it when nothing is pending
  // (authoritative release, channel reset, or a bounce-back true that cleared
  // pending). The commit applies the deferred value via `flush` and, for a
  // freeze, advances the clock through the ignore window.
  const syncCommitRaf = React.useCallback(() => {
    const wantsRaf = stateRef.current.pending !== null;
    if (wantsRaf && commitRafRef.current === null) {
      commitRafRef.current = window.requestAnimationFrame(() => {
        commitRafRef.current = null;
        frameRef.current += 1;
        applyEvent({ type: "flush", frame: frameRef.current });
        if (stateRef.current.ignoreUntilFrame > 0) {
          scheduleSettleTicks();
        }
      });
    } else if (!wantsRaf && commitRafRef.current !== null) {
      window.cancelAnimationFrame(commitRafRef.current);
      commitRafRef.current = null;
    }
  }, [applyEvent, scheduleSettleTicks]);
  React.useEffect(
    () => () => {
      if (commitRafRef.current !== null) {
        window.cancelAnimationFrame(commitRafRef.current);
      }
      if (settleRafRef.current !== null) {
        window.cancelAnimationFrame(settleRafRef.current);
      }
    },
    [],
  );
  // Authoritative synchronous release (Jump-to-latest / own message): routes
  // through the reducer's `release`, which clears any queued freeze so a stale
  // rAF cannot re-freeze. queueSemanticBottom is gone — virtualizer-driven
  // transitions go through applyEvent + the deferred flush above.
  const commitSemanticBottom = React.useCallback(
    (atBottom: boolean) => {
      applyEvent(
        atBottom
          ? { type: "release" }
          : { type: "virtualizer", atBottom: false, frame: frameRef.current },
      );
      syncCommitRaf();
    },
    [applyEvent, syncCommitRaf],
  );
  const channelResetRef = React.useRef(channelId);
  if (channelResetRef.current !== channelId) {
    channelResetRef.current = channelId;
    // MessageTimeline is reused across channels (only the scroll container
    // remounts), so the unmount cleanup above never runs between switches.
    // Reset synchronously during render — before paint, before any queued rAF
    // fires — by running the reducer's channelReset and cancelling the previous
    // channel's pending commit + settle ticks. confirmedBottom resets to false
    // so Virtua's mount-convergence transient on the new channel is ignored.
    if (commitRafRef.current !== null) {
      window.cancelAnimationFrame(commitRafRef.current);
      commitRafRef.current = null;
    }
    if (settleRafRef.current !== null) {
      window.cancelAnimationFrame(settleRafRef.current);
      settleRafRef.current = null;
    }
    const prevChannel = stateRef.current;
    stateRef.current = reduceSemanticBottom(prevChannel, {
      type: "channelReset",
    });
    if (stateRef.current.semanticAtBottom !== prevChannel.semanticAtBottom) {
      setIsSemanticallyAtBottom(stateRef.current.semanticAtBottom);
    }
  }
  const handleVirtualizerAtBottomStateChange = React.useCallback(
    (atBottom: boolean) => {
      // The reducer ignores a leave until this channel has confirmed bottom
      // once (confirmedBottom), so Virtua's initial scroll-to-end convergence
      // transient never freezes the tail. A virtualizer(true) carries the
      // current frame so the reducer can classify it as synthetic (inside the
      // ignore window) or a genuine physical return (at/after the boundary).
      if (atBottom) {
        applyEvent({
          type: "virtualizer",
          atBottom: true,
          frame: frameRef.current,
        });
        onVirtualizerAtBottomStateChange(true);
        syncCommitRaf();
      } else if (stateRef.current.confirmedBottom) {
        onVirtualizerAtBottomStateChange(false);
        applyEvent({
          type: "virtualizer",
          atBottom: false,
          frame: frameRef.current,
        });
        syncCommitRaf();
      }
    },
    [applyEvent, onVirtualizerAtBottomStateChange, syncCommitRaf],
  );

  const timelineIntroSurface = selectTimelineIntroSurface({
    hasChannelIntro: channelIntro !== null && directMessageIntro === null,
    hasDirectMessageIntro: directMessageIntro !== null,
    hasReachedChannelStart:
      !isRenderedTimelineBehindHistoryPrepend(deferredMessages, messages) &&
      !isHoldingPrepend &&
      (messages.length === 0 || (!hasOlderMessages && !isFetchingOlder)),
    isSkeletonVisible: showTimelineSkeleton,
  });
  const showDirectMessageIntro =
    timelineIntroSurface === "direct-message-intro";
  const showChannelIntro = timelineIntroSurface === "channel-intro";
  const activeDirectMessageIntro = showDirectMessageIntro
    ? directMessageIntro
    : null;
  const activeChannelIntro = showChannelIntro ? channelIntro : null;
  const showIntro =
    activeDirectMessageIntro !== null || activeChannelIntro !== null;
  const showGenericEmpty = timelineBodySurface === "empty" && !showIntro;
  const showMessageList = timelineBodySurface === "list";
  const showChannelIntroOnly = activeChannelIntro !== null && !showMessageList;

  const prepareForOwnMessage = React.useCallback(() => {
    // The user's own send is the deliberate Zulip exception: release buffered
    // output before arming the next-append bottom pin so the sent row can enter
    // Virtua's model and become the new physical floor.
    commitSemanticBottom(true);
    scrollToBottomOnNextUpdate();
  }, [commitSemanticBottom, scrollToBottomOnNextUpdate]);

  React.useImperativeHandle(
    ref,
    () => ({
      scrollToBottomOnNextUpdate: prepareForOwnMessage,
    }),
    [prepareForOwnMessage],
  );

  // Jump-to-message is purely DOM-based now: all loaded rows are mounted, so
  // `scrollToMessage` always finds the target row. No virtualizer convergence.
  const jumpToMessage = React.useCallback(
    (messageId: string, options?: { behavior?: ScrollBehavior }) => {
      return scrollToMessage(messageId, { highlight: true, ...options });
    },
    [scrollToMessage],
  );

  // The unread pill is a transient, per-open affordance: dismiss it once the
  // user acts on it (jumps to the oldest unread) or catches up by reaching the
  // bottom of the timeline. Reset when the channel changes so a freshly opened
  // channel shows its own pill.
  const [isUnreadPillDismissed, setIsUnreadPillDismissed] =
    React.useState(false);
  // Track whether the pill has been shown at least once this channel visit.
  // This prevents the dismiss effect from firing on mount (when isAtBottom
  // initializes as true) before the pill ever renders.
  const hasShownPillRef = React.useRef(false);
  // biome-ignore lint/correctness/useExhaustiveDependencies: reset on channel switch only
  React.useEffect(() => {
    setIsUnreadPillDismissed(false);
    hasShownPillRef.current = false;
  }, [channelId]);
  React.useEffect(() => {
    if (isAtBottom && hasShownPillRef.current) {
      setIsUnreadPillDismissed(true);
    }
  }, [isAtBottom]);
  const showUnreadPill =
    !isUnreadPillDismissed &&
    unreadCount > 0 &&
    firstUnreadMessageId !== null &&
    !showTimelineSkeleton;
  if (showUnreadPill) hasShownPillRef.current = true;
  const handleJumpToOldestUnread = React.useCallback(() => {
    setIsUnreadPillDismissed(true);
    if (firstUnreadMessageId) {
      jumpToMessage(firstUnreadMessageId);
    }
  }, [firstUnreadMessageId, jumpToMessage]);

  // Scroll to the active search match when it changes. `jumpToMessage` updates
  // the scroll anchor (so the post-commit restore won't yank the view back off
  // the match) and, when virtualized, converges on the target through the index
  // model — the row may be windowed out of the DOM.
  const prevSearchActiveRef = React.useRef<string | null>(null);
  const pendingSearchTargetRef = React.useRef<string | null>(null);
  React.useEffect(() => {
    if (showTimelineSkeleton) return;
    if (!searchActiveMessageId) {
      pendingSearchTargetRef.current = null;
    }
    if (
      !searchActiveMessageId ||
      searchActiveMessageId === prevSearchActiveRef.current
    ) {
      prevSearchActiveRef.current = searchActiveMessageId;
      return;
    }
    pendingSearchTargetRef.current = null;
    prevSearchActiveRef.current = searchActiveMessageId;
    if (!jumpToMessage(searchActiveMessageId, { behavior: "smooth" })) {
      pendingSearchTargetRef.current = searchActiveMessageId;
    }
  }, [jumpToMessage, searchActiveMessageId, showTimelineSkeleton]);

  // biome-ignore lint/correctness/useExhaustiveDependencies: deferredMessages and virtualizerRenderVersion are intentional retry triggers — a search hit may be spliced into messages asynchronously, and in virtualized mode a phase-1 index jump only realizes the row; retry when the rendered range changes so the DOM-visible path can center and highlight it.
  React.useEffect(() => {
    const target = pendingSearchTargetRef.current;
    if (!target || showTimelineSkeleton) return;
    if (
      useTimelineVirtualizer &&
      !activeScrollContainerRef.current?.querySelector(
        `[data-message-id="${CSS.escape(target)}"]`,
      )
    ) {
      // Phase 1: ask the virtualizer to realize the match's index. The retry effect
      // runs again on range change and the DOM-visible path does the actual
      // center + highlight once the row exists.
      void jumpToMessage(target, { behavior: "auto" });
      return;
    }
    if (jumpToMessage(target, { behavior: "auto" })) {
      pendingSearchTargetRef.current = null;
    }
  }, [
    deferredMessages,
    jumpToMessage,
    showTimelineSkeleton,
    virtualizerRenderVersion,
  ]);

  const loadOlderViaVirtualizer = React.useCallback((): boolean => {
    // Indexed find navigation can legitimately land near the current history
    // boundary. Do not mistake that programmatic jump for scrollback intent and
    // prepend underneath the active match.
    // A settle-gate hold means the reader is still parked at the OLD
    // boundary — don't stack more page fetches behind the held commit.
    if (
      searchActiveMessageId ||
      !fetchOlder ||
      isFetchingOlder ||
      isHoldingPrepend ||
      showTimelineSkeleton ||
      !hasOlderMessages
    ) {
      return false;
    }
    void fetchOlder();
    return true;
  }, [
    fetchOlder,
    hasOlderMessages,
    isFetchingOlder,
    isHoldingPrepend,
    searchActiveMessageId,
    showTimelineSkeleton,
  ]);

  useLoadOlderOnScroll({
    fetchOlder: useTimelineVirtualizer ? undefined : fetchOlder,
    hasOlderMessages,
    isLoading: showTimelineSkeleton,
    scrollContainerRef: activeScrollContainerRef,
    sentinelRef: topSentinelRef,
  });

  const timelineSkeletonRows = useTimelineSkeletonRows({
    channelId,
    isLoading: showTimelineSkeleton,
    messages: showTimelineSkeleton ? EMPTY_MESSAGES : deferredMessages,
  });

  const virtualizedLeadingContent = React.useMemo(
    () =>
      activeChannelIntro ? (
        <ChannelIntroBlock className="pb-4 pt-2" intro={activeChannelIntro} />
      ) : activeDirectMessageIntro ? (
        <div
          className="mb-2 flex w-full flex-col items-start px-3 pb-2 pt-2 text-left"
          data-testid="message-dm-intro"
        >
          <DirectMessageIntroAvatarStack
            participants={activeDirectMessageIntro.participants}
          />
          <p className="mt-4 max-w-full truncate text-xl font-semibold leading-7 tracking-tight text-foreground">
            {activeDirectMessageIntro.displayName}
          </p>
          <p className="mt-1 max-w-full truncate whitespace-nowrap text-sm leading-5 text-muted-foreground">
            This is the beginning of your direct message with{" "}
            <span className="font-medium text-foreground">
              {activeDirectMessageIntro.displayName}
            </span>
            .
          </p>
        </div>
      ) : null,
    [activeChannelIntro, activeDirectMessageIntro],
  );

  const handleVirtualizerRangeChanged = React.useCallback(() => {
    bumpVirtualizerRenderVersion();
  }, []);

  const timelineList = showMessageList ? (
    <TimelineMessageList
      key={scrollContainerDomKey}
      channelId={channelId}
      channelName={channelName}
      channelType={channelType}
      currentAgentId={currentAgentId}
      firstUnreadMessageId={firstUnreadMessageId}
      followThreadById={followThreadById}
      highlightedMessageId={highlightedMessageId}
      huddleMemberPubkeys={huddleMemberPubkeys}
      huddleMemberPubkeysPending={huddleMemberPubkeysPending}
      isFollowingThreadById={isFollowingThreadById}
      isMessageUnreadById={isMessageUnreadById}
      entranceMessageId={entranceMessageId}
      onEntranceMessageComplete={onEntranceMessageComplete}
      messageFooters={messageFooters}
      mainEntries={renderedMessages === messages ? mainEntries : undefined}
      leadingContent={virtualizedLeadingContent}
      historyExhausted={renderedHistoryExhausted}
      threadSummaries={threadSummaries}
      messages={renderedMessages}
      onDelete={
        nativeMessageCapabilities.canDeleteMessage ? onDelete : undefined
      }
      onEdit={nativeMessageCapabilities.canEditMessage ? onEdit : undefined}
      onMarkUnread={onMarkUnread}
      onMarkRead={onMarkRead}
      onReply={nativeMessageCapabilities.canReplyInThread ? onReply : undefined}
      isSendingVideoReviewComment={isSendingVideoReviewComment}
      onSendVideoReviewComment={onSendVideoReviewComment}
      onStartReached={loadOlderViaVirtualizer}
      onToggleReaction={
        nativeMessageCapabilities.canToggleReaction
          ? onToggleReaction
          : undefined
      }
      onVirtualizerApiChange={setTimelineVirtualizerApi}
      onVirtualizerRangeChanged={handleVirtualizerRangeChanged}
      onVirtualizerScrollerChange={setVirtualizerScrollParent}
      onAtBottomStateChange={handleVirtualizerAtBottomStateChange}
      personaLookup={personaLookup}
      profiles={profiles}
      ownerProfiles={ownerProfiles}
      searchActiveMessageId={searchActiveMessageId}
      searchMatchingMessageIds={searchMatchingMessageIds}
      searchQuery={searchQuery}
      useVirtualizer={useTimelineVirtualizer}
      threadUnreadCounts={threadUnreadCounts}
      unfollowThreadById={unfollowThreadById}
    />
  ) : null;

  return (
    <TooltipProvider delayDuration={200}>
      <div className="relative flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden">
        {showUnreadPill ? (
          <div
            className={cn(
              "pointer-events-none absolute inset-x-0 z-30 flex translate-y-3 justify-center px-4",
              channelChrome.top,
            )}
          >
            <UnreadPill
              direction="up"
              label={unreadCountLabel(unreadCount)}
              onClick={handleJumpToOldestUnread}
              testId="message-unread-pill"
            />
          </div>
        ) : null}
        {/* `isFetchingOlder` clears on fetch resolve, but rows paint a frame
            later (deferred snapshot / settle-gate hold) — keep the spinner up
            until the page actually renders. */}
        {isFetchingOlder ||
        isHoldingPrepend ||
        isRenderedTimelineBehindHistoryPrepend(deferredMessages, messages) ? (
          <div
            className={cn(
              "pointer-events-none absolute inset-x-0 z-30 flex translate-y-3 justify-center px-4",
              channelChrome.top,
            )}
            data-testid="message-timeline-fetching-older"
          >
            <span className="flex items-center rounded-full bg-background/80 p-1.5 shadow-sm ring-1 ring-border/40 backdrop-blur-sm">
              <Spinner className="h-4 w-4 border-2 text-muted-foreground" />
            </span>
          </div>
        ) : null}
        <div
          className={cn(
            "absolute inset-0 overflow-hidden",
            (!useTimelineVirtualizer || !showMessageList) &&
              cn(
                "overflow-y-auto overflow-x-hidden overscroll-contain px-2 pt-1",
                hasComposerOverlay
                  ? "pb-[var(--composer-overlay-height,6rem)]"
                  : "pb-4",
              ),
          )}
          data-buzz-conversation-scroll={
            useTimelineVirtualizer && showMessageList ? undefined : "true"
          }
          data-scroll-restoration-id={scrollRestorationId}
          data-testid={
            useTimelineVirtualizer && showMessageList
              ? undefined
              : "message-timeline"
          }
          key={scrollContainerDomKey}
          onScroll={useTimelineVirtualizer ? undefined : onScroll}
          ref={scrollContainerRef}
        >
          {useTimelineVirtualizer && timelineList ? (
            <div
              className="h-full min-h-0 w-full"
              data-render-pending={isRenderPending ? "true" : undefined}
            >
              {timelineList}
            </div>
          ) : (
            <div
              className={cn(
                "flex w-full flex-col gap-2",
                showChannelIntroOnly
                  ? "pt-[var(--channel-top-chrome-height,4.5rem)]"
                  : channelChrome.contentPadding,
                (showIntro || showGenericEmpty || showMessageList) &&
                  "min-h-full",
              )}
              ref={contentRef}
            >
              {showChannelIntroOnly ? null : (
                <div ref={topSentinelRef} aria-hidden className="h-px" />
              )}

              {/* Fixed-height history slot keeps the virtual spacer's offset
                  stable across load-older fetches. The intro-only state has no
                  history to anchor, so omitting it matches the virtualized
                  leading row's top geometry when the first message arrives. */}
              {showChannelIntroOnly ? null : (
                <div aria-hidden className="h-8" />
              )}

              <div
                className={cn(
                  "flex min-h-[18rem] min-w-0 flex-col gap-2",
                  useTimelineVirtualizer && "min-h-0 flex-1",
                  (showIntro || showGenericEmpty) && "min-h-full",
                  showMessageList &&
                    !showIntro &&
                    !useTimelineVirtualizer &&
                    "mt-auto",
                )}
              >
                {showTimelineSkeleton ? (
                  <TimelineSkeleton rows={timelineSkeletonRows} />
                ) : null}
                {activeDirectMessageIntro ? (
                  <div
                    className="mt-auto flex w-full flex-col items-start px-3 py-2 text-left"
                    data-testid="message-dm-intro"
                  >
                    <DirectMessageIntroAvatarStack
                      participants={activeDirectMessageIntro.participants}
                    />
                    <p className="mt-4 max-w-full truncate text-xl font-semibold leading-7 tracking-tight text-foreground">
                      {activeDirectMessageIntro.displayName}
                    </p>
                    <p className="mt-1 max-w-full truncate whitespace-nowrap text-sm leading-5 text-muted-foreground">
                      This is the beginning of your direct message with{" "}
                      <span className="font-medium text-foreground">
                        {activeDirectMessageIntro.displayName}
                      </span>
                      .
                    </p>
                  </div>
                ) : null}

                {activeChannelIntro ? (
                  /* Top-anchored like the virtualized leading row, so the
                     first message arrives below with zero layout shift. */
                  <ChannelIntroBlock
                    className="py-2"
                    intro={activeChannelIntro}
                  />
                ) : null}

                {showGenericEmpty ? (
                  <div
                    className="mt-auto rounded-2xl border border-dashed border-border/80 bg-card/70 px-6 py-10 text-center shadow-xs"
                    data-testid="message-empty"
                  >
                    <p className="text-base font-semibold tracking-tight">
                      {emptyTitle}
                    </p>
                    <p className="mt-2 text-sm text-muted-foreground">
                      {emptyDescription}
                    </p>
                  </div>
                ) : null}

                {showMessageList ? (
                  <div
                    className={cn(
                      "flex flex-col gap-2",
                      !showIntro && !useTimelineVirtualizer && "mt-auto",
                      useTimelineVirtualizer && "min-h-0 flex-1",
                    )}
                    data-render-pending={isRenderPending ? "true" : undefined}
                  >
                    {timelineList}
                  </div>
                ) : null}
              </div>
            </div>
          )}
        </div>

        {!isAtBottom ? (
          <div
            className={cn(
              "pointer-events-none absolute inset-x-0 z-50 flex justify-center px-4",
              hasComposerOverlay ? "bottom-36" : "bottom-4",
            )}
          >
            <UnreadPill
              direction="down"
              label={
                bufferedTimeline.pendingCount > 0
                  ? unreadCountLabel(bufferedTimeline.pendingCount)
                  : newMessageCount > 0
                    ? unreadCountLabel(newMessageCount)
                    : "Jump to latest"
              }
              onClick={() => {
                commitSemanticBottom(true);
                window.requestAnimationFrame(() => scrollToBottom("auto"));
              }}
              testId="message-scroll-to-latest"
            />
          </div>
        ) : null}
      </div>
    </TooltipProvider>
  );
});

export const MessageTimeline = React.memo(MessageTimelineBase);
