import * as React from "react";
import { useQueryClient, type QueryStatus } from "@tanstack/react-query";
import { toast } from "sonner";

import { channelsQueryKey } from "@/features/channels/hooks";
import {
  ensureStarterChannels,
  ensureWelcomeChannel,
  hasEnsuredWelcomeChannel,
  markWelcomeChannelEnsured,
  notifyWelcomeChannelReady,
  rememberPendingWelcomeChannel,
} from "@/features/onboarding/welcome";
import { forceFreshOnboarding } from "@/features/onboarding/devFreshOnboarding";
import { useProfileQuery } from "@/features/profile/hooks";
import { useCommunities } from "@/features/communities/useCommunities";
import { useIdentityQuery, useRecoveryStateQuery } from "@/shared/api/hooks";
import type { Channel } from "@/shared/api/types";
import {
  createChannel,
  deleteChannel,
  ensureStarterChannels as ensureStarterChannelsCommand,
  getChannelMembers,
  getChannels,
  updateChannel,
} from "@/shared/api/tauri";

const STARTER_CHANNEL_SETUP_TOAST_ID = "starter-channel-setup-error";

export type ChannelInitResult =
  | { ok: true; focusChannelId?: string }
  | { ok: false; reason: string; focusChannelId?: string };

export async function initializeStarterChannels(
  queryClient: ReturnType<typeof useQueryClient>,
  {
    focus,
    pubkey,
    communityScope,
  }: {
    focus: boolean;
    pubkey: string | null;
    communityScope: string | null;
  },
): Promise<ChannelInitResult> {
  try {
    let starterChannels: Awaited<
      ReturnType<typeof ensureStarterChannels>
    > | null = null;
    try {
      starterChannels = await ensureStarterChannels({
        ensureStarterChannels: ensureStarterChannelsCommand,
        getChannels,
      });
    } catch (error) {
      console.warn("Failed to initialize public starter channels.", error);
    }

    const welcomeChannel = await ensureWelcomeChannel(
      {
        createChannel,
        deleteChannel,
        getChannelMembers,
        getChannels,
        updateChannel,
      },
      {
        replaceExisting: forceFreshOnboarding,
      },
    );

    const starterChannelList = starterChannels?.channels ?? [];
    queryClient.setQueryData<Channel[]>(channelsQueryKey, (channels = []) => {
      const ensuredIds = new Set(
        starterChannelList.map((channel) => channel.id),
      );
      ensuredIds.add(welcomeChannel.id);
      return [
        ...starterChannelList,
        ...(starterChannelList.some(
          (channel) => channel.id === welcomeChannel.id,
        )
          ? []
          : [welcomeChannel]),
        ...channels.filter((channel) => !ensuredIds.has(channel.id)),
      ];
    });
    // Provisioning is deliberately deferred until Welcome is focused and its
    // native group has been acknowledged by the backend. Seeding here races
    // the route transition and can bind managed children to the prior group.
    markWelcomeChannelEnsured(pubkey, communityScope);
    await queryClient.invalidateQueries({ queryKey: channelsQueryKey });
    if (focus) {
      // Refreshing can briefly replace the optimistic cache with an older relay
      // snapshot. Reinsert the just-ensured channels before announcing focus so
      // the route can consume the pending Welcome channel immediately.
      queryClient.setQueryData<Channel[]>(channelsQueryKey, (channels = []) => {
        const byId = new Map(
          [...channels, ...starterChannelList, welcomeChannel].map(
            (channel) => [channel.id, channel],
          ),
        );
        return [...byId.values()];
      });
      rememberPendingWelcomeChannel(welcomeChannel.id);
      notifyWelcomeChannelReady(welcomeChannel.id);
    }
    const focusChannelId = focus ? welcomeChannel.id : undefined;
    // Native x0x deliberately has no relay-backed starter-channel seeding.
    // A personal Welcome group is sufficient to finish onboarding, so the
    // expected starter-channel rejection must not strand the user on the
    // final setup screen after that group was created successfully.
    return { ok: true, focusChannelId };
  } catch (error) {
    console.warn("Failed to initialize starter channels.", error);
    return {
      ok: false,
      reason:
        error instanceof Error
          ? error.message
          : "Failed to set up starter channels",
    };
  }
}

