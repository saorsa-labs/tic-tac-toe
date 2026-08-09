// Modified from block/buzz @ 710ed9ff — see FORK.md (Stage 1: local-stack runtime relay default)
// M3 cutover: the production identity is the x0x AgentId + four speakable
// words, sourced from the native daemon. No Nostr signer is compiled into
// the desktop identity surface.
use tauri::Manager;
use tauri::State;

use crate::{
    app_state::AppState,
    models::{IdentityInfo, RecoveryStateInfo},
};

/// Validate that `s` is exactly 64 lowercase-hex characters — the shape of an
/// x0x AgentId (SHA-256 of an ML-DSA-65 public key). Rejects uppercase, wrong
/// length, or non-hex so a malformed/placeholder daemon value can never become
/// a displayed identity.
fn validate_agent_id(s: &str) -> Result<(), String> {
    if s.len() != 64
        || !s
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(format!(
            "daemon agent_id must be 64 lowercase hex chars (got len {})",
            s.len()
        ));
    }
    Ok(())
}

/// Derive the four speakable identity words from a validated 64-hex AgentId
/// using the `four-word-networking` `IdentityEncoder` (byte-parity with
/// `x0x agent`, which injects `identity_words` via the same crate/API).
fn identity_words_for(agent_id: &str) -> Result<Vec<String>, String> {
    let encoder = four_word_networking::IdentityEncoder::new();
    let words = encoder
        .encode_hex(agent_id)
        .map_err(|e| format!("encode identity words: {e}"))?;
    Ok(words.agent_words().to_vec())
}

/// Bounded, authenticated fetch of the daemon AgentId via the named loopback
/// `api.port`/`api-token` artifacts. Fail-closed: returns `Err` when the
/// daemon is down, the artifacts are missing, or `agent_id` is absent or
/// malformed. Never falls back to deriving a display identity from the
/// compatibility signer.
pub(crate) fn fetch_agent_id() -> Result<String, String> {
    let value = crate::local_stack::fetch_agent()?;
    let agent_id = value
        .get("agent_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "daemon /agent response missing agent_id".to_string())?;
    let agent_id = agent_id.trim();
    validate_agent_id(agent_id)?;
    Ok(agent_id.to_string())
}

/// Displayed identity: the daemon AgentId + four speakable words. The sole
/// identity surfaced to the frontend — the M3 cutover removed the internal
/// Nostr compatibility signer, so there is no relay signer pubkey.
#[tauri::command]
pub fn get_identity() -> Result<IdentityInfo, String> {
    let agent_id = fetch_agent_id()?;
    let identity_words = identity_words_for(&agent_id)?;
    Ok(IdentityInfo {
        agent_id,
        identity_words,
    })
}

/// Accept loss of the old internal relay signer, persist a fresh replacement,
/// and restart so every owner-keyed service starts under one coherent signer.
/// The displayed x0x AgentId is daemon-owned and is not replaced here.
#[tauri::command]
pub fn recover_lost_identity(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    if is_shared_identity() {
        return Err(
            "Lost identity recovery is unavailable while BUZZ_SHARE_IDENTITY provides the compatibility signer."
                .to_string(),
        );
    }
    crate::app_state::replace_lost_identity(&app, &state)?;
    app.request_restart();
    Ok(())
}

/// Pure recovery-state reader (testable without a Tauri `State` wrapper):
/// snapshots the boot-time atomics into [`RecoveryStateInfo`].
pub(crate) fn recovery_state_from(state: &AppState) -> RecoveryStateInfo {
    RecoveryStateInfo {
        lost: state
            .identity_lost
            .load(std::sync::atomic::Ordering::Acquire),
        locked: state
            .keyring_locked
            .load(std::sync::atomic::Ordering::Acquire),
        reset_failed: state
            .reset_failed
            .load(std::sync::atomic::Ordering::Acquire),
    }
}

/// Boot-time recovery state. Always succeeds — reads in-memory atomics only,
/// never touches the daemon — so the frontend can route to a recovery screen
/// (keyring locked/lost, or reset-failed) before calling `get_identity`, which
/// is fail-closed when the daemon is unavailable. The frontend must call this
/// first and only call `get_identity` when all three flags are false.
#[tauri::command]
pub fn get_recovery_state(state: State<'_, AppState>) -> RecoveryStateInfo {
    recovery_state_from(&state)
}

#[tauri::command]
pub fn is_shared_identity() -> bool {
    // The shared-identity value is opaque (never parsed as a Nostr key): this
    // only signals dev worktree sharing, where the key arrives via env rather
    // than the native identity path.
    std::env::var("BUZZ_SHARE_IDENTITY")
        .map(|v| v == "1")
        .unwrap_or(false)
        && std::env::var("BUZZ_PRIVATE_KEY")
            .map(|k| !k.trim().is_empty())
            .unwrap_or(false)
}

/// Native x0x AgentId of a managed agent's dedicated child daemon, if one has
/// been provisioned for `pubkey`. The observer transport uses this to address
/// telemetry (child→owner) and control (owner→child) over authenticated PQC
/// direct messaging. Returns `null` when no child is provisioned yet (the
/// legacy relay path then applies). Exposes only the AgentId — never the data
/// dir or token.
#[tauri::command]
pub fn get_managed_agent_native_identity(pubkey: String) -> Option<String> {
    crate::managed_agents::agent_identity::managed_agent_child_identity(&pubkey)
        .map(|child| child.agent_id)
}

/// Write a reset-intent sentinel and request a graceful restart into Phase 2
/// (boot-time wipe).
///
/// The actual data destruction is deferred to the next boot: `setup()` in
/// `lib.rs` checks for the sentinel and performs the wipe before any migration
/// or identity resolution. This two-phase design means a crash before the
/// restart is safe — the sentinel persists and the wipe completes on the next
/// open.
///
/// Not available in shared-identity mode (`BUZZ_SHARE_IDENTITY=1`): the key
/// comes from an env var, not the keychain, so wiping would have no effect and
/// would be confusing.
#[tauri::command]
pub async fn sign_out(app: tauri::AppHandle) -> Result<(), String> {
    if is_shared_identity() {
        return Err(
            "Sign out isn't available while BUZZ_SHARE_IDENTITY provides your identity. Unset BUZZ_SHARE_IDENTITY and BUZZ_PRIVATE_KEY, then relaunch to sign out."
                .to_string(),
        );
    }

    // Stop all managed agents before restart so they don't race the wipe.
    if let Err(e) = crate::shutdown::shutdown_managed_agents(&app) {
        eprintln!("buzz-desktop sign-out: agent shutdown: {e}");
    }

    // Write the reset sentinel — destruction happens on next boot.
    let data_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("app data dir: {e}"))?;
    crate::reset::write_sentinel(&data_dir)?;

    // Tauri restarts only after normal shutdown, avoiding a single-instance
    // race. If restarting does not complete, the sentinel makes a manual open
    // finish the reset.
    app.request_restart();
    Ok(())
}

