//! Legacy Nostr identity migration — opaque, never activated.
//!
//! The desktop production identity is the x0x AgentId, sourced from the native
//! daemon (`GET /agent`). This module performs a ONE-TIME archival migration of
//! a legacy `identity.key` (an old Nostr `nsec`) into the OS keyring as an
//! **opaque string**, then deletes the plaintext file.
//!
//! The legacy key is NEVER parsed as a Nostr key, NEVER activated as a signer,
//! and NEVER surfaced as identity. It is archived solely so a plaintext secret
//! is not left stranded on disk after the cutover. The `nostr` crate is not
//! compiled into this path.
//!
//! Recovery flags (`identity_lost` / `keyring_locked` / `reset_failed`) are
//! preserved on [`super::AppState`] for frontend recovery-screen routing; only
//! `reset_failed` (the Phase 2 sign-out wipe) is set outside this module.

use std::io::Write;

use tauri::{AppHandle, Manager};

use super::keyring_config::migration_marker_name;
use super::{keyring_service, AppState};

/// Resolve the legacy identity state from the app data directory and wire the
/// resulting [`RecoveryState`] into `AppState`.
///
/// Priority: `BUZZ_PRIVATE_KEY` env var present (opaque dev/shared-identity
/// override) → skip. Otherwise perform the opaque legacy `identity.key` →
/// keyring archival migration and set the recovery flags.
///
/// Stores `identity_lost` / `keyring_locked` with `Release` ordering so a
/// reader observing `false` with `Acquire` sees the completed migration.
pub fn resolve_persisted_identity(app: &AppHandle, state: &AppState) -> Result<(), String> {
    // Shared-identity dev mode: the env var supplies the (opaque) identity.
    // Treated as present-and-non-empty — never parsed as a Nostr key.
    let env_present = std::env::var("BUZZ_PRIVATE_KEY")
        .map(|v| !v.trim().is_empty())
        .unwrap_or(false);
    if env_present {
        return Ok(());
    }

    let data_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("app data dir: {e}"))?;
    std::fs::create_dir_all(&data_dir).map_err(|e| format!("create app data dir: {e}"))?;

    let recovery = load_legacy_identity_state(&data_dir);
    state.identity_lost.store(
        recovery == RecoveryState::Lost,
        std::sync::atomic::Ordering::Release,
    );
    state.keyring_locked.store(
        recovery == RecoveryState::KeyringLocked,
        std::sync::atomic::Ordering::Release,
    );
    Ok(())
}

/// Accept loss of the archived legacy key after the user explicitly starts
/// fresh, then clear `identity_lost` with `Release` ordering.
///
/// There is no signer to replace. The migration marker is removed so the next
/// boot does not re-enter `Lost` recovery — `resolve_persisted_identity` then
/// observes no marker and treats the state as a fresh install.
pub fn replace_lost_identity(app: &AppHandle, state: &AppState) -> Result<(), String> {
    if state
        .keyring_locked
        .load(std::sync::atomic::Ordering::Acquire)
        || state
            .reset_failed
            .load(std::sync::atomic::Ordering::Acquire)
    {
        return Err("identity replacement is unavailable in this recovery state".to_string());
    }

    if !state
        .identity_lost
        .load(std::sync::atomic::Ordering::Acquire)
    {
        return Err("identity replacement requires lost recovery state".to_string());
    }

    let data_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("app data dir: {e}"))?;
    // Best-effort marker removal — a missing marker is already the target state.
    let marker_path = migration_marker_path(&data_dir);
    if let Err(e) = std::fs::remove_file(&marker_path) {
        eprintln!("buzz-desktop: replace_lost_identity: marker removal failed: {e}");
    }
    state
        .identity_lost
        .store(false, std::sync::atomic::Ordering::Release);
    Ok(())
}

/// Keyring key name for the archived human identity (legacy Nostr nsec, stored
/// opaquely). Shared with managed-agent keys under the same service.
const IDENTITY_KEY_NAME: &str = "identity";

/// Filename of the marker written once a successful keyring migration deletes
/// the legacy `identity.key`. Its presence is the only durable signal that a
/// key once lived in the keyring — used to tell a genuine first-ever launch
/// (no key anywhere) from a post-migration boot whose keyring is merely
/// unreachable (the key IS in the keyring, must NOT be treated as fresh).
const MIGRATION_MARKER_NAME: &str = "identity.migrated";

