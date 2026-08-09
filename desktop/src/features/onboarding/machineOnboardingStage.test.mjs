/**
 * Regression tests for the machine-onboarding gate stage.
 *
 * Contract under test (M2 recovery-gate bypass finding):
 *   When the identity query is in the `error` state, the gate may resolve to
 *   `ready` ONLY after the recovery query has settled to `success` with
 *   `lost = false`, `locked = false`, `resetFailed = false`. A pending or
 *   errored recovery query — where the recovery flags are unknown — must NEVER
 *   short-circuit to `ready`, otherwise an unresolved recovery state can be
 *   bypassed straight into the running app.
 *
 * Stage resolution is exercised through the pure `resolveMachineOnboardingStage`
 * export, mirroring the `resolveAgentReadiness` / `resolveWelcomeKickoffStagePhase`
 * pattern used elsewhere in this directory. The bypass was only reachable while
 * that decision lived inline inside the `useMachineOnboardingState` React hook
 * with no testable seam, so a missing pure resolver is itself the
 * regression-enabling condition and fails every case below loudly.
 */
import assert from "node:assert/strict";
import test from "node:test";

import * as MachineOnboarding from "./machineOnboarding.ts";

const PUBKEY =
  "aaaaaa1111112222223333334444445555556666667777778888889999990000aa";

// If the pure resolver is not exported, fail loudly with a contract-level
// message instead of a bare TypeError. This is scaffolding, not an assertion:
// the real assertions are the stage values checked in each case below.
function resolveStage(input) {
  const resolve = MachineOnboarding.resolveMachineOnboardingStage;
  if (typeof resolve !== "function") {
    throw new Error(
      "resolveMachineOnboardingStage is not exported from machineOnboarding.ts. " +
        "The recovery gate cannot be unit-tested until stage resolution is " +
        "extracted into this pure function (M2 recovery bypass finding).",
    );
  }
  return resolve(input);
}

// A fully-satisfied baseline: completion, evaluation, and continuation all
// settled, no relaunch condition, a known pubkey. Only the identity/recovery
// query snapshots vary between cases, so each case isolates the recovery-gate
// decision.
//
//   identity : { status: "pending" | "success" | "error", isFetching: boolean }
//   recovery : { status: "pending" | "success" | "error", isFetching: boolean,
//                lost: boolean, locked: boolean, resetFailed: boolean }
function makeInput(overrides = {}) {
  return {
    identity: { status: "error", isFetching: false },
    recovery: {
      status: "success",
      isFetching: false,
      lost: false,
      locked: false,
      resetFailed: false,
    },
    currentAgentId: PUBKEY,
    hasCompletedCurrentAgentId: true,
    evaluatedCurrentAgentId: true,
    continuingCurrentAgentId: true,
    bootedLost: false,
    bootedLocked: false,
    ...overrides,
  };
}

const identityError = { status: "error", isFetching: false };

// Recovery flags are unknown while the query is unsettled; model them falsy, as
// the gate does when recoveryStateQuery.data is undefined.
const recoveryUnsettled = (status, isFetching) => ({
  status,
  isFetching,
  lost: false,
  locked: false,
  resetFailed: false,
});

// ── Recovery unsettled + identity error: must NEVER reach ready ──────────────
// These three are the M2 bypass. With the bypass present, an unsettled recovery
// query leaves the recovery flags falsy, so the gate falls through to
// `identityQuery.status === "error" -> ready` and opens before recovery is known.

test("resolveMachineOnboardingStage_recovery_pending_fetching_with_identity_error_never_ready", () => {
  const stage = resolveStage(
    makeInput({
      identity: identityError,
      recovery: recoveryUnsettled("pending", true),
    }),
  );
  assert.notEqual(
    stage,
    "ready",
    "a recovery query that is still fetching must block the gate even when the " +
      "identity query errors; an unknown recovery state cannot short-circuit to ready",
  );
});

test("resolveMachineOnboardingStage_recovery_pending_idle_with_identity_error_never_ready", () => {
  const stage = resolveStage(
    makeInput({
      identity: identityError,
      recovery: recoveryUnsettled("pending", false),
    }),
  );
  assert.notEqual(
    stage,
    "ready",
    "a pending (not-yet-fetching) recovery query must block the gate even when " +
      "the identity query errors",
  );
});

test("resolveMachineOnboardingStage_recovery_error_with_identity_error_never_ready", () => {
  const stage = resolveStage(
    makeInput({
      identity: identityError,
      recovery: recoveryUnsettled("error", false),
    }),
  );
  assert.notEqual(
    stage,
    "ready",
    "an errored recovery query must block the gate even when the identity query " +
      "errors; a failed recovery read cannot be treated as a clean bill of health",
  );
});

// ── Recovery success but a flag set: identity-error must still not be ready ──
// Pins the "only after lost=false, locked=false, reset_failed=false" half of the
// contract, so a future branch reorder cannot slide a recovery flag past the
// ready guard. Mirrors the Rust-side signing_keys fail-closed gate.

test("resolveMachineOnboardingStage_recovery_success_lost_with_identity_error_never_ready", () => {
  const stage = resolveStage(
    makeInput({
      identity: identityError,
      recovery: {
        status: "success",
        isFetching: false,
        lost: true,
        locked: false,
        resetFailed: false,
      },
    }),
  );
  assert.notEqual(
    stage,
    "ready",
    "recovery success with lost=true must route to recovery, never ready",
  );
});

test("resolveMachineOnboardingStage_recovery_success_locked_with_identity_error_never_ready", () => {
  const stage = resolveStage(
    makeInput({
      identity: identityError,
      recovery: {
        status: "success",
        isFetching: false,
        lost: false,
        locked: true,
        resetFailed: false,
      },
    }),
  );
  assert.notEqual(
    stage,
    "ready",
    "recovery success with locked=true must route to recovery, never ready",
  );
});

test("resolveMachineOnboardingStage_recovery_success_reset_failed_with_identity_error_never_ready", () => {
  const stage = resolveStage(
    makeInput({
      identity: identityError,
      recovery: {
        status: "success",
        isFetching: false,
        lost: false,
        locked: false,
        resetFailed: true,
      },
    }),
  );
  assert.notEqual(
    stage,
    "ready",
    "recovery success with resetFailed=true must route to recovery, never ready " +
      "(mirrors the Rust signing_keys fail-closed gate for a failed wipe)",
  );
});

// ── The one allowed ready path: recovery success AND fully clean ─────────────

test("resolveMachineOnboardingStage_identity_error_yields_ready_only_when_recovery_success_and_clean", () => {
  const stage = resolveStage(
    makeInput({
      identity: identityError,
      recovery: {
        status: "success",
        isFetching: false,
        lost: false,
        locked: false,
        resetFailed: false,
      },
    }),
  );
  assert.equal(
    stage,
    "ready",
    "an identity error may reach ready only after recovery is success with " +
      "lost=false, locked=false, resetFailed=false; gating harder would block a " +
      "legitimate boot whose recovery is known-clean",
  );
});
