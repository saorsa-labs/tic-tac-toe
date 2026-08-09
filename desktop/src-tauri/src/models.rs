use serde::Serialize;

/// M2 displayed-identity wire shape returned by `get_identity`.
///
/// The user's *displayed* identity is the x0x AgentId (a 64-char lowercase-hex
/// SHA-256 of the agent's ML-DSA-65 public key, fetched from the daemon's
/// `GET /agent`) plus its four speakable identity words. This is the sole
/// identity surfaced to the frontend — the M3 cutover removed the internal
/// Nostr compatibility signer, so there is no relay signer pubkey.
///
/// Recovery state is deliberately NOT part of this struct: those flags live
/// in [`RecoveryStateInfo`] (returned by the separate `get_recovery_state`
/// command), which always succeeds with no daemon dependency so recovery
/// screens are reachable even when the daemon is intentionally not brought
/// up. `get_identity` fail-closes (returns `Err`) when the daemon is
/// unavailable; the frontend checks `get_recovery_state` first and only calls
/// `get_identity` when no recovery flag is set.
#[derive(Serialize)]
pub struct IdentityInfo {
    /// 64-char lowercase-hex x0x AgentId from `GET /agent`. The canonical
    /// displayed user identifier (replaces the old Nostr `pubkey`).
    pub agent_id: String,
    /// Four speakable identity words derived from the AgentId via the
    /// `four-word-networking` `IdentityEncoder` (parity with `x0x agent`).
    pub identity_words: Vec<String>,
}

/// Boot-time recovery state returned by `get_recovery_state`. Always succeeds
/// (reads in-memory atomics, never touches the daemon) so the frontend can
/// route to a recovery screen before attempting `get_identity`, which is
/// fail-closed when the daemon is unavailable.
#[derive(Serialize)]
pub struct RecoveryStateInfo {
    /// Ephemeral compatibility key because the keyring was empty despite a
    /// prior migration marker (key externally deleted). Mutually exclusive
    /// with `locked`.
    pub lost: bool,
    /// Ephemeral compatibility key because the keyring is unreachable this
    /// boot (locked/unavailable). The real key still exists in the keyring.
    /// Mutually exclusive with `lost`.
    pub locked: bool,
    /// Boot-time Phase 2 reset wipe verification failed; the sentinel is
    /// preserved so the next relaunch retries.
    pub reset_failed: bool,
}