/// Recovery state produced by legacy-identity migration. `None` means no
/// recovery needed. `Lost` means the keyring was reachable-but-empty despite a
/// prior migration marker — the archived key vanished externally.
/// `KeyringLocked` means the keyring is unreachable this boot but was used in
/// the past (marker present, no file). Both non-`None` variants route the
/// frontend to a recovery screen; neither activates a signer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RecoveryState {
    None,
    Lost,
    KeyringLocked,
}

/// Determine the legacy-identity recovery state and perform the opaque archival
/// migration. No Nostr key is ever parsed or activated.
fn load_legacy_identity_state(data_dir: &std::path::Path) -> RecoveryState {
    let legacy_path = data_dir.join("identity.key");

    if !cfg!(feature = "system-keyring") {
        // No keyring backend: the plaintext file (if any) is the only copy and
        // is left in place as archival. Nothing to migrate, no signer to seat.
        return RecoveryState::None;
    }

    let store = crate::secret_store::SecretStore::shared(keyring_service());
    resolve_legacy_with_store(store, &legacy_path, data_dir)
}

/// Legacy-identity resolution over an [`IdentityKeyStore`] seam. Split from
/// [`load_legacy_identity_state`] so the probe/migration branches are testable
/// without the live OS keyring.
fn resolve_legacy_with_store(
    store: &impl IdentityKeyStore,
    legacy_path: &std::path::Path,
    data_dir: &std::path::Path,
) -> RecoveryState {
    use crate::secret_store::KeyringProbe;

    let marker_exists = migration_marker_path(data_dir).exists();

    match store.probe(IDENTITY_KEY_NAME) {
        KeyringProbe::Present => {
            // The archived key is already in the keyring. Clean up any leftover
            // plaintext file (crash-safe: marker before delete) and self-heal
            // the marker so a later unreachable boot can still detect archival.
            if legacy_path.exists() {
                ensure_marker_then_cleanup(data_dir, legacy_path);
            } else if !marker_exists {
                if let Err(e) = write_migration_marker(&migration_marker_path(data_dir)) {
                    eprintln!(
                        "buzz-desktop: keyring present but marker missing; \
                         self-heal marker write failed ({e}), continuing"
                    );
                }
            }
            RecoveryState::None
        }
        KeyringProbe::ReachableButEmpty => {
            // One-time migration: archive the legacy plaintext file opaquely.
            if legacy_path.exists() {
                if let Err(e) = migrate_identity_file(store, legacy_path, data_dir) {
                    eprintln!(
                        "buzz-desktop: legacy identity.key migration failed ({e}); \
                         leaving file in place"
                    );
                }
                RecoveryState::None
            } else if marker_exists {
                // Marker present, keyring empty, no file — the archived key is
                // gone (keyring cleared externally). Surface Lost so the
                // frontend prompts recovery rather than silently treating this
                // as a fresh install.
                RecoveryState::Lost
            } else {
                // Genuine first-ever launch: nothing to archive.
                RecoveryState::None
            }
        }
        KeyringProbe::Unreachable => {
            // Keyring down this boot. With no file but a marker, the archived
            // key exists in the keyring but is inaccessible — boot
            // keyring-locked recovery. A present file (or no marker) needs no
            // recovery.
            if !legacy_path.exists() && marker_exists {
                RecoveryState::KeyringLocked
            } else {
                RecoveryState::None
            }
        }
    }
}

/// The keyring operations the legacy-identity migration needs. Abstracted so
/// the migration is unit-testable against a fake without the live OS keyring.
trait IdentityKeyStore {
    fn probe(&self, name: &str) -> crate::secret_store::KeyringProbe;
    fn store(&self, name: &str, value: &str) -> Result<(), String>;
    fn verify_stored(&self, key: &str, expected: &str) -> Result<bool, String>;
}

