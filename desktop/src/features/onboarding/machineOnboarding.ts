import * as React from "react";
import { type QueryStatus, useQueryClient } from "@tanstack/react-query";

import { useIdentityQuery, useRecoveryStateQuery } from "@/shared/api/hooks";

const MACHINE_ONBOARDING_COMPLETION_STORAGE_KEY =
  "buzz-machine-onboarding-complete.v2";
const LEGACY_ONBOARDING_COMPLETION_STORAGE_KEY = "buzz-onboarding-complete.v1";

export type MachineOnboardingStage =
  | "blocking"
  | "keyring-locked"
  | "onboarding"
  | "ready"
  | "relaunch-required"
  | "reset-failed";

function completionKey(prefix: string, pubkey: string) {
  return `${prefix}:${pubkey}`;
}

export function readMachineOnboardingCompletion(pubkey: string | null) {
  if (typeof window === "undefined" || !pubkey) return false;
  return (
    window.localStorage.getItem(
      completionKey(MACHINE_ONBOARDING_COMPLETION_STORAGE_KEY, pubkey),
    ) === "true"
  );
}

function clearMachineOnboardingCompletion(pubkey: string | null) {
  if (typeof window === "undefined" || !pubkey) return;
  window.localStorage.removeItem(
    completionKey(MACHINE_ONBOARDING_COMPLETION_STORAGE_KEY, pubkey),
  );
}

function forceMachineOnboarding() {
  if (!import.meta.env?.DEV || typeof window === "undefined") return false;
  return (
    new URL(window.location.href).searchParams.get("machineOnboarding") === "1"
  );
}

/** @internal Exported for unit testing only. */
export function migrateMachineOnboardingCompletion(
  pubkey: string,
  /**
   * The `pubkey` field of the active community from localStorage, or
   * `undefined` if no community is configured. Community-creation paths stamp
   * the current identity's pubkey on write; absent pubkey (`null`) therefore
   * indicates a legacy entry that pre-dates the stamp and is NOT treated as a
   * voucher. Pass `undefined` when there is no active community at all.
   *
   * Using the community's own pubkey prevents a freshly generated post-reset
   * key from being vouched for by a stale community entry that survived the
   * webview wipe.
   */
  activeCommunityPubkey: string | null | undefined,
  isSharedIdentity: boolean,
) {
  if (forceMachineOnboarding()) return false;
  if (readMachineOnboardingCompletion(pubkey)) return true;

  const completedLegacyOnboarding =
    window.localStorage.getItem(
      completionKey(LEGACY_ONBOARDING_COMPLETION_STORAGE_KEY, pubkey),
    ) === "true";

  // A community entry vouches for the current pubkey only when its recorded
  // pubkey matches. Absent pubkey (legacy entries predating the stamp) and
  // no community at all (undefined) do not vouch — after community creation
  // paths stamp pubkey on write, absent means the entry pre-dates the stamp
  // and cannot be trusted to identify which identity created it.
  const communityVouchesForPubkey =
    activeCommunityPubkey !== undefined &&
    activeCommunityPubkey !== null &&
    activeCommunityPubkey === pubkey;

  if (
    !completedLegacyOnboarding &&
    !communityVouchesForPubkey &&
    !isSharedIdentity
  ) {
    return false;
  }

  window.localStorage.setItem(
    completionKey(MACHINE_ONBOARDING_COMPLETION_STORAGE_KEY, pubkey),
    "true",
  );
  return true;
}

function identitySettled(status: QueryStatus, isFetching: boolean) {
  return !isFetching && (status === "success" || status === "error");
}
export type MachineOnboardingStageInput = {
  identity: { status: QueryStatus; isFetching: boolean };
  recovery: {
    status: QueryStatus;
    isFetching: boolean;
    lost: boolean;
    locked: boolean;
    resetFailed: boolean;
  };
  currentAgentId: string | null;
  hasCompletedCurrentAgentId: boolean;
  evaluatedCurrentAgentId: boolean;
  continuingCurrentAgentId: boolean;
  bootedLost: boolean;
  bootedLocked: boolean;
};

/**
 * Pure stage resolver for the machine-onboarding gate. Extracted from the hook
 * so the recovery-gate decision is unit-testable: an unsettled recovery query
 * (or any recovery flag) must NEVER short-circuit to `ready` when the identity
 * query errors. The one allowed `ready` path is recovery settled to `success`
 * with all flags false.
 */