async function refreshChannelsCache(
  queryClient: ReturnType<typeof useQueryClient>,
) {
  try {
    queryClient.setQueryData(channelsQueryKey, await getChannels());
  } catch {
    // The next mounted channels query can still retry; this cache refresh is
    // only here to avoid a blank Home flash after first-run setup.
  }

  await queryClient.invalidateQueries({ queryKey: channelsQueryKey });
}

const ONBOARDING_COMPLETION_STORAGE_KEY = "buzz-onboarding-complete.v1";
type OnboardingGateStage = "blocking" | "onboarding" | "ready";

type UseFirstRunOnboardingGateOptions = {
  currentAgentId: string | null;
  identityIsFetching: boolean;
  identityLost: boolean;
  identityStatus: QueryStatus;
  isSharedIdentity: boolean;
  profileHasEvent: boolean | undefined;
  profileIsFetching: boolean;
  profileStatus: QueryStatus;
};

type OnboardingGateState = {
  currentAgentId: string | null;
  hasCompletedCurrentAgentId: boolean;
  hasSettledCurrentAgentId: boolean;
  isOpen: boolean;
};

function onboardingCompletionStorageKey(pubkey: string) {
  return `${ONBOARDING_COMPLETION_STORAGE_KEY}:${pubkey}`;
}

function readOnboardingCompletion(pubkey: string | null) {
  if (forceFreshOnboarding) {
    return false;
  }
  if (typeof window === "undefined" || !pubkey) {
    return false;
  }

  return (
    window.localStorage.getItem(onboardingCompletionStorageKey(pubkey)) ===
    "true"
  );
}

function createOnboardingGateState(pubkey: string | null): OnboardingGateState {
  const hasCompletedCurrentAgentId = readOnboardingCompletion(pubkey);

  return {
    currentAgentId: pubkey,
    hasCompletedCurrentAgentId,
    hasSettledCurrentAgentId: hasCompletedCurrentAgentId,
    isOpen: false,
  };
}

function resolveActiveGateState(
  gateState: OnboardingGateState,
  currentAgentId: string | null,
) {
  return gateState.currentAgentId === currentAgentId
    ? gateState
    : createOnboardingGateState(currentAgentId);
}

function updateActiveGateState(
  gateState: OnboardingGateState,
  currentAgentId: string | null,
  update: (activeGateState: OnboardingGateState) => OnboardingGateState,
) {
  return update(resolveActiveGateState(gateState, currentAgentId));
}

function isSettledQueryStatus(status: QueryStatus) {
  return status === "success" || status === "error";
}

function resolveOnboardingGateStage({
  currentAgentId,
  gateState,
  identityIsFetching,
  identityStatus,
}: {
  currentAgentId: string | null;
  gateState: OnboardingGateState;
  identityIsFetching: boolean;
  identityStatus: QueryStatus;
}): OnboardingGateStage {
  const isBlockingCurrentAgentId =
    currentAgentId !== null &&
    !gateState.hasCompletedCurrentAgentId &&
    (gateState.isOpen || !gateState.hasSettledCurrentAgentId);

  if (gateState.isOpen) {
    return "onboarding";
  }

  if (
    identityIsFetching ||
    !isSettledQueryStatus(identityStatus) ||
    isBlockingCurrentAgentId
  ) {
    return "blocking";
  }

  return "ready";
}

