import { isTauri } from "@tauri-apps/api/core";
import { emit } from "@tauri-apps/api/event";
import { QueryClientProvider } from "@tanstack/react-query";
import { RouterProvider } from "@tanstack/react-router";
import {
  type ReactNode,
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
} from "react";

import { router } from "@/app/router";
import {
  completeCommunityViewTransition,
  replaceCommunityDestinationRoute,
} from "@/app/communityViewTransition";
import { deriveShellRoute } from "@/app/AppShell.helpers";
import { ThemeGrainientBackground } from "@/app/ThemeGrainientBackground";
import { useReloadShortcut } from "@/app/useReloadShortcut";
import { KnownAgentPubkeysProvider } from "@/features/agents/useKnownAgentPubkeys";
import { useAppOnboardingState } from "@/features/onboarding/hooks";
import { useMachineOnboardingState } from "@/features/onboarding/machineOnboarding";
import {
  type FirstCommunityPage,
  useCommunityOnboarding,
} from "@/features/onboarding/communityOnboarding";
import { CommunityOnboardingFlow } from "@/features/onboarding/ui/CommunityOnboardingFlow";
import {
  MachineOnboardingFlow,
  type MachineOnboardingPage,
} from "@/features/onboarding/ui/MachineOnboardingFlow";
import { OnboardingFlow } from "@/features/onboarding/ui/OnboardingFlow";
import { KeyringLockedScreen } from "@/features/onboarding/ui/KeyringLockedScreen";
import { RelaunchRequiredScreen } from "@/features/onboarding/ui/RelaunchRequiredScreen";
import { ResetFailedScreen } from "@/features/onboarding/ui/ResetFailedScreen";
import { useCommunityInit } from "@/features/communities/useCommunityInit";
import { useNestNotifications } from "@/features/communities/useNestNotifications";
import { useCommunities } from "@/features/communities/useCommunities";
import {
  loadCommunityDestination,
  markPendingCommunityRestore,
  saveCommunityDestination,
} from "@/features/communities/communityNavigationStorage";
import { WelcomeSetup } from "@/features/communities/ui/WelcomeSetup";
import { CommunityApplyErrorScreen } from "@/features/communities/ui/CommunityApplyErrorScreen";
import { CommunityChangeOverlay } from "@/features/communities/ui/CommunityChangeOverlay";
import { createBuzzQueryClient } from "@/shared/api/queryClient";
import { isSharedIdentity as isSharedIdentityCmd } from "@/shared/api/tauri";
import { cn } from "@/shared/lib/cn";
import { StartupWindowDragRegion } from "@/shared/ui/StartupWindowDragRegion";
import { TicTacToeMark } from "@/shared/ui/TicTacToeMark";

const LOADING_TEXT = "Setting up your community...";

// Minimum time the cold-boot splash stays on screen. A real boot resolves the
// community in well under 100ms, and the native window setup plus first paint
// can take longer than that — without a hold, the mark is unmounted before it is
// ever visible. The hold runs as an overlay above the already-mounted app, so
// time-to-interactive is unchanged; only the reveal waits.
const BOOT_SPLASH_MIN_VISIBLE_MS = 1_200;
const BOOT_SPLASH_FADE_MS = 200;
const INITIAL_RENDER_READY_EVENT = "initial-render-ready";

type BootSplashPhase = "holding" | "fading" | "done";

function useInitialRenderReady() {
  useLayoutEffect(() => {
    if (!isTauri()) {
      return;
    }

    void emit(INITIAL_RENDER_READY_EVENT);
  }, []);
}

// E2E runs skip the hold (it would slow every spec's boot and block pointer
// actionability); a spec can opt back in via __BUZZ_E2E__.bootSplashHoldMs.
function bootSplashHoldMs(): number {
  const e2e = (
    window as Window & {
      __BUZZ_E2E__?: { bootSplashHoldMs?: number };
    }
  ).__BUZZ_E2E__;
  if (e2e) {
    return e2e.bootSplashHoldMs ?? 0;
  }
  return BOOT_SPLASH_MIN_VISIBLE_MS;
}

