//! Post-cutover AppState smoke tests.
//!
//! The Nostr compatibility signer (`signing_keys` / `compat_signer` /
//! `identity_from_env`) was removed in the M3 cutover — the production identity
//! is the x0x AgentId (native daemon). These tests pin that the app state still
//! builds and that the recovery atomics (preserved for frontend routing) start
//! clear. Recovery-flag routing is covered by `commands::identity::identity_tests`.

use super::*;

#[test]
fn build_app_state_succeeds_with_no_nostr_signer() {
    // The state builds with the native identity surface and no Nostr signer.
    let state = build_app_state();
    // Recovery flags preserved for frontend recovery screens; start clear.
    assert!(
        !state
            .identity_lost
            .load(std::sync::atomic::Ordering::Acquire),
        "fresh state must not be in lost recovery"
    );
    assert!(
        !state
            .keyring_locked
            .load(std::sync::atomic::Ordering::Acquire),
        "fresh state must not be in locked recovery"
    );
    assert!(
        !state
            .reset_failed
            .load(std::sync::atomic::Ordering::Acquire),
        "fresh state must not be reset-failed"
    );
}
