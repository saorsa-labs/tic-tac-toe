import { useEffect, useEffectEvent } from "react";
import {
  type QueryClient,
  type QueryKey,
  useMutation,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";
import { toast } from "sonner";

import {
  channelMessagesKey,
  channelWindowKey,
  threadRepliesKey,
} from "@/features/messages/lib/messageQueryKeys";
import {
  buildReplyTags,
  getThreadReference,
  normalizeMentionPubkeys,
  resolveReplyRootId,
} from "@/features/messages/lib/threading";
import { projectChannelWindowMessages } from "@/features/messages/lib/projectChannelWindow";
import { reconcileChannelWindowMessages } from "@/features/messages/lib/channelWindowReconciliation";
import {
  mergeMessages,
  mergeTimelineCacheMessages,
} from "@/features/messages/lib/messageMerge";

export { mergeMessages, mergeTimelineCacheMessages };
import { splitOutgoingTags } from "@/features/messages/lib/imetaMediaMarkdown";
import { messageMentionPubkeys } from "@/features/messages/lib/messageMentionPubkeys";
import {
  clearTimeoutState,
  recordTimeoutFromRejection,
} from "@/features/moderation/lib/timeoutStore";
import { channelsQueryKey } from "@/features/channels/hooks";
import {
  NATIVE_DELETE_BLOCKER,
  NATIVE_EDIT_BLOCKER,
  NATIVE_REACTION_BLOCKER,
  NATIVE_RICH_MESSAGE_BLOCKER,
  fetchNativeChannelWindow,
  nativeScopeForChannel,
  sendNativeMessage,
} from "@/features/messages/lib/nativeMessaging";
import { setResolvedHistoryScope } from "@/features/messages/lib/nativeHistoryScopeStore";
import { useResolvedHistoryScope } from "@/features/messages/lib/useResolvedHistoryScope";
import {
  liveDirectMessageToRelayEvent,
  liveMessageToRelayEvent,
} from "@/shared/api/nativeMessageAdapter";
import { subscribeX0xLive } from "@/shared/api/tauriNativeX0x";
import type { Channel, Identity, RelayEvent } from "@/shared/api/types";
import {
  emptyChannelWindowStore,
  mergeLiveChannelWindowEvent,
  replaceNewestChannelWindow,
  type ChannelWindowStore,
} from "@/features/messages/lib/channelWindowStore";
import {
  CHANNEL_TIMELINE_CONTENT_KINDS,
  KIND_STREAM_MESSAGE,
} from "@/shared/constants/kinds";

type MessageQueryContext = {
  optimisticId: string;
  previousMessages: RelayEvent[];
  previousWindow: ChannelWindowStore | undefined;
  previousThreadReplies: RelayEvent[] | undefined;
  threadRootId: string | null;
  channelId: string;
  queryKey: ReturnType<typeof channelMessagesKey>;
};

const NATIVE_HISTORY_REFRESH_MS = 2_000;

const CHANNEL_TIMELINE_KINDS = new Set<number>(CHANNEL_TIMELINE_CONTENT_KINDS);

export function createOptimisticMessage(
  channelId: string,
  content: string,
  identity: Identity,
  currentMessages: RelayEvent[],
  mentionPubkeys: string[] = [],
  parentEventId: string | null = null,
  mediaTags: string[][] = [],
): RelayEvent {
  const authorAgentId = identity?.agentId;
  if (!authorAgentId) {
    throw new Error("createOptimisticMessage requires the native x0x AgentId.");
  }

  const localKey = `optimistic-${crypto.randomUUID()}`;
  const tags: string[][] = [];

  if (parentEventId) {
    tags.push(
      ...buildReplyTags(
        channelId,
        authorAgentId,
        parentEventId,
        resolveReplyRootId(parentEventId, currentMessages),
        mentionPubkeys,
      ),
    );
  } else {
    tags.push(["h", channelId]);
    tags.push(["p", authorAgentId]);
    for (const pubkey of normalizeMentionPubkeys(
      mentionPubkeys,
      authorAgentId,
    )) {
      tags.push(["p", pubkey]);
    }
  }

  for (const tag of mediaTags) {
    tags.push(tag);
  }

  return {
    id: localKey,
    localKey,
    pubkey: authorAgentId,
    created_at: Math.floor(Date.now() / 1_000),
    kind: KIND_STREAM_MESSAGE,
    tags,
    content,
    sig: "",
    pending: true,
  };
}

/**
 * Resolves the effective target channel for a send operation.
 *
 * When `capturedChannelId` is supplied (non-null), the target is looked up from
 * `channelsCache` — this pins the send to the compose-time channel regardless
 * of any subsequent navigation. If the id is supplied but resolves to nothing,
 * returns `null` (caller should throw — don't silently fall back to the live
 * channel). When `capturedChannelId` is null, the caller didn't capture one and
 * the closed-over `fallbackChannel` is the intended target.
 *
 * Exported for unit testing.
 */
export function resolveEffectiveChannel(
  capturedChannelId: string | null | undefined,
  channelsCache: Channel[] | undefined,
  fallbackChannel: Channel | null,
): Channel | null {
  if (capturedChannelId == null) {
    return fallbackChannel;
  }
  return channelsCache?.find((c) => c.id === capturedChannelId) ?? null;
}

/**
 * Resolves a send target captured as either the channel object itself or its id.
 * A relay-returned channel remains authoritative even when the shared channel
 * list is temporarily stale and does not contain it.
 *
 * Exported for unit testing.
 */
export function resolveSendChannel(
  targetChannel: Channel | undefined,
  capturedChannelId: string | null | undefined,
  channelsCache: Channel[] | undefined,
  fallbackChannel: Channel | null,
): Channel | null {
  return (
    targetChannel ??
    resolveEffectiveChannel(capturedChannelId, channelsCache, fallbackChannel)
  );
}

/**
 * Resolves the thread reply target from a submit-time captured context or,
 * for callers that predate the capture pattern, from live refs.
 *
 * When `threadContext` is supplied (non-null), its values are used exclusively
 * — no live-ref reads occur. This is the race-free path: the context was
 * captured synchronously at submit time before any async awaits.
 *
 * When `threadContext` is null/undefined (legacy callers), falls back to
 * `liveReplyTargetId ?? liveThreadHeadId`.
 *
 * Returns null when no parentEventId can be resolved (caller should bail).
 */
export function resolveThreadReplyTarget(
  threadContext:
    | { parentEventId: string | null; threadHeadId: string | null }
    | null
    | undefined,
  liveReplyTargetId: string | null | undefined,
  liveThreadHeadId: string | null | undefined,
): { parentEventId: string; threadHeadId: string | null } | null {
  if (threadContext != null) {
    // Captured context: use exclusively — no ?? fallback to live refs.
    if (!threadContext.parentEventId) {
      return null;
    }
    return {
      parentEventId: threadContext.parentEventId,
      threadHeadId: threadContext.threadHeadId,
    };
  }
  // Legacy path: read from live refs.
  const parentEventId = liveReplyTargetId ?? liveThreadHeadId ?? null;
  if (!parentEventId) {
    return null;
  }
  return {
    parentEventId,
    threadHeadId: liveThreadHeadId ?? null,
  };
}

export function useChannelWindowQuery(channel: Channel | null) {
  const queryClient = useQueryClient();
  // Subscribe to the resolved scope so the scope-aware window key recomputes
  // (and the store re-renders) when the group's stable historyScope arrives.
  const channelId = channel?.id ?? null;
  useResolvedHistoryScope(channelId);
  const queryKey = channelWindowKey(channelId ?? "none");
  return useQuery({
    enabled: channel !== null && channel.channelType !== "forum",
    queryKey,
    queryFn: () =>
      queryClient.getQueryData<ChannelWindowStore>(queryKey) ??
      emptyChannelWindowStore(),
    staleTime: Number.POSITIVE_INFINITY,
  });
}

export function useChannelMessagesQuery(channel: Channel | null) {
  const queryClient = useQueryClient();
  const channelId = channel?.id ?? null;
  const queryKey = channelMessagesKey(channelId ?? "none");
  const windowKey = channelWindowKey(channelId ?? "none");
  // Groups must not cold-load on the REST id before the live subscription
  // resolves the durable (stable) historyScope; hold until the registry knows
  // it. DMs resolve deterministically and never block.
  const resolvedHistoryScope = useResolvedHistoryScope(channelId);
  const isAwaitingGroupScope =
    channel !== null &&
    channel.channelType !== "forum" &&
    channel.channelType !== "dm" &&
    resolvedHistoryScope === null;

  return useQuery({
    enabled:
      channel !== null &&
      channel.channelType !== "forum" &&
      !isAwaitingGroupScope,
    queryKey,
    queryFn: async () => {
      if (!channel) throw new Error("No channel selected.");
      const previousMessages =
        queryClient.getQueryData<RelayEvent[]>(queryKey) ?? [];
      const page = await fetchNativeChannelWindow(channel);
      const current =
        queryClient.getQueryData<ChannelWindowStore>(windowKey) ??
        emptyChannelWindowStore();
      const next = replaceNewestChannelWindow(current, page);
      queryClient.setQueryData(windowKey, next);
      return reconcileChannelWindowMessages(next, previousMessages);
    },
    staleTime: 5 * 60 * 1_000,
    gcTime: 60 * 60 * 1_000,
    // The daemon's authenticated direct fallback is durable but does not emit
    // a `/ws` topic frame. Refresh the active window so direct-only delivery
    // converges in the open UI without requiring navigation or a reload.
    refetchInterval: NATIVE_HISTORY_REFRESH_MS,
  });
}

export function useChannelSubscription(channel: Channel | null) {
  const queryClient = useQueryClient();
  const channelId = channel?.id ?? null;
  const channelType = channel?.channelType ?? null;

  const appendMessage = useEffectEvent((event: RelayEvent) => {
    if (!channelId || !CHANNEL_TIMELINE_KINDS.has(event.kind)) return;
    const threadReference = getThreadReference(event.tags);
    if (threadReference.parentId != null) {
      if (threadReference.rootId) {
        queryClient.setQueryData<RelayEvent[]>(
          threadRepliesKey(channelId, threadReference.rootId),
          (current = []) => mergeMessages(current, event),
        );
      }
      return;
    }

    const windowKey = channelWindowKey(channelId);
    const current =
      queryClient.getQueryData<ChannelWindowStore>(windowKey) ??
      emptyChannelWindowStore();
    const next = mergeLiveChannelWindowEvent(current, event);
    if (next !== current) {
      queryClient.setQueryData(windowKey, next);
      projectChannelWindowMessages(queryClient, channelId);
    }
  });

  useEffect(() => {
    if (!channel || !channelId || channelType === "forum") return;

    let isDisposed = false;
    let subscription: Awaited<ReturnType<typeof subscribeX0xLive>> | null =
      null;
    let scope: ReturnType<typeof nativeScopeForChannel>;
    try {
      scope = nativeScopeForChannel(channel);
    } catch (error) {
      console.error("Failed to resolve native channel scope", channelId, error);
      return;
    }

    // DM scopes open /ws/direct, which delivers EVERY peer's inbound DMs to
    // this session. Keep only the active conversation's peer (the scope id is
    // the recipient AgentId, already validated by nativeScopeForChannel). Own
    // outbound rows are not echoed here — they persist optimistically and
    // reconcile via the dm:<peer> cold-load keyed by clientId.
    const isDm = channel.channelType === "dm";
    const dmPeer = isDm
      ? scope.slice(scope.indexOf(":") + 1).toLowerCase()
      : null;

    void subscribeX0xLive({ scope, backfill: { limit: 50 } }, (frame) => {
      if (isDisposed) return;
      if (frame.type === "error") {
        console.error(
          "Native x0xd channel stream error",
          channelId,
          frame.message,
        );
        return;
      }
      if (isDm) {
        if (frame.type !== "direct_message") return;
        if (dmPeer && frame.sender.toLowerCase() !== dmPeer) return;
        const event = liveDirectMessageToRelayEvent(frame, channelId);
        if (event) appendMessage(event);
        return;
      }
      if (frame.type !== "message") return;
      const event = liveMessageToRelayEvent(frame, channelId);
      if (event) appendMessage(event);
    })
      .then((opened) => {
        if (isDisposed) {
          void opened.close();
        } else {
          subscription = opened;
          // Capture the daemon-resolved durable history scope (the stable group
          // id, which may differ from the live backfill `scope` above) so the
          // history REST consumers load against the right scope. DMs resolve
          // deterministically and need no registry entry.
          if (!isDm && channelId) {
            setResolvedHistoryScope(channelId, opened.historyScope);
          }
        }
      })
      .catch((error) => {
        if (!isDisposed) {
          console.error(
            "Failed to subscribe to native x0xd channel",
            channelId,
            error,
          );
        }
      });

    return () => {
      isDisposed = true;
      if (subscription) void subscription.close();
    };
  }, [channel, channelId, channelType]);
}

function restoreCachedQuery<T>(
  queryClient: QueryClient,
  queryKey: QueryKey,
  previous: T | undefined,
) {
  if (previous === undefined) {
    queryClient.removeQueries({ queryKey, exact: true });
    return;
  }
  queryClient.setQueryData(queryKey, previous);
}

/** Restore every cache surface changed by an optimistic send. */
export function rollbackOptimisticMessageCache(
  queryClient: QueryClient,
  context: MessageQueryContext,
) {
  queryClient.setQueryData(context.queryKey, context.previousMessages);
  restoreCachedQuery(
    queryClient,
    channelWindowKey(context.channelId),
    context.previousWindow,
  );
  if (context.threadRootId) {
    restoreCachedQuery(
      queryClient,
      threadRepliesKey(context.channelId, context.threadRootId),
      context.previousThreadReplies,
    );
  }
}

export function useSendMessageMutation(
  channel: Channel | null,
  identity: Identity | undefined,
) {
  const queryClient = useQueryClient();

  return useMutation<
    RelayEvent,
    Error,
    {
      channelId?: string;
      targetChannel?: Channel;
      content: string;
      mentionPubkeys?: string[];
      parentEventId?: string | null;
      mediaTags?: string[][];
    },
    MessageQueryContext | undefined
  >({
    mutationFn: async ({
      channelId: capturedChannelId,
      targetChannel,
      content,
      mentionPubkeys,
      parentEventId,
      mediaTags,
    }) => {
      // Prefer a channel captured by the caller at compose time. Otherwise,
      // resolve a captured id from the shared channel cache so navigation
      // cannot redirect the message. Legacy callers without either value use
      // the closed-over `channel`.
      const effectiveChannel = resolveSendChannel(
        targetChannel,
        capturedChannelId,
        queryClient.getQueryData<Channel[]>(channelsQueryKey),
        channel,
      );

      if (effectiveChannel == null) {
        if (capturedChannelId != null) {
          throw new Error("Channel is no longer available.");
        }
        throw new Error("This channel does not support message sending yet.");
      }

      if (effectiveChannel.channelType === "forum") {
        throw new Error("This channel does not support message sending yet.");
      }

      if (!identity) {
        throw new Error("No identity available for sending messages.");
      }

      const { mediaTags: imetaTags, emojiTags } = splitOutgoingTags(mediaTags);
      if (imetaTags.length > 0 || emojiTags.length > 0) {
        throw new Error(NATIVE_RICH_MESSAGE_BLOCKER);
      }

      const recipientAgentIds = messageMentionPubkeys(
        effectiveChannel,
        identity.agentId,
        mentionPubkeys,
      );

      const currentMessages =
        queryClient.getQueryData<RelayEvent[]>(
          channelMessagesKey(effectiveChannel.id),
        ) ?? [];
      const threadParent = parentEventId ?? null;
      const threadRoot = threadParent
        ? (resolveReplyRootId(threadParent, currentMessages) ?? threadParent)
        : null;

      return sendNativeMessage({
        channel: effectiveChannel,
        content,
        identity,
        mentionPubkeys: recipientAgentIds,
        threadRoot,
        threadParent,
      });
    },
    onMutate: async ({
      channelId: capturedChannelId,
      targetChannel,
      content,
      mentionPubkeys,
      parentEventId,
      mediaTags,
    }) => {
      // Mirror mutationFn's target resolution so the optimistic message lands
      // in the cache for the same channel as the real send. A caller-supplied
      // channel remains valid even when a stale channel-list read omitted it.
      const effectiveChannel = resolveSendChannel(
        targetChannel,
        capturedChannelId,
        queryClient.getQueryData<Channel[]>(channelsQueryKey),
        channel,
      );

      if (
        !effectiveChannel ||
        !identity ||
        effectiveChannel.channelType === "forum"
      ) {
        return undefined;
      }

      const queryKey = channelMessagesKey(effectiveChannel.id);
      await queryClient.cancelQueries({ queryKey });

      const previousMessages =
        queryClient.getQueryData<RelayEvent[]>(queryKey) ?? [];
      const windowKey = channelWindowKey(effectiveChannel.id);
      const previousWindow =
        queryClient.getQueryData<ChannelWindowStore>(windowKey);
      const optimisticMessage = createOptimisticMessage(
        effectiveChannel.id,
        content.trim(),
        identity,
        previousMessages,
        mentionPubkeys ?? [],
        parentEventId ?? null,
        mediaTags ?? [],
      );

      const nextWindow = mergeLiveChannelWindowEvent(
        previousWindow ?? emptyChannelWindowStore(),
        optimisticMessage,
      );
      queryClient.setQueryData(windowKey, nextWindow);
      projectChannelWindowMessages(queryClient, effectiveChannel.id);

      const threadReference = getThreadReference(optimisticMessage.tags);
      const threadRootId =
        threadReference.parentId !== null ? threadReference.rootId : null;
      const threadKey = threadRootId
        ? threadRepliesKey(effectiveChannel.id, threadRootId)
        : null;
      const previousThreadReplies = threadKey
        ? queryClient.getQueryData<RelayEvent[]>(threadKey)
        : undefined;
      if (threadKey) {
        queryClient.setQueryData<RelayEvent[]>(threadKey, (current = []) =>
          mergeMessages(current, optimisticMessage),
        );
      }

      return {
        optimisticId: optimisticMessage.id,
        previousMessages,
        previousWindow,
        previousThreadReplies,
        threadRootId,
        channelId: effectiveChannel.id,
        queryKey,
      };
    },
    onError: (error, _variables, context) => {
      // A community timeout surfaces here as the relay's `OK false` reason.
      // Record it so the composer can show the timeout chip and block further
      // sends until it expires; other errors fall through to the caller.
      recordTimeoutFromRejection(error?.message);
      if (!context) {
        return;
      }

      rollbackOptimisticMessageCache(queryClient, context);
    },
    onSuccess: (message, _variables, context) => {
      // An accepted send proves the write-block is lifted; clear any recorded
      // timeout so the chip and disable state fall away immediately.
      clearTimeoutState();
      if (!context) {
        return;
      }

      const windowKey = channelWindowKey(context.channelId);
      const current =
        queryClient.getQueryData<ChannelWindowStore>(windowKey) ??
        emptyChannelWindowStore();
      const withoutPending: ChannelWindowStore = {
        ...current,
        liveOverlay: current.liveOverlay.filter(
          (event) => event.id !== context.optimisticId,
        ),
      };
      const next = mergeLiveChannelWindowEvent(withoutPending, {
        ...message,
        localKey: context.optimisticId,
      });
      queryClient.setQueryData(windowKey, next);
      projectChannelWindowMessages(queryClient, context.channelId);
      if (context.threadRootId) {
        queryClient.setQueryData<RelayEvent[]>(
          threadRepliesKey(context.channelId, context.threadRootId),
          (current = []) =>
            mergeMessages(
              current.filter((event) => event.id !== context.optimisticId),
              { ...message, localKey: context.optimisticId },
            ),
        );
      }
    },
  });
}

export function useToggleReactionMutation() {
  return useMutation<
    void,
    Error,
    {
      eventId: string;
      emoji: string;
      remove: boolean;
    }
  >({
    mutationFn: async () => {
      throw new Error(NATIVE_REACTION_BLOCKER);
    },
  });
}

export function useDeleteMessageMutation(channel: Channel | null) {
  return useMutation<void, Error, { eventId: string }>({
    mutationFn: async () => {
      if (!channel) throw new Error("No channel selected.");
      throw new Error(NATIVE_DELETE_BLOCKER);
    },
    onError: (error) => {
      toast.error(`Failed to delete message: ${error.message}`);
    },
  });
}

export function useEditMessageMutation(channel: Channel | null) {
  return useMutation<
    void,
    Error,
    {
      eventId: string;
      content: string;
      mediaTags?: string[][];
      mentionPubkeys?: string[];
    }
  >({
    mutationFn: async () => {
      if (!channel) throw new Error("No channel selected.");
      throw new Error(NATIVE_EDIT_BLOCKER);
    },
  });
}