function useBootSplashHold(): BootSplashPhase {
  const [phase, setPhase] = useState<BootSplashPhase>(() =>
    bootSplashHoldMs() > 0 ? "holding" : "done",
  );

  useEffect(() => {
    const holdMs = bootSplashHoldMs();
    if (holdMs <= 0) {
      return;
    }
    const fadeTimer = window.setTimeout(() => setPhase("fading"), holdMs);
    const doneTimer = window.setTimeout(
      () => setPhase("done"),
      holdMs + BOOT_SPLASH_FADE_MS,
    );
    return () => {
      window.clearTimeout(fadeTimer);
      window.clearTimeout(doneTimer);
    };
  }, []);

  return phase;
}

// Product mark for loading gates. It is plain SVG (no JS/SMIL), so it paints on
// the first frame even before scripting starts and avoids a blank hard-reload
// flash.
function ProductLoader({
  ariaLabel,
  className,
  tintClassName = "text-foreground",
}: {
  ariaLabel: string;
  className?: string;
  tintClassName?: string;
}) {
  return (
    <TicTacToeMark
      ariaLabel={ariaLabel}
      className={cn("relative animate-pulse", tintClassName, className)}
    />
  );
}

// Cold boot gate: the theme-adaptive background with the product mark centered.
// The old text remains as an sr-only status caption.
function AppLoadingGate() {
  return (
    <div
      className="buzz-setup-loading-shell flex min-h-dvh flex-col items-center justify-center overflow-hidden px-6 py-10"
      data-testid="app-loading-gate"
      role="status"
    >
      <StartupWindowDragRegion />
      <ThemeGrainientBackground />
      <span className="sr-only">{LOADING_TEXT}</span>
      <TicTacToeMark
        ariaLabel="tic-tac-toe is starting"
        className="relative z-10 h-auto w-28 animate-pulse"
      />
    </div>
  );
}

// Quiet gate for switching between already-set-up communities: visually empty
// unless the switch takes long, so fast switches don't flash the boot splash.
function CommunitySwitchGate() {
  const [showSpinner, setShowSpinner] = useState(false);

  useEffect(() => {
    const timer = window.setTimeout(() => setShowSpinner(true), 300);
    return () => window.clearTimeout(timer);
  }, []);

  return (
    <div
      className="flex min-h-dvh items-center justify-center bg-background"
      data-testid="community-switch-gate"
      role="status"
    >
      <StartupWindowDragRegion />
      <span className="sr-only">Switching community…</span>
      {showSpinner ? (
        <ProductLoader
          ariaLabel="Switching community…"
          className="h-auto w-20"
          tintClassName="text-muted-foreground"
        />
      ) : null}
    </div>
  );
}

function CommunityQueryProvider({ children }: { children: ReactNode }) {
  const [queryClient] = useState(createBuzzQueryClient);

  useEffect(() => {
    const e2eWindow = window as Window & {
      __BUZZ_E2E__?: unknown;
      __BUZZ_E2E_QUERY_CLIENT__?: typeof queryClient;
    };
    if (!e2eWindow.__BUZZ_E2E__) {
      return;
    }

    e2eWindow.__BUZZ_E2E_QUERY_CLIENT__ = queryClient;
    return () => {
      if (e2eWindow.__BUZZ_E2E_QUERY_CLIENT__ === queryClient) {
        delete e2eWindow.__BUZZ_E2E_QUERY_CLIENT__;
      }
    };
  }, [queryClient]);

  return (
    <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>
  );
}

