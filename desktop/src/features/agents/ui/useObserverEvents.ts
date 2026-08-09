import * as React from "react";

import {
  ensureRelayObserverSubscription,
  fetchOlderAgentArchived,
  getAgentObserverSnapshot,
  getAgentTranscript,
  getArchivedChannelEvents,
  subscribeAgentObserverStore,
} from "@/features/agents/observerRelayStore";
import type { ObserverEvent, TranscriptItem } from "./agentSessionTypes";

// Stable subscribe reference shared by all useSyncExternalStore hooks.
// subscribeAgentObserverStore already has a fixed identity, so this thin
// wrapper satisfies React's requirement without per-hook useCallback.
const subscribeToStore = (onStoreChange: () => void) =>
  subscribeAgentObserverStore(onStoreChange);

export function useObserverEvents(
  enabled: boolean,
  agentPubkey?: string | null,
) {
  const getSnapshot = React.useCallback(
    () => getAgentObserverSnapshot(agentPubkey, enabled),
    [agentPubkey, enabled],
  );

  const snapshot = React.useSyncExternalStore(subscribeToStore, getSnapshot);

  React.useEffect(() => {
    if (enabled && agentPubkey) {
      void ensureRelayObserverSubscription();
    }
  }, [enabled, agentPubkey]);

  return snapshot;
}

export function useAgentTranscript(
  enabled: boolean,
  agentPubkey?: string | null,
): TranscriptItem[] {
  const getSnapshot = React.useCallback(
    () => getAgentTranscript(agentPubkey, enabled),
    [agentPubkey, enabled],
  );

  return React.useSyncExternalStore(subscribeToStore, getSnapshot);
}

/**
 * Reactively read the channel-scoped archive raw events for a given
 * (agent, channel) pair. Returns an empty array until archive pages are loaded.
 *
 * Subscribes to `subscribeAgentObserverStore` so it re-renders whenever the
 * cold-load replay writes new pages to the archive window — the same
 * subscription used by the live event snapshot, keeping both in sync.
 *
 * UI consumers merge these events with the live event window and call
 * `buildTranscriptState()` once over the combined sorted/deduplicated set,
 * so stateful transcript relationships (tool start/update, plan replacement,
 * permission request/response) are never split across two independent state
 * machines.
 */
export function useArchivedChannelEvents(
  agentPubkey: string | null | undefined,
  channelId: string | null | undefined,
): ObserverEvent[] {
  const getSnapshot = React.useCallback(
    () => getArchivedChannelEvents(agentPubkey, channelId),
    [agentPubkey, channelId],
  );

  return React.useSyncExternalStore(subscribeToStore, getSnapshot);
}

// Number of cold-load pages to fetch eagerly on panel open (before any scroll).
// Each page advances the per-child rowid cursor; 10 pages covers agent turns
// that emit hundreds of frames (e.g. a full code-review turn ~900).
const INITIAL_HYDRATION_BUDGET_PAGES = 10;

/**
 * Load-older-on-scroll for archived observer frames, scoped to one channel.
 *
 * Native model: the agent's observer telemetry is durably stored as x0x direct
 * messages in the child daemon's `dm:<child>` history. This hook drives a
 * per-child cold-load (via `fetchOlderAgentArchived`) that pages that history,
 * decodes + owner/child-auth-validates each row, and ingests only frames whose
 * `channelId` matches — so cross-channel contamination is impossible. No relay,
 * no decrypt.
 *
 * `agentPubkey` resolves the dedicated child identity; without it (or without a
 * `channelId`) the hook degrades to `hasOlderArchived: false` with no calls.
 */
export function useLoadArchivedObserverEvents(
  enabled: boolean,
  channelId: string | null,
  agentPubkey?: string | null,
) {
  const [hasOlderArchived, setHasOlderArchived] = React.useState(false);
  // Single-flight lock shared by the eager hydration loop and scroll-triggered
  // fetches: the per-child cursor lives in the store, so two concurrent cold
  // loads would race on it. The lock is only held across the awaited load.
  const fetchLockRef = React.useRef(false);

  const fetchOlderArchived = React.useCallback(async () => {
    if (!enabled || !channelId || !agentPubkey || fetchLockRef.current) {
      return;
    }
    fetchLockRef.current = true;
    try {
      const more = await fetchOlderAgentArchived(agentPubkey, channelId);
      setHasOlderArchived(more);
    } catch (error) {
      console.error("[useLoadArchivedObserverEvents] fetch failed:", error);
    } finally {
      fetchLockRef.current = false;
    }
  }, [enabled, channelId, agentPubkey]);

  // Eager initial hydration: on panel open or channel/agent change, load pages
  // until the budget is reached or the child's history is exhausted, so
  // archived history is visible immediately without scrolling.
  React.useEffect(() => {
    if (!enabled || !channelId || !agentPubkey) {
      setHasOlderArchived(false);
      return;
    }
    let cancelled = false;
    setHasOlderArchived(true);
    void (async () => {
      try {
        for (let page = 0; page < INITIAL_HYDRATION_BUDGET_PAGES; page++) {
          if (cancelled) return;
          fetchLockRef.current = true;
          const more = await fetchOlderAgentArchived(agentPubkey, channelId);
          fetchLockRef.current = false;
          if (cancelled) return;
          if (!more) {
            setHasOlderArchived(false);
            return;
          }
        }
      } catch (error) {
        console.error(
          "[useLoadArchivedObserverEvents] hydration failed:",
          error,
        );
      } finally {
        fetchLockRef.current = false;
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [enabled, channelId, agentPubkey]);

  return { fetchOlderArchived, hasOlderArchived };
}