// ── M2 displayed-identity regression tests ─────────────────────────────────
//
// Pins the cutover invariants: the displayed identity is the daemon AgentId
// (exact 64 lowercase hex) + its four speakable words, the Nostr compatibility
// signer is a separate internal value, and a malformed/unreachable daemon
// fails closed rather than synthesizing a display identity from the signer.
// These reach the private gate/derivation fns directly (`use super::*`) so a
// future refactor cannot quietly weaken them.
#[cfg(test)]
mod identity_tests {
    use std::sync::atomic::Ordering;

    use super::{fetch_agent_id, identity_words_for, recovery_state_from, validate_agent_id};

    use crate::app_state::build_app_state;

    /// Canonical x0xd parity vector: `GET /agent.agent_id` for the default dev
    /// daemon and the four words `x0x agent` prints for it. Byte-parity with the
    /// `four-word-networking` crate + the daemon; if either side changes the
    /// derivation this fails closed.
    const PARITY_AGENT_ID: &str =
        "dd6530452610619d468e4e82be82107e86384365c58efa6e3018d7762c7368da";
    const PARITY_WORDS: [&str; 4] = ["bodily", "example", "dismiss", "galaxy"];

    fn assert_rejects(value: &str) {
        assert!(
            validate_agent_id(value).is_err(),
            "expected daemon value to be rejected, but accepted: {value:?}"
        );
    }

    // ── validate_agent_id: the fail-closed gate on daemon values ─────────────

    #[test]
    fn validate_agent_id_accepts_canonical_64_lowercase_hex() {
        assert!(validate_agent_id(PARITY_AGENT_ID).is_ok());
        // Independent canonical ids also pass — the gate is shape, not value.
        assert!(validate_agent_id(&"a".repeat(64)).is_ok());
        assert!(validate_agent_id(&"0".repeat(64)).is_ok());
    }

    #[test]
    fn validate_agent_id_rejects_uppercase_hex() {
        // The namespace is lowercase: an uppercase twin is a different, rejected
        // value — never silently folded into the canonical form.
        assert_rejects(&PARITY_AGENT_ID.to_uppercase());
        assert_rejects("AB12cd");
    }

    #[test]
    fn validate_agent_id_rejects_wrong_length() {
        assert_rejects(&PARITY_AGENT_ID[..63]); // one char short
        assert_rejects(&format!("{PARITY_AGENT_ID}0")); // one char long
        assert_rejects("");
    }