export function useFirstRunOnboardingGate({
  currentAgentId,
  identityIsFetching,
  identityLost,
  identityStatus,
  isSharedIdentity,
  profileHasEvent,
  profileIsFetching,
  profileStatus,
}: UseFirstRunOnboardingGateOptions) {
  const [gateState, setGateState] = React.useState<OnboardingGateState>(() =>
    createOnboardingGateState(currentAgentId),
  );
  const activeGateState = resolveActiveGateState(gateState, currentAgentId);
  const { hasCompletedCurrentAgentId, hasSettledCurrentAgentId } =
    activeGateState;

  React.useEffect(() => {
    setGateState((current) =>
      current.currentAgentId === currentAgentId
        ? current
        : createOnboardingGateState(currentAgentId),
    );
  }, [currentAgentId]);

  // When the backend signals "identity lost", force onboarding open
  // immediately so the user can resolve a new identity. This runs once,
  // after identity settles.
  React.useEffect(() => {
    if (!identityLost || !currentAgentId || identityStatus !== "success") {
      return;
    }
    setGateState((current) =>
      updateActiveGateState(current, currentAgentId, (activeGateState) => ({
        ...activeGateState,
        hasCompletedCurrentAgentId: false,
        hasSettledCurrentAgentId: true,
        isOpen: true,
      })),
    );
  }, [currentAgentId, identityLost, identityStatus]);

  React.useEffect(() => {
    // Fast-path: shared identity worktrees have already onboarded in the
    // main checkout. Skip unconditionally without waiting for the relay
    // profile query. Guarded by !hasCompletedCurrentAgentId so it fires once.
    if (
      !forceFreshOnboarding &&
      isSharedIdentity &&
      currentAgentId &&
      identityStatus === "success" &&
      !hasCompletedCurrentAgentId
    ) {
      if (typeof window !== "undefined") {
        window.localStorage.setItem(
          onboardingCompletionStorageKey(currentAgentId),
          "true",
        );
      }
      setGateState((current) =>
        updateActiveGateState(current, currentAgentId, (activeGateState) => ({
          ...activeGateState,
          hasCompletedCurrentAgentId: true,
          hasSettledCurrentAgentId: true,
          isOpen: false,
        })),
      );
      return;
    }

    // Original guard — restored to simple form.
    if (hasSettledCurrentAgentId || !currentAgentId) {
      return;
    }

    if (identityStatus === "error") {
      setGateState((current) =>
        updateActiveGateState(current, currentAgentId, (activeGateState) => ({
          ...activeGateState,
          hasSettledCurrentAgentId: true,
        })),
      );
      return;
    }

    if (identityStatus !== "success") {
      return;
    }

    if (!isSettledQueryStatus(profileStatus) || profileIsFetching) {
      return;
    }

    // If the relay has a real kind:0 metadata event for this pubkey, the user
    // has previously completed onboarding (possibly on another machine or app
    // data directory). Skip the onboarding flow and mark as complete so they
    // go straight to the app.
    //
    // We gate on `hasProfileEvent` — a flag set by the Tauri backend when a
    // real kind:0 event was found — rather than any field value. This correctly
    // handles the case where a returning user's display_name is empty: the event
    // still exists, so onboarding is skipped. A missing event (new user, or no
    // kind:0 on the relay) always shows onboarding regardless of display_name.
    const hasExistingProfile =
      !forceFreshOnboarding &&
      profileStatus === "success" &&
      profileHasEvent === true;

    setGateState((current) =>
      updateActiveGateState(current, currentAgentId, (activeGateState) => {
        // Re-read localStorage here to handle the webkit2gtk WAL race: the
        // synchronous useState initializer may have run before the WAL was
        // merged into the main SQLite file, returning null for a flag that is
        // actually present. By the time this effect fires (identity + profile
        // settled), the WAL has had time to merge and the read is reliable.
        const hasCompletedAfterRecheck =
          readOnboardingCompletion(currentAgentId);
        const alreadyOnboarded =
          activeGateState.hasCompletedCurrentAgentId ||
          hasCompletedAfterRecheck ||
          hasExistingProfile;
        if (alreadyOnboarded && typeof window !== "undefined") {
          window.localStorage.setItem(
            onboardingCompletionStorageKey(currentAgentId),
            "true",
          );
        }
        return {
          ...activeGateState,
          hasCompletedCurrentAgentId: alreadyOnboarded,
          hasSettledCurrentAgentId: true,
          isOpen: !alreadyOnboarded,
        };
      }),
    );
  }, [
    currentAgentId,
    hasCompletedCurrentAgentId,
    hasSettledCurrentAgentId,
    identityStatus,
    isSharedIdentity,
    profileHasEvent,
    profileIsFetching,
    profileStatus,
  ]);

  const skipForNow = React.useCallback(() => {
    setGateState((current) =>
      updateActiveGateState(current, currentAgentId, (activeGateState) => ({
        ...activeGateState,
        hasSettledCurrentAgentId: true,
        isOpen: false,
      })),
    );
  }, [currentAgentId]);

  const complete = React.useCallback(() => {
    if (typeof window !== "undefined" && currentAgentId) {
      window.localStorage.setItem(
        onboardingCompletionStorageKey(currentAgentId),
        "true",
      );
    }

    setGateState({
      currentAgentId,
      hasCompletedCurrentAgentId: true,
      hasSettledCurrentAgentId: true,
      isOpen: false,
    });
  }, [currentAgentId]);

  return {
    complete,
    skipForNow,
    stage: resolveOnboardingGateStage({
      currentAgentId,
      gateState: activeGateState,
      identityIsFetching,
      identityStatus,
    }),
  };
}

