import { invokeTauri } from "@/shared/api/tauri";
import type { Identity, RecoveryState } from "@/shared/api/types";

/** Exact 64 lowercase hex chars — the shape of an x0x AgentId. */
const AGENT_ID_PATTERN = /^[0-9a-f]{64}$/;

type RawIdentity = {
  agent_id: string;
  identity_words: string[];
};

/**
 * Validate + map the daemon's identity payload to `{ agentId, identityWords }`.
 * If either field is absent or malformed (daemon down, artifact missing, bad
 * agent id) we FAIL hard rather than fall back to a partial identity. The
 * legacy Nostr relay signer was removed in the M3 identity cutover — the x0x
 * AgentId is now the sole identity on both sides of the IPC boundary.
 */
function fromRawIdentity(raw: RawIdentity): Identity {
  const agentId = raw.agent_id ?? "";
  if (!AGENT_ID_PATTERN.test(agentId)) {
    throw new Error(
      `Identity unavailable: malformed agent id (expected 64-hex, got ${agentId.length} chars).`,
    );
  }
  const identityWords = Array.isArray(raw.identity_words)
    ? raw.identity_words.filter(
        (word) => typeof word === "string" && word.length > 0,
      )
    : [];
  if (identityWords.length === 0) {
    throw new Error("Identity unavailable: missing identity words.");
  }
  return { agentId, identityWords };
}

type RawRecoveryState = {
  lost: boolean;
  locked: boolean;
  reset_failed: boolean;
};

/**
 * Boot-time recovery state — ALWAYS succeeds (no daemon dependency). Call this
 * FIRST; when any flag is true the daemon could not resolve an identity and
 * `getIdentity` must not be called (it fail-closes). Routes the UI to the
 * keyring-lost / locked / reset-failed recovery screen instead.
 */
export async function getRecoveryState(): Promise<RecoveryState> {
  const raw = await invokeTauri<RawRecoveryState>("get_recovery_state");
  return {
    lost: raw.lost === true,
    locked: raw.locked === true,
    resetFailed: raw.reset_failed === true,
  };
}

export async function getIdentity(): Promise<Identity> {
  return fromRawIdentity(await invokeTauri<RawIdentity>("get_identity"));
}

/**
 * Accept loss of the old internal relay signer, persist a fresh replacement,
 * and restart the app so all signer-dependent services initialize coherently.
 * The daemon-owned x0x AgentId is unchanged.
 */
export async function recoverLostIdentity(): Promise<void> {
  await invokeTauri("recover_lost_identity");
}

/**
 * Wipe all local Buzz state (keychain, App Support, WebKit, nest, OAuth cache,
 * CLI symlinks) and relaunch into first-run onboarding.
 *
 * The app restarts after this call completes. Callers should keep the pending
 * state until the process exits and only handle errors (e.g. display a toast).
 */
export async function signOut(): Promise<void> {
  await invokeTauri("sign_out");
}