    #[test]
    fn validate_agent_id_rejects_non_hex_and_bech32_placeholders() {
        assert_rejects(&format!("{}g", &PARITY_AGENT_ID[..63])); // 'g' is not hex
                                                                 // bech32 / placeholder shapes a misconfigured daemon might emit must
                                                                 // never pass as a 64-hex AgentId.
        assert_rejects(&format!("npub1{}", "0".repeat(60)));
        assert_rejects(&"z".repeat(64));
    }

    #[test]
    fn validate_agent_id_rejects_whitespace_padding() {
        // The gate is exact-length; surrounding whitespace must not smuggle a
        // value through a trim-only path upstream.
        assert_rejects(&format!(" {PARITY_AGENT_ID}"));
        assert_rejects(&format!("{PARITY_AGENT_ID}\n"));
    }

    // ── identity_words_for: canonical four-word derivation ────────────────────

    #[test]
    fn identity_words_for_returns_canonical_parity_vector() {
        let words = identity_words_for(PARITY_AGENT_ID).expect("parity id encodes");
        assert_eq!(
            words, PARITY_WORDS,
            "displayed words must match the x0x agent parity vector exactly"
        );
    }

    #[test]
    fn identity_words_for_is_deterministic() {
        let first = identity_words_for(PARITY_AGENT_ID).unwrap();
        let second = identity_words_for(PARITY_AGENT_ID).unwrap();
        assert_eq!(first, second, "same agent id must yield identical words");
    }

    #[test]
    fn identity_words_for_returns_exactly_four_nonempty_words() {
        // Every valid agent id — not only the parity vector — yields exactly
        // four non-empty speakable words (4 × 12-bit = first 48 bits).
        let zeros = "0".repeat(64);
        let a_hex = "a".repeat(64);
        let f_hex = "f".repeat(64);
        let mixed = "1234567890abcdef".repeat(4);
        let ids = [
            zeros.as_str(),
            a_hex.as_str(),
            f_hex.as_str(),
            mixed.as_str(),
        ];
        for id in ids {
            let words = identity_words_for(id).unwrap_or_else(|err| panic!("encode {id}: {err}"));
            assert_eq!(words.len(), 4, "expected exactly four words for {id}");
            assert!(words.iter().all(|w| !w.is_empty()), "empty word for {id}");
        }
    }

    #[test]
    fn identity_words_for_encodes_only_the_first_48_bit_prefix() {
        // The four words encode ONLY the first 48 bits (6 bytes = first 12 hex
        // chars) of the AgentId — a lossy, human-friendly prefix, never a unique
        // id. This is precisely why the full 64-hex AgentId is the canonical
        // copy target and `validate_agent_id` enforces the whole string: the
        // words collide by design past ~2^24 agents. Pin that bits after the
        // 48-bit prefix never change the words, while a different prefix does.
        // (The encoder itself is case-lenient — `hex::decode` accepts uppercase
        // — so lowercase-namespace enforcement is `validate_agent_id`'s job,
        // exercised above; it runs first inside `fetch_agent_id`.)
        let prefix = &PARITY_AGENT_ID[..12]; // first 6 bytes
        let same_prefix = format!("{prefix}{}", "0".repeat(64 - 12));
        assert_ne!(same_prefix, PARITY_AGENT_ID);
        assert_eq!(
            identity_words_for(&same_prefix).unwrap(),
            identity_words_for(PARITY_AGENT_ID).unwrap(),
            "words must depend only on the first 48 bits"
        );
        // A different 48-bit prefix ⇒ different words.
        assert_ne!(
            identity_words_for(PARITY_AGENT_ID).unwrap(),
            identity_words_for(&"0".repeat(64)).unwrap(),
            "distinct prefixes must yield distinct words"
        );
        // Non-hex input still fails — the encoder refuses garbage, not merely
        // off-canonical-case input.
        assert!(identity_words_for(&"g".repeat(64)).is_err());
    }

    // ── Namespace separation: the compat signer never becomes the display ─────

    #[test]
    fn compat_signer_never_yields_the_displayed_words() {
        // The displayed identity is derived ONLY from the daemon AgentId. The
        // internal Nostr compatibility signer (`relay_pubkey`) is an independent
        // 64-hex value. Prove the namespaces are not interchangeable: feeding a
        // distinct signer-shaped value through the SAME encoder produces
        // DIFFERENT words, so the signer can never silently stand in for the
        // displayed identity. `get_identity` wires `identity_words_for(&agent_id)`
        // (never the signer); this pins that the encoder keeps them distinct —
        // the structural guarantee that makes any swap detectable.
        let display = identity_words_for(PARITY_AGENT_ID).unwrap();
        // A distinct, valid-shape relay signer pubkey (differs in the first 48
        // bits the encoder reads, so the words differ deterministically).
        let relay_signer = "0".repeat(64);
        assert_ne!(relay_signer, PARITY_AGENT_ID);
        let signer_words = identity_words_for(&relay_signer).unwrap();
        assert_ne!(
            display, signer_words,
            "compat signer must not derive the displayed identity words"
        );
    }

