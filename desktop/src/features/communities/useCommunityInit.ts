import { useEffect, useRef, useState } from "react";

import { clearSearchHitEventCache } from "@/app/navigation/searchHitEventCache";
import { clearAllDrafts } from "@/features/messages/lib/useDrafts";
import {
  resetActiveAgentTurnsStore,
  saveActiveAgentTurnsForCommunity,
  restoreActiveAgentTurnsForCommunity,
} from "@/features/agents/activeAgentTurnsStore";
import { resetAgentWorkingSignal } from "@/features/agents/agentWorkingSignal";
import { resetAgentObserverStore } from "@/features/agents/observerRelayStore";
import { resetAvatarPresentations } from "@/features/profile/avatarPresentationStore";
import { clearMarkdownNodeCache } from "@/shared/ui/markdown/nodeCache";
import { resetVideoPlayerState } from "@/shared/ui/videoPlayerState";

import type { Community } from "./types";
import { bindNativeGroup } from "./nativeCommunityApi";

/**
 * Tear down all community-scoped module singletons so the new
 * community starts with a clean slate. Hook-managed subscriptions are
 * torn down via their own effect cleanup and do not need entries here.
 * See AGENTS.md "Community Switching" for the full contract.
 */
function resetCommunityState({
  resetAvatarState,
}: {
  resetAvatarState: boolean;
}): void {
  clearAllDrafts();
  resetAgentObserverStore();
  resetActiveAgentTurnsStore();
  resetAgentWorkingSignal();
  if (resetAvatarState) {
    resetAvatarPresentations();
  }
  resetVideoPlayerState();
  clearSearchHitEventCache();
  clearMarkdownNodeCache();
}

type CommunityInitResult =
  | { isReady: true; needsSetup: false; appliedKey: string }
  | {
      isReady: false;
      needsSetup: true;
      defaultRelayUrl?: string;
    }
  | { isReady: false; needsSetup: false; appliedKey: string | null }
  | { isReady: false; needsSetup: false; appliedKey: null; error: string };

/**
 * Applies the active community config to the Tauri backend and resets
 * all community-scoped module singletons when the community changes.
 *
 * Returns a discriminated union — only render the app after the
 * community is applied. When `needsSetup` is true, the caller
 * should show a first-run welcome screen.
 */
export function useCommunityInit(
  activeCommunity: Community | null,
  communityKey: string,
  isSharedIdentity: boolean,
): CommunityInitResult {
  const [result, setResult] = useState<CommunityInitResult>({
    isReady: false,
    needsSetup: false,
    appliedKey: null,
  });

  // Track whether this is the initial mount or a community switch.
  // On the initial mount we skip resetting singletons (they're fresh).
  const hasInitializedRef = useRef(false);

  // Track the previously-applied community ID so we can save its turn state
  // before resetting when the user switches to a different community.
  const prevCommunityIdRef = useRef<string | null>(null);
  // Track the daemon group whose local presentation state is active.
  const appliedGroupIdRef = useRef<string | null>(null);

  // biome-ignore lint/correctness/useExhaustiveDependencies: name-only changes do not rebind the daemon group
  useEffect(() => {
    let cancelled = false;

    async function init() {
      if (!activeCommunity) {
        // M3 cutover: the packaged app has no default relay URL to resolve.
        // The user adds a native x0x community through the setup dialog.
        if (!cancelled) {
          setResult({
            isReady: false,
            needsSetup: true,
          });
        }
        return;
      }

      // Mark this community config as pending while it is applied to the
      // backend. App.tsx also checks appliedKey against the active communityKey,
      // which prevents rendering community-scoped UI for a new community until
      // that exact config has finished applying.
      setResult({
        isReady: false,
        needsSetup: false,
        appliedKey: communityKey,
      });

      // On community switch (not initial mount), reset module singletons
      // so the new tree starts with a clean slate.
      if (hasInitializedRef.current) {
        // Save the outgoing community's turn state before wiping the store so
        // timers survive a round-trip (A → B → A keeps A's elapsed time).
        if (prevCommunityIdRef.current) {
          saveActiveAgentTurnsForCommunity(prevCommunityIdRef.current);
          // Null out immediately so a rapid community switch (A→B→C before
          // B's applyCommunity resolves) doesn't re-save the now-empty
          // store under the outgoing community ID and delete its snapshot.
          prevCommunityIdRef.current = null;
        }
        resetCommunityState({
          resetAvatarState:
            appliedGroupIdRef.current !== activeCommunity.groupId,
        });
      }
      hasInitializedRef.current = true;
      appliedGroupIdRef.current = activeCommunity.groupId;

      try {
        await bindNativeGroup(activeCommunity.groupId);
      } catch (error) {
        if (!cancelled) {
          setResult({
            isReady: false,
            needsSetup: false,
            appliedKey: null,
            error:
              error instanceof Error
                ? error.message
                : "Failed to activate native workspace",
          });
        }
        return;
      }
      if (!cancelled) {
        restoreActiveAgentTurnsForCommunity(activeCommunity.id);
        prevCommunityIdRef.current = activeCommunity.id;
        setResult({
          isReady: true,
          needsSetup: false,
          appliedKey: communityKey,
        });
      }
      return;
    }

    void init();

    return () => {
      cancelled = true;
    };
  }, [
    activeCommunity?.id,
    activeCommunity?.groupId,
    activeCommunity?.reposDir,
    isSharedIdentity,
    communityKey,
  ]);

  return result;
}