export function resolveMachineOnboardingStage(
  input: MachineOnboardingStageInput,
): MachineOnboardingStage {
  const recoveryReady = input.recovery.status === "success";
  const { lost, locked, resetFailed } = input.recovery;
  const relaunchRequired =
    ((input.bootedLost && !lost) || (input.bootedLocked && !locked)) &&
    input.identity.status === "success";

  if (!recoveryReady) return "blocking";
  if (resetFailed) return "reset-failed";
  if (locked) return "keyring-locked";
  if (relaunchRequired) return "relaunch-required";
  if (lost) return "onboarding";
  if (input.identity.status === "error") return "ready";
  if (
    !identitySettled(input.identity.status, input.identity.isFetching) ||
    !input.currentAgentId ||
    (!input.hasCompletedCurrentAgentId &&
      !input.evaluatedCurrentAgentId &&
      !input.continuingCurrentAgentId)
  ) {
    return "blocking";
  }
  if (lost || !input.hasCompletedCurrentAgentId) return "onboarding";
  return "ready";
}

export function useMachineOnboardingState({
  activeCommunityPubkey,
  isSharedIdentity,
}: {
  activeCommunityPubkey: string | null | undefined;
  isSharedIdentity: boolean;
}) {
  const queryClient = useQueryClient();
  const recoveryStateQuery = useRecoveryStateQuery();
  const recovery = recoveryStateQuery.data;
  const recoveryReady = recoveryStateQuery.status === "success";
  const identityLost = recovery?.lost === true;
  const identityLocked = recovery?.locked === true;
  const identityResetFailed = recovery?.resetFailed === true;
  const identityQuery = useIdentityQuery(
    recoveryReady && !identityLost && !identityLocked && !identityResetFailed,
  );
  const identity = identityQuery.data;
  const currentAgentId = identity?.agentId ?? null;
  const [completedPubkey, setCompletedPubkey] = React.useState<string | null>(
    () =>
      currentAgentId &&
      !forceMachineOnboarding() &&
      readMachineOnboardingCompletion(currentAgentId)
        ? currentAgentId
        : null,
  );
  const [evaluatedPubkey, setEvaluatedPubkey] = React.useState<string | null>(
    null,
  );
  const continuingPubkeyRef = React.useRef<string | null>(null);
  const startupPubkeyRef = React.useRef<string | null>(null);
  const [bootedLost, setBootedLost] = React.useState(false);
  const [bootedLocked, setBootedLocked] = React.useState(false);

  React.useEffect(() => {
    if (
      identityQuery.status === "success" &&
      startupPubkeyRef.current === null
    ) {
      startupPubkeyRef.current = currentAgentId;
    }
  }, [currentAgentId, identityQuery.status]);
  React.useEffect(() => {
    if (identityLost) setBootedLost(true);
  }, [identityLost]);
  React.useEffect(() => {
    if (identityLocked) setBootedLocked(true);
  }, [identityLocked]);

  React.useEffect(() => {
    if (
      !currentAgentId ||
      currentAgentId !== startupPubkeyRef.current ||
      identityQuery.status !== "success" ||
      identityLost
    ) {
      return;
    }
    if (
      migrateMachineOnboardingCompletion(
        currentAgentId,
        activeCommunityPubkey,
        isSharedIdentity,
      )
    ) {
      setCompletedPubkey(currentAgentId);
    }
    setEvaluatedPubkey(currentAgentId);
  }, [
    currentAgentId,
    activeCommunityPubkey,
    identityLost,
    identityQuery.status,
    isSharedIdentity,
  ]);

  const complete = React.useCallback(
    (completedIdentityPubkey?: string) => {
      const pubkey = completedIdentityPubkey ?? currentAgentId;
      if (!pubkey) return;
      window.localStorage.setItem(
        completionKey(MACHINE_ONBOARDING_COMPLETION_STORAGE_KEY, pubkey),
        "true",
      );
      setCompletedPubkey(pubkey);
    },
    [currentAgentId],
  );

  const continueWithIdentity = React.useCallback((pubkey: string) => {
    continuingPubkeyRef.current = pubkey;
  }, []);

  const reopen = React.useCallback(() => {
    clearMachineOnboardingCompletion(currentAgentId);
    setCompletedPubkey((pubkey) => (pubkey === currentAgentId ? null : pubkey));
    setEvaluatedPubkey(currentAgentId);
  }, [currentAgentId]);

  const hasCompletedCurrentAgentId =
    completedPubkey === currentAgentId ||
    (!forceMachineOnboarding() &&
      readMachineOnboardingCompletion(currentAgentId));

  const stage = resolveMachineOnboardingStage({
    identity: {
      status: identityQuery.status,
      isFetching: identityQuery.fetchStatus === "fetching",
    },
    recovery: {
      status: recoveryStateQuery.status,
      isFetching: recoveryStateQuery.fetchStatus === "fetching",
      lost: identityLost,
      locked: identityLocked,
      resetFailed: identityResetFailed,
    },
    currentAgentId,
    hasCompletedCurrentAgentId,
    evaluatedCurrentAgentId: evaluatedPubkey === currentAgentId,
    continuingCurrentAgentId: continuingPubkeyRef.current === currentAgentId,
    bootedLost,
    bootedLocked,
  });

  return {
    complete,
    continueWithIdentity,
    currentAgentId,
    identityLost,
    queryClient,
    reopen,
    stage,
  };
}