export function useAppOnboardingState(isSharedIdentity: boolean) {
  const queryClient = useQueryClient();
  const { activeCommunity } = useCommunities();
  const recoveryStateQuery = useRecoveryStateQuery();
  const recovery = recoveryStateQuery.data;
  const recoveryReady = recoveryStateQuery.status === "success";
  const identityLost = recovery?.lost === true;
  // Keyring unreachable at boot — the real key is still in the OS keyring but
  // the session cannot access it. No in-app recovery is possible; the user
  // must unlock the keyring externally and relaunch. Mutually exclusive with lost.
  const identityLocked = recovery?.locked === true;
  // Boot-time Phase 2 reset failed — wipe was attempted but verification failed.
  // The sentinel is preserved so the next relaunch retries automatically.
  const identityResetFailed = recovery?.resetFailed === true;
  const identityQuery = useIdentityQuery(
    recoveryReady && !identityLost && !identityLocked && !identityResetFailed,
  );
  const identity = identityQuery.data;
  const currentAgentId = identity?.agentId ?? null;
  const starterChannelsCommunityScope = activeCommunity?.groupId ?? null;
  const starterChannelsInitPromisesRef = React.useRef(
    new Map<string, Promise<ChannelInitResult>>(),
  );
  const [isCompletingStarterSetup, setIsCompletingStarterSetup] =
    React.useState(false);

  // Sticky boot fact: once identity was lost at boot, this remains true for the
  // entire session. Per-component state in OnboardingFlow cannot carry this
  // because the flow remounts when pubkey changes after recovery.
  const [bootedLost, setBootedLost] = React.useState(false);
  React.useEffect(() => {
    if (identityLost) setBootedLost(true);
  }, [identityLost]);

  // Sticky boot fact: once identity was locked at boot, this remains true for
  // the entire session. The keyring-locked state clears only on external
  // unlock + relaunch; the relaunchRequired derivation uses this to force the
  // relaunch screen.
  const [bootedLocked, setBootedLocked] = React.useState(false);
  React.useEffect(() => {
    if (identityLocked) setBootedLocked(true);
  }, [identityLocked]);

  const profileQuery = useProfileQuery(
    !identityLost && !identityLocked && identityQuery.status === "success",
  );
  const onboardingGate = useFirstRunOnboardingGate({
    currentAgentId,
    identityIsFetching: identityQuery.fetchStatus === "fetching",
    identityLost,
    identityStatus: identityQuery.status,
    isSharedIdentity,
    profileHasEvent: profileQuery.data?.hasProfileEvent,
    profileIsFetching: profileQuery.fetchStatus === "fetching",
    profileStatus: profileQuery.status,
  });
  const gateComplete = onboardingGate.complete;
  const starterChannelsFocusIntentRef = React.useRef(
    new Map<string, boolean>(),
  );
  const requestStarterChannels = React.useCallback(
    (focus: boolean): Promise<ChannelInitResult> => {
      if (!currentAgentId || !starterChannelsCommunityScope) {
        return Promise.resolve({ ok: true });
      }

      const starterChannelsInitKey = `${starterChannelsCommunityScope}:${currentAgentId}`;
      const currentPromise = starterChannelsInitPromisesRef.current.get(
        starterChannelsInitKey,
      );
      if (currentPromise) {
        // A focus=true request must not be swallowed behind an in-flight
        // focus=false promise. Upgrade the intent: when the background
        // promise resolves, chain a focus-only follow-up.
        if (
          focus &&
          !starterChannelsFocusIntentRef.current.get(starterChannelsInitKey)
        ) {
          starterChannelsFocusIntentRef.current.set(
            starterChannelsInitKey,
            true,
          );
          return currentPromise.then((result) => {
            if (!result.ok) return result;
            return initializeStarterChannels(queryClient, {
              focus: true,
              pubkey: currentAgentId,
              communityScope: starterChannelsCommunityScope,
            });
          });
        }
        return currentPromise;
      }

      if (focus) {
        starterChannelsFocusIntentRef.current.set(starterChannelsInitKey, true);
      }
      const promise = initializeStarterChannels(queryClient, {
        focus,
        pubkey: currentAgentId,
        communityScope: starterChannelsCommunityScope,
      }).finally(() => {
        starterChannelsInitPromisesRef.current.delete(starterChannelsInitKey);
        starterChannelsFocusIntentRef.current.delete(starterChannelsInitKey);
      });
      starterChannelsInitPromisesRef.current.set(
        starterChannelsInitKey,
        promise,
      );
      return promise;
    },
    [currentAgentId, queryClient, starterChannelsCommunityScope],
  );

  React.useEffect(() => {
    if (
      onboardingGate.stage !== "ready" ||
      !currentAgentId ||
      !starterChannelsCommunityScope ||
      !readOnboardingCompletion(currentAgentId) ||
      hasEnsuredWelcomeChannel(currentAgentId, starterChannelsCommunityScope)
    ) {
      return;
    }

    void requestStarterChannels(false);
  }, [
    currentAgentId,
    onboardingGate.stage,
    requestStarterChannels,
    starterChannelsCommunityScope,
  ]);

  const showStarterRetryToast = React.useCallback(
    (reason: string) => {
      toast.error("Couldn't set up starter channels", {
        id: STARTER_CHANNEL_SETUP_TOAST_ID,
        action: {
          label: "Retry",
          onClick: (event) => {
            event.preventDefault();
            void requestStarterChannels(true).then((result) => {
              if (!result.ok) {
                window.setTimeout(
                  // Sonner dismisses an action toast as its click resolves, so
                  // recreate a failed retry after that dismissal completes.
                  () => showStarterRetryToast(result.reason),
                  0,
                );
                return;
              }
              toast.dismiss(STARTER_CHANNEL_SETUP_TOAST_ID);
            });
          },
        },
        description: reason,
      });
    },
    [requestStarterChannels],
  );

  const completeAndShowWelcome = React.useCallback(() => {
    setIsCompletingStarterSetup(true);
    void requestStarterChannels(true).then(async (starterResult) => {
      await refreshChannelsCache(queryClient);
      gateComplete();
      setIsCompletingStarterSetup(false);
      if (starterResult.focusChannelId) {
        window.location.hash = `/channels/${encodeURIComponent(
          starterResult.focusChannelId,
        )}`;
      }
      if (!starterResult.ok) {
        showStarterRetryToast(starterResult.reason);
      }
    });
  }, [
    gateComplete,
    queryClient,
    requestStarterChannels,
    showStarterRetryToast,
  ]);
  const flow = {
    actions: {
      complete: completeAndShowWelcome,
      skipForNow: onboardingGate.skipForNow,
    },
    initialProfile: {
      profile: profileQuery.data,
    },
  };

  // Recovery completed this boot: force a relaunch screen regardless of any
  // other gate state. Backend startup routines (event sync, agent restore,
  // pending-event flush) were skipped for the ephemeral key and cannot restart
  // in-process, so nothing else can proceed until the app restarts.
  const relaunchRequired =
    ((bootedLost && !identityLost) || (bootedLocked && !identityLocked)) &&
    identityQuery.status === "success";

  return {
    currentAgentId,
    flow,
    identityLost,
    // reset-failed is the highest-precedence stage: a failed boot-time reset
    // means identity resolution was skipped entirely. Nothing can proceed until
    // the user relaunches and the wipe retries.
    stage: !recoveryReady
      ? ("blocking" as const)
      : identityResetFailed
        ? ("reset-failed" as const)
        : identityLocked
          ? ("keyring-locked" as const)
          : relaunchRequired
            ? ("relaunch-required" as const)
            : isCompletingStarterSetup
              ? ("blocking" as const)
              : onboardingGate.stage,
  };
}