function AppReady({
  isSharedIdentity,
  isCommunitySwitch,
}: {
  isSharedIdentity: boolean;
  isCommunitySwitch: boolean;
}) {
  const onboarding = useAppOnboardingState(isSharedIdentity);

  if (onboarding.stage === "reset-failed") {
    return <ResetFailedScreen />;
  }

  if (onboarding.stage === "keyring-locked") {
    return <KeyringLockedScreen />;
  }

  if (onboarding.stage === "relaunch-required") {
    return <RelaunchRequiredScreen />;
  }

  if (onboarding.stage === "onboarding") {
    return (
      <OnboardingFlow
        actions={onboarding.flow.actions}
        identityLost={onboarding.identityLost}
        initialProfile={onboarding.flow.initialProfile}
        key={onboarding.currentAgentId ?? "anonymous"}
      />
    );
  }

  if (onboarding.stage === "blocking") {
    return isCommunitySwitch ? <CommunitySwitchGate /> : <AppLoadingGate />;
  }

  return (
    <KnownAgentPubkeysProvider>
      <RouterProvider router={router} />
    </KnownAgentPubkeysProvider>
  );
}

function CommunityApp({
  currentAgentId: _currentAgentId,
  onBackToMachineConfig,
  sharedIdentity,
}: {
  currentAgentId: string | null;
  onBackToMachineConfig: () => void;
  sharedIdentity: boolean;
}) {
  const {
    activeCommunity,
    communities,
    reinitKey,
    addCommunity,
    clearCommunities,
    removeCommunity,
    switchCommunity,
    reconnectCommunity,
  } = useCommunities();
  const communityOnboarding = useCommunityOnboarding();
  const connectingTransactionRef = useRef<string | null>(null);
  const [isCommunityChangeOpen, setIsCommunityChangeOpen] = useState(false);
  const [resumeFirstCommunityPage, setResumeFirstCommunityPage] =
    useState<FirstCommunityPage | null>(null);

  // Surface nest-related backend events (repos-dir errors, legacy migration)
  // as toasts. Mounted before useCommunityInit so the listeners are registered
  // ahead of the first apply_workspace call.
  useNestNotifications();

  // Composite key: changes when community ID changes OR when
  // the active community's config is updated (relayUrl/token).
  const communityKey = `${activeCommunity?.id ?? "none"}-${reinitKey}`;

  // Latch once the community key deviates from its cold-boot value: from then
  // on, loading phases are in-app switches and get the quiet gate instead of
  // the full "Setting up your community" splash.
  const initialCommunityKeyRef = useRef(communityKey);
  const hasSwitchedCommunityRef = useRef(false);
  if (communityKey !== initialCommunityKeyRef.current) {
    hasSwitchedCommunityRef.current = true;
  }
  const isCommunitySwitch = hasSwitchedCommunityRef.current;

  const community = useCommunityInit(
    activeCommunity,
    communityKey,
    sharedIdentity,
  );

  const transitionCommunity = useCallback(
    async (targetCommunityId: string) => {
      const activeCommunityId = activeCommunity?.id;
      if (targetCommunityId === activeCommunityId) return;
      if (activeCommunityId) {
        const route = deriveShellRoute(router.state.location.pathname);
        saveCommunityDestination(
          activeCommunityId,
          route.selectedView === "channel" && route.selectedChannelId
            ? { kind: "channel", channelId: route.selectedChannelId }
            : { kind: "home" },
        );
        await router.navigate({ to: "/", replace: true });
        markPendingCommunityRestore(targetCommunityId);
        const destination = loadCommunityDestination(targetCommunityId);
        if (destination?.kind === "channel") {
          replaceCommunityDestinationRoute(
            destination.channelId,
            router.history,
          );
        }
      }
      switchCommunity(targetCommunityId);
    },
    [activeCommunity?.id, switchCommunity],
  );

  const handleCommunityOnboardingConnect = useCallback(async () => {
    const transaction = communityOnboarding.transaction;
    if (transaction?.stage !== "connecting") return;
    if (connectingTransactionRef.current === transaction.id) return;
    connectingTransactionRef.current = transaction.id;
    if (transaction.communityId) {
      await transitionCommunity(transaction.communityId);
      return;
    }
    const previousCommunityId = activeCommunity?.id;
    if (!transaction.groupId) {
      communityOnboarding.update({ error: "Native group id is missing." });
      return;
    }
    const groupAlreadyExists = communities.some(
      (community) => community.groupId === transaction.groupId,
    );
    const id = addCommunity({
      id: transaction.groupId,
      groupId: transaction.groupId,
      name: transaction.communityName,
      reposDir: transaction.reposDir,
      addedAt: new Date().toISOString(),
    });
    communityOnboarding.update({
      communityId: id,
      previousCommunityId,
      addedCommunity: !groupAlreadyExists,
      error: undefined,
    });
    await transitionCommunity(id);
    reconnectCommunity();
  }, [
    activeCommunity?.id,
    addCommunity,
    communities,
    communityOnboarding,
    reconnectCommunity,
    transitionCommunity,
  ]);

  const handleCommunityOnboardingCancel = useCallback(async () => {
    const transaction = communityOnboarding.transaction;
    communityOnboarding.clear();

    if (!transaction?.communityId) return;
    if (!transaction.addedCommunity) {
      if (transaction.previousCommunityId) {
        await transitionCommunity(transaction.previousCommunityId);
      }
      return;
    }
    if (communities.length === 1) {
      if (transaction.source === "first-community") {
        setResumeFirstCommunityPage(transaction.firstCommunityPage ?? "join");
      }
      clearCommunities();
      return;
    }
    if (transaction.previousCommunityId) {
      await transitionCommunity(transaction.previousCommunityId);
    }
    removeCommunity(transaction.communityId);
  }, [
    clearCommunities,
    communities.length,
    communityOnboarding,
    removeCommunity,
    transitionCommunity,
  ]);

  const bootSplashPhase = useBootSplashHold();

  const transaction = communityOnboarding.transaction;
  useEffect(() => {
    if (transaction?.stage !== "connecting") {
      connectingTransactionRef.current = null;
    }
  }, [transaction?.stage]);
  const targetIsReady =
    transaction?.communityId === activeCommunity?.id &&
    community.isReady &&
    community.appliedKey === communityKey;
  useEffect(() => {
    if (transaction?.stage !== "connecting" || !targetIsReady) return;
    communityOnboarding.update(
      { stage: "profile", error: undefined },
      transaction.id,
    );
  }, [communityOnboarding, targetIsReady, transaction?.stage, transaction?.id]);
  // During "entering" the transaction stays alive as a curtain: the app mounts
  // underneath (already pointed at the Welcome channel route) while the
  // onboarding screen covers it, then fades once Welcome reports ready.
  //
  // The flow must keep ONE stable position in the element tree across every
  // stage. Rendering it from a different slot when the stage flips to
  // "entering" would remount it — React state resets and the "Meet your
  // starter team" screen visibly restarts mid-handoff.
  const isEnteringCurtain = transaction?.stage === "entering";

  // The app mounts (and starts loading data) beneath the splash overlay; the
  // overlay just keeps the bee on screen long enough to be seen, then fades.
  // Community switches keep their quiet gate.
  const showBootSplashOverlay =
    bootSplashPhase !== "done" && !isCommunitySwitch;

  let appContent: ReactNode = null;
  if (!transaction) {
    if (community.needsSetup) {
      // Show welcome setup for first-run users with no communities
      appContent = (
        <WelcomeSetup
          initialPage={resumeFirstCommunityPage ?? undefined}
          onBack={onBackToMachineConfig}
        />
      );
    } else if ("error" in community && community.error) {
      // Surface apply failures so the user can retry or change community.
      appContent = (
        <>
          <CommunityApplyErrorScreen
            error={community.error}
            onChangeCommunity={() => setIsCommunityChangeOpen(true)}
            onRetry={reconnectCommunity}
          />
          {isCommunityChangeOpen ? (
            <CommunityChangeOverlay
              onClose={() => setIsCommunityChangeOpen(false)}
            />
          ) : null}
        </>
      );
    }
  }
  // Wait for this exact community config to be applied to the backend before
  // rendering anything that connects to the relay. The appliedKey check avoids
  // a one-render race where React sees the new active community while the
  // Tauri backend is still configured for the previous one.
  const communityApplied =
    community.isReady && community.appliedKey === communityKey;
  useLayoutEffect(() => {
    if (communityApplied) {
      completeCommunityViewTransition();
    }
  }, [communityApplied]);
  if (appContent === null && (!transaction || isEnteringCurtain)) {
    appContent = communityApplied ? (
      <CommunityQueryProvider key={communityKey}>
        <AppReady
          isCommunitySwitch={isCommunitySwitch}
          key={communityKey}
          isSharedIdentity={sharedIdentity}
        />
        {showBootSplashOverlay ? (
          <div
            aria-hidden="true"
            className={cn(
              "fixed inset-0 z-50 transition-opacity",
              bootSplashPhase === "fading" ? "opacity-0" : "opacity-100",
            )}
            data-testid="boot-splash-overlay"
            style={{ transitionDuration: `${BOOT_SPLASH_FADE_MS}ms` }}
          >
            <AppLoadingGate />
          </div>
        ) : null}
      </CommunityQueryProvider>
    ) : isCommunitySwitch ? (
      <CommunitySwitchGate />
    ) : (
      <AppLoadingGate />
    );
  }

  return (
    <>
      {appContent}
      {transaction ? (
        <div
          className={isEnteringCurtain ? "fixed inset-0 z-50" : undefined}
          data-testid={
            isEnteringCurtain ? "onboarding-entering-curtain" : undefined
          }
        >
          <CommunityOnboardingFlow
            onCancel={handleCommunityOnboardingCancel}
            onConnect={handleCommunityOnboardingConnect}
          />
        </div>
      ) : null}
    </>
  );
}