impl IdentityKeyStore for crate::secret_store::SecretStore {
    fn probe(&self, name: &str) -> crate::secret_store::KeyringProbe {
        crate::secret_store::SecretStore::probe(self, name)
    }
    fn store(&self, name: &str, value: &str) -> Result<(), String> {
        crate::secret_store::SecretStore::store(self, name, value)
    }
    fn verify_stored(&self, key: &str, expected: &str) -> Result<bool, String> {
        crate::secret_store::SecretStore::verify_stored_raw(self, key, expected)
    }
}

/// Archive the plaintext `identity.key` into the keyring as an opaque string,
/// verify the round-trip, then delete the file. Returns `Err` when the keyring
/// write or verify fails (the caller leaves the file in place so the archival
/// is not lost). The file content is treated as opaque — it is never parsed as
/// a Nostr key.
fn migrate_identity_file(
    store: &impl IdentityKeyStore,
    legacy_path: &std::path::Path,
    data_dir: &std::path::Path,
) -> Result<(), String> {
    let nsec = load_key_file(legacy_path)?;

    store.store(IDENTITY_KEY_NAME, &nsec)?;
    // Read-back verify before deleting the plaintext file. Uses verify_stored()
    // which bypasses the in-process cache and reads directly from the OS
    // backend — proving the OS keyring round-trip, not just the cache.
    let verify_ok = match store.verify_stored(IDENTITY_KEY_NAME, &nsec) {
        Ok(b) => b,
        Err(e) => return Err(format!("keyring read-back verify failed: {e}")),
    };
    if !verify_ok {
        return Err("keyring read-back verify failed for identity key".to_string());
    }
    // Crash-safe ordering: record that the key now lives in the keyring
    // (marker write + fsync) BEFORE deleting the file.
    ensure_marker_then_cleanup(data_dir, legacy_path);
    eprintln!("buzz-desktop: archived legacy identity.key into OS keyring (opaque)");
    Ok(())
}

/// Path of the migration-completed marker within `data_dir`.
fn migration_marker_path(data_dir: &std::path::Path) -> std::path::PathBuf {
    data_dir.join(migration_marker_name(
        keyring_service(),
        MIGRATION_MARKER_NAME,
    ))
}

/// Atomically write (and fsync) the migration-completed marker. The content is
/// irrelevant — only the file's durable existence is the signal — so a single
/// byte keeps it minimal. Atomicity + fsync guarantee that once this returns
/// `Ok`, the marker survives a crash, which is what makes deleting the legacy
/// file afterward safe.
fn write_migration_marker(marker_path: &std::path::Path) -> Result<(), String> {
    use atomic_write_file::AtomicWriteFile;

    let mut f = AtomicWriteFile::open(marker_path).map_err(|e| format!("marker open: {e}"))?;
    f.write_all(b"1")
        .map_err(|e| format!("marker write: {e}"))?;
    f.commit().map_err(|e| format!("marker sync: {e}"))?;
    Ok(())
}

/// Ensure the migration marker exists (writing it if absent), then remove the
/// leftover `identity.key`. Crash-safe ordering: the marker is written and
/// fsync-committed before the file is deleted, so a crash between the two
/// leaves the marker on disk and the file intact — the invariant
/// "keyring-only implies marker exists" is preserved. If the marker write
/// fails, the file is kept so a later keyring-unreachable boot can use it.
fn ensure_marker_then_cleanup(data_dir: &std::path::Path, legacy_path: &std::path::Path) {
    let marker_path = migration_marker_path(data_dir);
    if !marker_path.exists() {
        if let Err(e) = write_migration_marker(&marker_path) {
            eprintln!("buzz-desktop: migration marker write failed ({e}); keeping identity.key");
            return;
        }
    }
    cleanup_leftover_identity_file(legacy_path);
}

/// Best-effort removal of a leftover `identity.key` once the keyring is the
/// authoritative store. Idempotent: a missing file is success. Logs but does
/// not error on failure — a delete failure must never block startup.
fn cleanup_leftover_identity_file(legacy_path: &std::path::Path) {
    if let Err(e) = std::fs::remove_file(legacy_path) {
        if e.kind() != std::io::ErrorKind::NotFound {
            eprintln!("buzz-desktop: failed to delete leftover identity.key: {e}");
        }
    }
}