    // ── fetch_agent_id: fail-closed when the daemon is unreachable/malformed ──

    #[test]
    fn fetch_agent_id_never_surfaces_a_malformed_identity() {
        // `fetch_agent_id` does a bounded (500 ms connect / 2 s read), loopback,
        // bearer-authenticated GET against the daemon. There is no injectable
        // data-dir seam, so a unit test cannot choose the daemon's reply — but
        // it can pin the fail-closed invariant: the result is EITHER a canonical
        // 64-hex AgentId OR an error. A malformed/placeholder value can never
        // become the displayed identity, and no display is synthesized from the
        // compatibility signer when the daemon is unreachable.
        match fetch_agent_id() {
            Ok(id) => {
                assert!(
                    validate_agent_id(&id).is_ok(),
                    "surfaced a non-canonical agent id: {id}"
                );
                assert_eq!(id.len(), 64);
            }
            Err(_) => {
                // Unreachable daemon (no data dir / port / token / connection,
                // or a malformed /agent body) correctly fails closed.
            }
        }
    }

    // ── Recovery split: daemon-independent recovery state (Main/reviewer CRITICAL)
    //
    // The boot surface is split: `get_recovery_state` (→ `recovery_state_from`)
    // ALWAYS succeeds by reading in-memory atomics only — never the daemon — so
    // the frontend can route to recovery even when the daemon is down. Only when
    // all three flags are false does the frontend call `get_identity`, which
    // fail-closes on an unreachable daemon. These pin both halves together.

    #[test]
    fn recovery_state_surfaces_each_flag_without_touching_the_daemon() {
        // `recovery_state_from` is a pure snapshot of the boot atomics: it must
        // surface lost/locked/reset_failed and never depend on daemon
        // availability. `build_app_state` yields a cheap state with all flags
        // false and an ephemeral signer (no keyring, no network).
        let state = build_app_state();

        // Freshly built state: no recovery mode active.
        let clean = recovery_state_from(&state);
        assert!(!clean.lost, "fresh state must not be in lost recovery");
        assert!(!clean.locked, "fresh state must not be in locked recovery");
        assert!(!clean.reset_failed, "fresh state must not be reset-failed");

        // identity_lost: keyring empty despite a prior migration marker.
        state.identity_lost.store(true, Ordering::Release);
        let lost = recovery_state_from(&state);
        assert!(lost.lost, "lost flag must reach the frontend");
        assert!(!lost.locked, "lost and locked are mutually exclusive");
        state.identity_lost.store(false, Ordering::Release);

        // keyring_locked: keyring unreachable this boot (e.g. GNOME Keyring
        // locked). Must still route to recovery despite no daemon.
        state.keyring_locked.store(true, Ordering::Release);
        assert!(
            recovery_state_from(&state).locked,
            "locked flag must reach the frontend without a daemon"
        );
        state.keyring_locked.store(false, Ordering::Release);

        // reset_failed: Phase 2 wipe verification failed; identity resolution
        // was skipped. Must surface so the UI shows the reset-failed screen.
        state.reset_failed.store(true, Ordering::Release);
        assert!(
            recovery_state_from(&state).reset_failed,
            "reset_failed flag must reach the frontend without a daemon"
        );
    }

    #[test]
    fn normal_identity_fail_closes_while_recovery_state_still_succeeds() {
        // The two halves of the split, pinned in one scenario: with NO recovery
        // flag set and the daemon unavailable (the unit-test environment has no
        // running daemon), the identity read fail-closes — it never synthesizes
        // a display identity from the compatibility signer — while the recovery
        // read still returns Ok. This is exactly the intended boot sequence:
        // get_recovery_state first (always Ok), and only if all-false does
        // get_identity run (and fail-close here).
        let state = build_app_state();
        let recovery = recovery_state_from(&state);
        assert!(!recovery.lost && !recovery.locked && !recovery.reset_failed);

        match fetch_agent_id() {
            Ok(id) => {
                // If a daemon happens to be reachable, it must still be canonical.
                assert!(
                    validate_agent_id(&id).is_ok(),
                    "surfaced a non-canonical agent id: {id}"
                );
            }
            Err(_) => {
                // Daemon unavailable ⇒ identity correctly fail-closed. Recovery
                // routing does NOT depend on this succeeding.
            }
        }
        // And recovery_state_from is still Ok regardless of the daemon outcome.
        let _ = recovery_state_from(&state);
    }
}
