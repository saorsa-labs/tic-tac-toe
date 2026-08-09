/**
 * Tests for the onboarding step-count logic. These are pure-logic tests — no
 * React rendering needed.
 */
import assert from "node:assert/strict";
import test from "node:test";

// Mirrors the activeSteps array in OnboardingFlow.tsx. The relay-scoped flow
// owns only the community profile: profile → avatar.
const ACTIVE_STEPS = ["profile", "avatar"];
const STEP_OFFSET = 1;

/**
 * Mirrors the currentStep derivation in OnboardingFlow.tsx:
 * profile(1) → avatar(2).
 */
function computeCurrentStep(page) {
  const idx = ACTIVE_STEPS.indexOf(page);
  return idx >= 0 ? idx + STEP_OFFSET : STEP_OFFSET;
}

function computeTotalSteps() {
  return ACTIVE_STEPS.length;
}

// ---------------------------------------------------------------------------
// Step count and numbering
// ---------------------------------------------------------------------------

test("totalSteps_is_2", () => {
  assert.equal(computeTotalSteps(), 2);
});

test("currentStep_profile_is_1", () => {
  assert.equal(computeCurrentStep("profile"), 1);
});

test("currentStep_avatar_is_2", () => {
  assert.equal(computeCurrentStep("avatar"), 2);
});

test("currentStep_falls_back_to_1_for_pages_outside_the_step_list", () => {
  assert.equal(computeCurrentStep("membership-denied"), 1);
});

// ---------------------------------------------------------------------------
// Avatar skip button visibility logic
// ---------------------------------------------------------------------------

test("always_skip_shows_skip_button_when_no_error", () => {
  const showAlwaysSkip = true;
  const errorMessage = null;
  const canSkipForNow = false;
  const showSkip = canSkipForNow || (showAlwaysSkip && errorMessage === null);
  assert.equal(showSkip, true);
});

test("always_skip_hides_skip_button_when_error_is_present", () => {
  // On error, the error-recovery buttons take over (canAdvanceWithoutSaving)
  const showAlwaysSkip = true;
  const errorMessage = "Save failed";
  const canSkipForNow = false;
  const showSkip = canSkipForNow || (showAlwaysSkip && errorMessage === null);
  assert.equal(showSkip, false);
});

test("error_recovery_shows_skip_button_regardless_of_always_skip", () => {
  const showAlwaysSkip = false;
  const errorMessage = null;
  const canSkipForNow = true;
  const showSkip = canSkipForNow || (showAlwaysSkip && errorMessage === null);
  assert.equal(showSkip, true);
});

test("skip_button_hidden_when_no_error_and_always_skip_false", () => {
  const showAlwaysSkip = false;
  const errorMessage = null;
  const canSkipForNow = false;
  const showSkip = canSkipForNow || (showAlwaysSkip && errorMessage === null);
  assert.equal(showSkip, false);
});