/// Read the legacy `identity.key` as an opaque string. Rejects an empty file.
/// The content is never parsed as a Nostr key — it is archived verbatim.
fn load_key_file(path: &std::path::Path) -> Result<String, String> {
    let content = std::fs::read_to_string(path).map_err(|e| format!("read identity.key: {e}"))?;
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return Err("identity.key is empty".to_string());
    }
    Ok(trimmed.to_string())
}

/// Atomically write an opaque key string to disk (`0o600` on Unix). Retained
/// for the test suite, which exercises the atomic/restricted-write path; the
/// production migration no longer writes a plaintext signer (the legacy file is
/// archived to the keyring and deleted, never regenerated).
#[cfg(test)]
pub(crate) fn save_key_file(path: &std::path::Path, nsec: &str) -> Result<(), String> {
    use atomic_write_file::AtomicWriteFile;

    let mut f = AtomicWriteFile::open(path).map_err(|e| format!("open identity.key: {e}"))?;
    f.write_all(nsec.as_bytes())
        .map_err(|e| format!("write identity.key: {e}"))?;
    f.commit().map_err(|e| format!("sync identity.key: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secret_store::KeyringProbe;
    use std::cell::RefCell;

    /// In-memory [`IdentityKeyStore`] for the opaque migration tests. Interior
    /// mutability via `RefCell` so the `&self` trait methods can record state.
    struct FakeStore {
        value: RefCell<Option<String>>,
        probe_outcome: KeyringProbe,
        store_fails: bool,
        readback_override: Option<String>,
    }

    impl FakeStore {
        fn present(value: &str) -> Self {
            Self {
                value: RefCell::new(Some(value.to_string())),
                probe_outcome: KeyringProbe::Present,
                store_fails: false,
                readback_override: None,
            }
        }
        fn empty() -> Self {
            Self {
                value: RefCell::new(None),
                probe_outcome: KeyringProbe::ReachableButEmpty,
                store_fails: false,
                readback_override: None,
            }
        }
        fn unreachable() -> Self {
            Self {
                value: RefCell::new(None),
                probe_outcome: KeyringProbe::Unreachable,
                store_fails: false,
                readback_override: None,
            }
        }
        fn with_store_failing() -> Self {
            Self {
                value: RefCell::new(None),
                probe_outcome: KeyringProbe::ReachableButEmpty,
                store_fails: true,
                readback_override: None,
            }
        }
        fn with_readback_corruption(other: &str) -> Self {
            Self {
                value: RefCell::new(None),
                probe_outcome: KeyringProbe::ReachableButEmpty,
                store_fails: false,
                readback_override: Some(other.to_string()),
            }
        }
        fn stored(&self) -> Option<String> {
            self.value.borrow().clone()
        }
    }

    impl IdentityKeyStore for FakeStore {
        fn probe(&self, _name: &str) -> KeyringProbe {
            self.probe_outcome
        }
        fn store(&self, _name: &str, value: &str) -> Result<(), String> {
            if self.store_fails {
                return Err("keyring store failed (fake)".to_string());
            }
            *self.value.borrow_mut() = Some(value.to_string());
            Ok(())
        }
        fn verify_stored(&self, _key: &str, expected: &str) -> Result<bool, String> {
            if let Some(override_val) = &self.readback_override {
                return Ok(override_val == expected);
            }
            Ok(self.value.borrow().as_deref() == Some(expected))
        }
    }

    fn dir() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    // ── save/load (opaque) ───────────────────────────────────────────────────

    #[test]
    fn save_and_load_round_trip_opaque() {
        let d = dir();
        let path = d.path().join("identity.key");
        save_key_file(&path, "nsec1opaquelegacykey").unwrap();
        assert_eq!(load_key_file(&path).unwrap(), "nsec1opaquelegacykey");
    }

    #[test]
    fn load_rejects_empty_file() {
        let d = dir();
        let path = d.path().join("identity.key");
        std::fs::write(&path, "   ").unwrap();
        assert!(load_key_file(&path).is_err());
    }

    #[test]
    fn load_missing_file_is_err() {
        let d = dir();
        assert!(load_key_file(&d.path().join("nope.key")).is_err());
    }

    // ── migrate_identity_file: opaque archive + delete ───────────────────────

    #[test]
    fn migrate_archives_opaque_and_deletes_file() {
        let d = dir();
        let path = d.path().join("identity.key");
        save_key_file(&path, "nsec1legacy").unwrap();
        let store = FakeStore::empty();
        migrate_identity_file(&store, &path, d.path()).unwrap();
        // Keyring holds the opaque string verbatim (never parsed as a Nostr key).
        assert_eq!(store.stored().as_deref(), Some("nsec1legacy"));
        assert!(!path.exists());
        assert!(migration_marker_path(d.path()).exists());
    }

    #[test]
    fn migrate_readback_mismatch_leaves_file() {
        let d = dir();
        let path = d.path().join("identity.key");
        save_key_file(&path, "nsec1legacy").unwrap();
        let store = FakeStore::with_readback_corruption("nsec1other");
        assert!(migrate_identity_file(&store, &path, d.path()).is_err());
        assert!(path.exists());
    }

    #[test]
    fn migrate_store_failure_leaves_file() {
        let d = dir();
        let path = d.path().join("identity.key");
        save_key_file(&path, "nsec1legacy").unwrap();
        let store = FakeStore::with_store_failing();
        assert!(migrate_identity_file(&store, &path, d.path()).is_err());
        assert!(path.exists());
    }

    // ── resolve_legacy_with_store: recovery states ───────────────────────────

    #[test]
    fn resolve_present_cleans_leftover_file() {
        let d = dir();
        let path = d.path().join("identity.key");
        save_key_file(&path, "nsec1legacy").unwrap();
        let store = FakeStore::present("nsec1archived");
        assert_eq!(
            resolve_legacy_with_store(&store, &path, d.path()),
            RecoveryState::None
        );
        assert!(!path.exists());
        assert!(migration_marker_path(d.path()).exists());
    }

    #[test]
    fn resolve_empty_with_file_migrates() {
        let d = dir();
        let path = d.path().join("identity.key");
        save_key_file(&path, "nsec1legacy").unwrap();
        let store = FakeStore::empty();
        assert_eq!(
            resolve_legacy_with_store(&store, &path, d.path()),
            RecoveryState::None
        );
        assert_eq!(store.stored().as_deref(), Some("nsec1legacy"));
        assert!(!path.exists());
    }

    #[test]
    fn resolve_empty_no_file_marker_is_lost() {
        let d = dir();
        let path = d.path().join("identity.key");
        write_migration_marker(&migration_marker_path(d.path())).unwrap();
        let store = FakeStore::empty();
        assert_eq!(
            resolve_legacy_with_store(&store, &path, d.path()),
            RecoveryState::Lost
        );
    }

    #[test]
    fn resolve_empty_no_file_no_marker_is_none() {
        let d = dir();
        let path = d.path().join("identity.key");
        let store = FakeStore::empty();
        assert_eq!(
            resolve_legacy_with_store(&store, &path, d.path()),
            RecoveryState::None
        );
    }

    #[test]
    fn resolve_unreachable_no_file_marker_is_locked() {
        let d = dir();
        let path = d.path().join("identity.key");
        write_migration_marker(&migration_marker_path(d.path())).unwrap();
        let store = FakeStore::unreachable();
        assert_eq!(
            resolve_legacy_with_store(&store, &path, d.path()),
            RecoveryState::KeyringLocked
        );
    }

    #[test]
    fn resolve_unreachable_with_file_is_none() {
        let d = dir();
        let path = d.path().join("identity.key");
        save_key_file(&path, "nsec1legacy").unwrap();
        let store = FakeStore::unreachable();
        // File present → no recovery (do not migrate when keyring is down).
        assert_eq!(
            resolve_legacy_with_store(&store, &path, d.path()),
            RecoveryState::None
        );
        assert!(path.exists());
    }

    // ── cleanup ──────────────────────────────────────────────────────────────

    #[test]
    fn cleanup_removes_file() {
        let d = dir();
        let path = d.path().join("identity.key");
        save_key_file(&path, "x").unwrap();
        cleanup_leftover_identity_file(&path);
        assert!(!path.exists());
    }

    #[test]
    fn cleanup_is_noop_when_absent() {
        let d = dir();
        let path = d.path().join("identity.key");
        cleanup_leftover_identity_file(&path);
        assert!(!path.exists());
    }
}