function MachineBootstrap({ sharedIdentity }: { sharedIdentity: boolean }) {
  const machine = useMachineOnboardingState({
    activeCommunityPubkey: undefined,
    isSharedIdentity: sharedIdentity,
  });
  const [machineInitialPage, setMachineInitialPage] =
    useState<MachineOnboardingPage>();

  const reopenMachineConfig = useCallback(() => {
    setMachineInitialPage("config");
    machine.reopen();
  }, [machine.reopen]);

  const completeMachineOnboarding = useCallback(
    (pubkey?: string) => {
      setMachineInitialPage(undefined);
      machine.complete(pubkey);
    },
    [machine.complete],
  );

  if (machine.stage === "reset-failed") return <ResetFailedScreen />;
  if (machine.stage === "keyring-locked") return <KeyringLockedScreen />;
  if (machine.stage === "relaunch-required") return <RelaunchRequiredScreen />;
  if (machine.stage === "blocking") return <AppLoadingGate />;
  if (machine.stage === "ready") {
    return (
      <CommunityApp
        currentAgentId={machine.currentAgentId}
        onBackToMachineConfig={reopenMachineConfig}
        sharedIdentity={sharedIdentity}
      />
    );
  }

  return (
    <MachineOnboardingFlow
      complete={completeMachineOnboarding}
      identityLost={machine.identityLost}
      initialPage={machineInitialPage}
      queryClient={machine.queryClient}
    />
  );
}

export function App() {
  useReloadShortcut();
  useInitialRenderReady();
  const [sharedIdentity, setSharedIdentity] = useState<boolean | null>(null);
  const [queryClient] = useState(createBuzzQueryClient);

  useEffect(() => {
    isSharedIdentityCmd()
      .then(setSharedIdentity)
      .catch((err) => {
        console.warn("is_shared_identity command failed:", err);
        setSharedIdentity(false);
      });
  }, []);

  if (sharedIdentity === null) return <AppLoadingGate />;

  return (
    <QueryClientProvider client={queryClient}>
      <MachineBootstrap sharedIdentity={sharedIdentity} />
    </QueryClientProvider>
  );
}
