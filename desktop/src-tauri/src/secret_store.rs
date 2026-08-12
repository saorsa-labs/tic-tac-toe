//! OS keyring access for desktop nsec private keys.
//!
//! All secrets are stored as a single JSON blob under one keychain entry
//! (service = the store's service name, username = `"secrets"`). This means
//! exactly one OS prompt per process lifetime regardless of how many keys are
//! stored — the same pattern used by Goose.
//!
//! The chosen backend is selected at compile time by the per-target feature in
//! `Cargo.toml`. On macOS the legacy `keyring` crate (SecKeychain API) is used
//! for the blob entry. Signed release and unsigned dev builds deliberately use
//! distinct service namespaces; DPK (Data Protection Keychain) is used only
//! by the one-time migration path that reads old per-key entries written by
//! #1264.
//! Windows and Linux use the `keyring` crate directly. The `system-keyring`
//! feature gates the whole store; when it is off, [`SecretStore`] is unusable
//! and callers fall back to their own `0o600` file storage.
//!
//! The store is deliberately NOT on any env-read path. `BUZZ_PRIVATE_KEY`
//! resolution for harnessed agents and CI is handled upstream (an env
//! short-circuit for the human key, child-process env injection for agents);
//! adding an env tier here would duplicate that precedence and create a
//! divergent-behavior trap.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

/// Result of probing the keyring before a migration: distinguishes "reachable
/// but holds no entry" (safe to migrate into) from "unreachable this boot"
/// (must NOT migrate — re-importing from a leftover plaintext file could
/// resurrect a rotated/stale key).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyringProbe {
    /// Keyring is reachable and an entry for the key already exists.
    Present,
    /// Keyring is reachable but has no entry for the key.
    ReachableButEmpty,
    /// Keyring backend is unavailable this boot (no Secret Service, dbus
    /// failure, etc.). Migration must be skipped.
    Unreachable,
}

/// Username used for the single blob keychain entry. All secrets are stored
/// as a JSON map under this name within the service.
const BLOB_KEY: &str = "secrets";

// ── Interprocess advisory lock ─────────────────────────────────────────────
//
// Two concurrent desktop processes using the same service namespace share one
// OS keychain blob. Each process holds its own in-memory cache, so without an
// interprocess lock a warm-cache write in process A drops keys added by process
// B between A's last cache-warming read and A's write.
//
// The fix: `mutate_blob` acquires an exclusive advisory file lock, then always
// performs a fresh `read_blob_raw()` inside the lock, applies the mutation,
// writes back, and releases. The cache is still updated after a successful
// write, so same-process reads remain fast. The lock is file-based at a fixed
// per-user path `/tmp/buzz-keychain-<uid>-<service>.lock` on Unix — a path
// that is invariant to `$TMPDIR`/process environment, so both the GUI-launched
// signed DMG and a terminal-launched dev build always take the same lock.

/// Return the path of the advisory lockfile for `service`.
///
/// The path is `/tmp/buzz-keychain-<uid>-<service>.lock` on Unix — a
/// deterministic per-user path that is invariant to `$TMPDIR`/process
/// environment. Both a GUI-launched signed DMG (`launchd`, env-stripped) and a
/// terminal-launched dev build resolve `/tmp` to the same inode, so they
/// contend on the same lockfile and achieve mutual exclusion.
///
/// On Windows the same name used for the kernel mutex is derived from the
/// lockfile path, so the service-keyed uniqueness is preserved.
fn blob_lockfile_path(service: &str) -> PathBuf {
    #[cfg(unix)]
    {
        // Use the real UID so distinct users get distinct lockfiles.
        // SAFETY: getuid() is always safe on Unix — it never fails.
        let uid = unsafe { libc::getuid() };
        PathBuf::from(format!("/tmp/buzz-keychain-{uid}-{service}.lock"))
    }
    #[cfg(not(unix))]
    {
        // Windows: no lockfile used (named mutex instead); this path is only
        // used to derive the mutex name and for test assertions.
        std::env::temp_dir().join(format!("buzz-keychain-{service}.lock"))
    }
}

/// Acquire an exclusive advisory file lock for the blob identified by `service`.
///
/// Opens (or creates) the lockfile and blocks until the lock is acquired.
/// Returns the open `File`; the lock is released when the file is dropped.
///
/// On non-Unix/non-Windows platforms this is a no-op that returns a stub.
#[cfg(feature = "system-keyring")]
fn acquire_blob_lock(service: &str) -> Result<BlobLockGuard, String> {
    let path = blob_lockfile_path(service);
    BlobLockGuard::acquire(&path)
}

/// RAII guard that holds an exclusive advisory file lock.
///
/// On Unix, implemented via `flock(2)` on a lockfile in the system temp dir.
/// On Windows, implemented via a named kernel mutex (cross-process, no file I/O
/// needed). The Windows mutex handle is released on drop.
#[cfg(feature = "system-keyring")]
struct BlobLockGuard {
    /// The open lockfile. Never read — held purely for RAII: closing the fd
    /// releases the `flock(LOCK_EX)` on Unix.
    #[cfg(unix)]
    #[allow(dead_code)]
    file: std::fs::File,
    #[cfg(windows)]
    mutex_handle: windows_sys::Win32::Foundation::HANDLE,
}

#[cfg(feature = "system-keyring")]
impl BlobLockGuard {
    fn acquire(path: &std::path::Path) -> Result<Self, String> {
        #[cfg(unix)]
        {
            let file = std::fs::OpenOptions::new()
                .create(true)
                .truncate(false)
                .write(true)
                .open(path)
                .map_err(|e| format!("blob lock open {}: {e}", path.display()))?;
            use std::os::unix::io::AsRawFd;
            // LOCK_EX blocks until the lock is acquired (no LOCK_NB).
            let ret = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
            if ret != 0 {
                let err = std::io::Error::last_os_error();
                return Err(format!("blob lock flock: {err}"));
            }
            return Ok(BlobLockGuard { file });
        }

        #[cfg(windows)]
        {
            // Named kernel mutexes are cross-process on Windows — no lockfile
            // needed. Derive a unique mutex name from the lockfile path so
            // distinct services get distinct mutexes.
            let name_str = format!(
                "Local\\BuzzKeychain-{}",
                path.file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("default")
            );
            // Encode as null-terminated UTF-16.
            let name_wide: Vec<u16> = name_str
                .encode_utf16()
                .chain(std::iter::once(0u16))
                .collect();
            use windows_sys::Win32::Foundation::WAIT_OBJECT_0;
            use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
            use windows_sys::Win32::System::Threading::{
                CreateMutexW, WaitForSingleObject, INFINITE,
            };
            // CreateMutexW: lpMutexAttributes = null (default security),
            // bInitialOwner = FALSE (0), lpName = our mutex name.
            let handle = unsafe {
                CreateMutexW(
                    std::ptr::null::<SECURITY_ATTRIBUTES>(),
                    0,
                    name_wide.as_ptr(),
                )
            };
            // HANDLE = *mut c_void; null means creation failed.
            if handle.is_null() {
                let err = std::io::Error::last_os_error();
                return Err(format!("blob lock CreateMutexW: {err}"));
            }
            let wait_result = unsafe { WaitForSingleObject(handle, INFINITE) };
            if wait_result != WAIT_OBJECT_0 {
                // Also accept WAIT_ABANDONED (0x80) — previous holder crashed;
                // the mutex is still acquired and we own it.
                if wait_result != windows_sys::Win32::Foundation::WAIT_ABANDONED {
                    let err = std::io::Error::last_os_error();
                    unsafe { windows_sys::Win32::Foundation::CloseHandle(handle) };
                    return Err(format!(
                        "blob lock WaitForSingleObject: {wait_result} / {err}"
                    ));
                }
            }
            return Ok(BlobLockGuard {
                mutex_handle: handle,
            });
        }

        // Fallback for exotic platforms: no-op lock (only Unix/Windows ship).
        #[allow(unreachable_code)]
        Err("blob lock: unsupported platform".to_string())
    }
}

#[cfg(feature = "system-keyring")]
impl Drop for BlobLockGuard {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            // Dropping `self.file` closes the fd, which releases flock on Unix.
            // Nothing explicit needed.
        }
        #[cfg(windows)]
        {
            unsafe {
                windows_sys::Win32::System::Threading::ReleaseMutex(self.mutex_handle);
                windows_sys::Win32::Foundation::CloseHandle(self.mutex_handle);
            }
        }
    }
}

// ── End interprocess advisory lock ────────────────────────────────────────

/// An OS keyring, addressed by service name. All secrets are stored in a
/// single JSON blob entry (one OS prompt per process lifetime).
pub struct SecretStore {
    service: String,
    /// In-memory cache of the deserialized blob. `None` means "not yet loaded".
    cache: Mutex<Option<HashMap<String, String>>>,
}

impl SecretStore {
    /// Keyring-backed store under `service`. The active platform backend
    /// (apple-native / windows-native / sync-secret-service) is chosen at
    /// compile time.
    pub fn keyring(service: impl Into<String>) -> Self {
        SecretStore {
            service: service.into(),
            cache: Mutex::new(None),
        }
    }

    /// Return a process-global `SecretStore` for `service`. All callers with
    /// the same service name share one instance — and therefore one in-memory
    /// cache and one mutex — so concurrent blob read-modify-write operations
    /// see each other's writes and the last-writer-wins race is closed.
    ///
    /// One service name is selected per process by `app_state::keyring_service`.
    /// If multiple services are ever needed concurrently, this can be extended
    /// to a registry.
    pub fn shared(service: &'static str) -> &'static SecretStore {
        use std::sync::OnceLock;
        static INSTANCE: OnceLock<SecretStore> = OnceLock::new();
        INSTANCE.get_or_init(|| SecretStore::keyring(service))
    }
}

/// Whether a keyring error string indicates the backend itself is unavailable
/// (vs. a per-entry error like "not found"). Mirrors goose's discriminator
/// (`crates/goose/src/config/base.rs`): treat dbus / Secret Service / platform
/// secure-storage failures as "keyring unavailable, fall back to file".
#[cfg(feature = "system-keyring")]
fn is_keyring_availability_error(error_str: &str) -> bool {
    let lower = error_str.to_lowercase();
    lower.contains("keyring")
        || lower.contains("dbus")
        || lower.contains("org.freedesktop.secrets")
        || lower.contains("platform secure storage")
        || lower.contains("no secret service")
}

#[cfg(feature = "system-keyring")]
fn keyring_entry(service: &str, key: &str) -> Result<keyring::Entry, keyring::Error> {
    keyring::Entry::new(service, key)
}

// macOS-specific imports for the Data Protection Keychain backend.
#[cfg(all(feature = "system-keyring", target_os = "macos"))]
use security_framework::base::Error as SFError;
#[cfg(all(feature = "system-keyring", target_os = "macos"))]
use security_framework::passwords::{
    delete_generic_password_options, generic_password, PasswordOptions,
};

/// Returns true when the security-framework error is "item not found" (-25300).
#[cfg(all(feature = "system-keyring", target_os = "macos"))]
fn is_not_found(e: &SFError) -> bool {
    e.code() == -25300
}

/// Returns true when DPK is unavailable because the binary lacks the required
/// entitlement (`errSecMissingEntitlement`, -34018). This happens for unsigned
/// dev builds (`tauri dev` / `cargo run`). The caller should fall back to the
/// legacy `keyring` crate path, which uses the old-style keychain and does not
/// require hardened-runtime entitlements.
#[cfg(all(feature = "system-keyring", target_os = "macos"))]
fn is_dpk_unavailable(e: &SFError) -> bool {
    e.code() == -34018
}

/// Build a `PasswordOptions` for the Data Protection Keychain.
#[cfg(all(feature = "system-keyring", target_os = "macos"))]
fn dpk_opts(service: &str, key: &str) -> PasswordOptions {
    let mut opts = PasswordOptions::new_generic_password(service, key);
    opts.use_protected_keychain();
    opts
}

impl SecretStore {
    /// Read the blob from the keychain and return the deserialized map.
    ///
    /// Returns `Ok(None)` when no blob entry exists yet (first launch or
    /// fresh install). Returns `Err` when the backend is unavailable or the
    /// stored JSON is corrupt.
    ///
    /// On success the result is stored in `self.cache` so subsequent calls
    /// within the same process return immediately without a keychain round-trip.
    #[cfg(feature = "system-keyring")]
    fn load_blob(&self) -> Result<Option<HashMap<String, String>>, String> {
        {
            let guard = self.cache.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(ref map) = *guard {
                return Ok(Some(map.clone()));
            }
        }

        let raw = self.read_blob_raw()?;
        let map = match raw {
            None => return Ok(None),
            Some(bytes) => {
                let json = String::from_utf8(bytes).map_err(|e| format!("blob utf8: {e}"))?;
                serde_json::from_str::<HashMap<String, String>>(&json)
                    .map_err(|e| format!("blob json: {e}"))?
            }
        };

        // Only populate the cache if it is still empty — a concurrent
        // mutate_blob() may have written a newer value while we were reading.
        let mut guard = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        if guard.is_none() {
            *guard = Some(map.clone());
        }
        Ok(Some(map))
    }

    /// Read the raw blob bytes from the keychain. `Ok(None)` = not found.
    ///
    /// Always uses the legacy keyring crate on macOS so that signed and
    /// unsigned (dev) builds share the same store. DPK is only used by
    /// `migrate_legacy_key` to read old per-key entries written by #1264.
    #[cfg(all(feature = "system-keyring", target_os = "macos"))]
    fn read_blob_raw(&self) -> Result<Option<Vec<u8>>, String> {
        self.read_blob_raw_keyring()
    }

    #[cfg(all(feature = "system-keyring", not(target_os = "macos")))]
    fn read_blob_raw(&self) -> Result<Option<Vec<u8>>, String> {
        self.read_blob_raw_keyring()
    }

    /// Read blob via the legacy `keyring` crate (Windows, Linux, or macOS dev
    /// builds that lack hardened-runtime entitlements).
    #[cfg(feature = "system-keyring")]
    fn read_blob_raw_keyring(&self) -> Result<Option<Vec<u8>>, String> {
        let entry =
            keyring_entry(&self.service, BLOB_KEY).map_err(|e| format!("keyring entry: {e}"))?;
        match entry.get_password() {
            Ok(s) => Ok(Some(s.into_bytes())),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) if is_keyring_availability_error(&e.to_string()) => {
                Err(format!("keyring unavailable: {e}"))
            }
            Err(e) => Err(format!("keyring read: {e}")),
        }
    }

    /// Atomically load the blob, apply `f` to a candidate map, write back if
    /// changed, and only then advance the cache.
    ///
    /// **Cross-process safety**: acquires an exclusive advisory file lock
    /// (`flock(2)` on Unix, `LockFileEx` on Windows) before reading, mutating,
    /// and writing. The lock is keyed by service name and stored in the system
    /// temp directory, making it reachable from both the signed DMG build and
    /// unsigned dev builds. Inside the lock a fresh `read_blob_raw()` is always
    /// performed (even when the cache is warm) so a concurrent process's write
    /// is never silently dropped.
    ///
    /// **Idempotent**: when `f` leaves the candidate equal to the freshly-read
    /// map, `write_blob_raw` is skipped entirely. On macOS the legacy
    /// `SecKeychain` API treats a write as a distinct ACL operation from the
    /// "Always Allow"-ed read, so skipping no-op writes eliminates the keychain
    /// prompt that fires when saving an agent whose model changed but whose key
    /// did not.
    ///
    /// **Copy-on-write**: the candidate `next` is a separate allocation from
    /// `current`. The cache is only replaced with `next` after `write_blob_raw`
    /// succeeds. On write failure the cache is cleared to `None` so the next
    /// caller re-reads from the keychain rather than building on a stale state.
    ///
    /// Deadlock-free: `read_blob_raw` and `write_blob_raw` do not acquire the
    /// cache mutex. `load_blob` does acquire it, but `mutate_blob` does not call
    /// `load_blob` — it reads from the keyring directly inside the file lock.
    #[cfg(feature = "system-keyring")]
    fn mutate_blob<F>(&self, f: F) -> Result<(), String>
    where
        F: FnOnce(&mut HashMap<String, String>),
    {
        // Acquire the interprocess advisory lock first. All Buzz processes
        // using the same service name contend on the same lockfile at
        // /tmp/buzz-keychain-<uid>-<service>.lock (a deterministic per-user
        // path invariant to $TMPDIR), so only one process performs a
        // read-modify-write at a time.
        let _lock = acquire_blob_lock(&self.service)?;

        // Always do a fresh read from the keychain while holding the lock —
        // this is the critical correction over the prior warm-cache path. A
        // stale warm cache would make us build our candidate on an outdated
        // baseline and drop keys written by another process.
        let raw = self.read_blob_raw()?;
        let current: HashMap<String, String> = match raw {
            None => HashMap::new(),
            Some(bytes) => {
                let json = String::from_utf8(bytes).map_err(|e| format!("blob utf8: {e}"))?;
                serde_json::from_str::<HashMap<String, String>>(&json)
                    .map_err(|e| format!("blob json: {e}"))?
            }
        };

        // Build the candidate state in a separate allocation so that a write
        // failure below cannot leave the cache ahead of durable storage.
        let mut next = current.clone();
        f(&mut next);

        // Skip the keychain write when the candidate equals the freshly-read
        // durable state — no I/O needed and no keychain ACL prompt on macOS.
        if next == current {
            // Update the cache to the fresh read even on no-op so subsequent
            // reads in this process see any keys another process may have added.
            let mut guard = self.cache.lock().unwrap_or_else(|e| e.into_inner());
            *guard = Some(current);
            return Ok(());
        }

        // Write to keyring while still holding the file lock.
        let json = serde_json::to_string(&next).map_err(|e| format!("blob serialize: {e}"))?;
        match self.write_blob_raw(json.as_bytes()) {
            Ok(()) => {
                // Advance the cache to `next` only after the durable write succeeds.
                let mut guard = self.cache.lock().unwrap_or_else(|e| e.into_inner());
                *guard = Some(next);
                Ok(())
            }
            Err(e) => {
                // On write failure, clear the cache so the next caller re-reads
                // from the keychain rather than building on a stale state.
                let mut guard = self.cache.lock().unwrap_or_else(|e| e.into_inner());
                *guard = None;
                Err(e)
            }
        }
    }

    /// Always uses the legacy keyring crate on macOS — see `read_blob_raw`.
    #[cfg(all(feature = "system-keyring", target_os = "macos"))]
    fn write_blob_raw(&self, bytes: &[u8]) -> Result<(), String> {
        self.write_blob_raw_keyring(bytes)
    }

    #[cfg(all(feature = "system-keyring", not(target_os = "macos")))]
    fn write_blob_raw(&self, bytes: &[u8]) -> Result<(), String> {
        self.write_blob_raw_keyring(bytes)
    }

    #[cfg(feature = "system-keyring")]
    fn write_blob_raw_keyring(&self, bytes: &[u8]) -> Result<(), String> {
        let value = std::str::from_utf8(bytes).map_err(|e| format!("blob utf8 encode: {e}"))?;
        let entry =
            keyring_entry(&self.service, BLOB_KEY).map_err(|e| format!("keyring entry: {e}"))?;
        entry
            .set_password(value)
            .map_err(|e| format!("keyring write: {e}"))
    }

    /// Probe whether `key` exists and whether the backend is reachable.
    pub fn probe(&self, key: &str) -> KeyringProbe {
        #[cfg(feature = "system-keyring")]
        {
            match self.load_blob() {
                Ok(Some(map)) => {
                    if map.contains_key(key) {
                        KeyringProbe::Present
                    } else {
                        // Blob exists but key absent — still check old per-key
                        // entries so a partial migration (e.g. identity migrated
                        // first) doesn't silently drop agent keys.
                        self.probe_legacy_key(key)
                    }
                }
                // No blob yet — check old per-key entries so callers that
                // gate `load()` on `Present` still trigger migration.
                Ok(None) => self.probe_legacy_key(key),
                Err(e) if is_keyring_availability_error(&e) => KeyringProbe::Unreachable,
                Err(_) => KeyringProbe::Unreachable, // corrupt blob — fail closed
            }
        }
        #[cfg(not(feature = "system-keyring"))]
        {
            let _ = key;
            KeyringProbe::Unreachable
        }
    }

    /// Check old per-key DPK/keyring entries for `key`. Used by `probe()` when
    /// the blob doesn't exist yet (first launch after upgrade).
    #[cfg(all(feature = "system-keyring", target_os = "macos"))]
    fn probe_legacy_key(&self, key: &str) -> KeyringProbe {
        match generic_password(dpk_opts(&self.service, key)) {
            Ok(_) => KeyringProbe::Present,
            Err(ref e) if is_not_found(e) => self.probe_legacy_key_keyring(key),
            Err(ref e) if is_dpk_unavailable(e) => self.probe_legacy_key_keyring(key),
            Err(ref e) if is_keyring_availability_error(&e.to_string()) => {
                KeyringProbe::Unreachable
            }
            Err(_) => KeyringProbe::ReachableButEmpty,
        }
    }

    #[cfg(all(feature = "system-keyring", not(target_os = "macos")))]
    fn probe_legacy_key(&self, key: &str) -> KeyringProbe {
        self.probe_legacy_key_keyring(key)
    }

    #[cfg(feature = "system-keyring")]
    fn probe_legacy_key_keyring(&self, key: &str) -> KeyringProbe {
        match keyring_entry(&self.service, key) {
            Ok(entry) => match entry.get_password() {
                Ok(_) => KeyringProbe::Present,
                Err(keyring::Error::NoEntry) => KeyringProbe::ReachableButEmpty,
                Err(e) if is_keyring_availability_error(&e.to_string()) => {
                    KeyringProbe::Unreachable
                }
                Err(_) => KeyringProbe::ReachableButEmpty,
            },
            Err(e) if is_keyring_availability_error(&e.to_string()) => KeyringProbe::Unreachable,
            Err(_) => KeyringProbe::Unreachable,
        }
    }

    /// Verify that `key` holds `expected` by reading directly from the OS
    /// backend, bypassing the in-process cache. This is the key innovation for
    /// read-back verification: it proves the OS keyring round-trip, not just
    /// that the in-process cache was updated.
    ///
    /// Returns `Ok(true)` when the stored value matches `expected`, `Ok(false)`
    /// when the entry is absent or holds a different value, and `Err` when the
    /// backend is unavailable.
    pub fn verify_stored_raw(&self, key: &str, expected: &str) -> Result<bool, String> {
        #[cfg(feature = "system-keyring")]
        {
            let raw = self.read_blob_raw()?;
            match raw {
                None => Ok(false),
                Some(bytes) => {
                    let json = String::from_utf8(bytes).map_err(|e| format!("blob utf8: {e}"))?;
                    let map =
                        serde_json::from_str::<std::collections::HashMap<String, String>>(&json)
                            .map_err(|e| format!("blob json: {e}"))?;
                    Ok(map.get(key).is_some_and(|v| v == expected))
                }
            }
        }
        #[cfg(not(feature = "system-keyring"))]
        {
            let _ = (key, expected);
            Err("system-keyring feature disabled".to_string())
        }
    }

    /// Store `value` for `key`. Reports `Err` on availability failures — callers
    /// decide whether to fall back to file storage.
    pub fn store(&self, key: &str, value: &str) -> Result<(), String> {
        #[cfg(feature = "system-keyring")]
        {
            self.mutate_blob(|map| {
                map.insert(key.to_string(), value.to_string());
            })
        }
        #[cfg(not(feature = "system-keyring"))]
        {
            let _ = (key, value);
            Err("system-keyring feature disabled".to_string())
        }
    }

    /// Delete the entire keychain blob for this service, plus all legacy per-key
    /// entries that could resurrect an identity on next boot.
    ///
    /// Order of operations:
    /// 1. Read the blob to collect every key name (e.g. `identity`, agent keys).
    /// 2. Delete legacy per-key DPK entries for every key + the DPK blob itself.
    /// 3. Delete legacy per-key keyring entries for every key.
    /// 4. Delete the blob entry.
    /// 5. Clear the in-memory cache.
    ///
    /// This is the correct wipe path for sign-out: the old `delete_all` skipped
    /// step 1–3 so stale per-key entries could be re-imported on the next launch
    /// via `migrate_legacy_key`. This method prevents that resurrection.
    pub fn delete_all_with_legacy_cleanup(&self) -> Result<(), String> {
        #[cfg(feature = "system-keyring")]
        {
            let _lock = acquire_blob_lock(&self.service)?;

            // Step 1: read current blob keys (best-effort; no entry = empty set).
            let blob_keys: Vec<String> = match self.read_blob_raw() {
                Ok(Some(bytes)) => {
                    let json = String::from_utf8(bytes).unwrap_or_default();
                    serde_json::from_str::<std::collections::HashMap<String, String>>(&json)
                        .map(|m| m.into_keys().collect())
                        .unwrap_or_default()
                }
                _ => vec![],
            };

            // Always include "identity" even if the blob is empty or absent —
            // it may exist only as a legacy per-key entry.
            let mut all_keys = blob_keys;
            if !all_keys.contains(&"identity".to_string()) {
                all_keys.push("identity".to_string());
            }

            // Steps 2 & 3: delete legacy per-key entries for every key.
            for key in &all_keys {
                #[cfg(target_os = "macos")]
                {
                    match delete_generic_password_options(dpk_opts(&self.service, key)) {
                        Ok(()) => {}
                        Err(ref e) if is_not_found(e) => {}
                        Err(ref e) if is_dpk_unavailable(e) => {}
                        Err(e) => return Err(format!("dpk per-key delete {key}: {e}")),
                    }
                }
                {
                    let entry = keyring_entry(&self.service, key)
                        .map_err(|e| format!("keyring entry constructor {key}: {e}"))?;
                    match entry.delete_credential() {
                        Ok(()) | Err(keyring::Error::NoEntry) => {}
                        Err(e) if is_keyring_availability_error(&e.to_string()) => {
                            return Err(format!("keyring unavailable deleting {key}: {e}"));
                        }
                        Err(e) => {
                            return Err(format!("keyring per-key delete {key}: {e}"));
                        }
                    }
                }
            }
            // Step 2 (cont.): also delete the legacy DPK blob written by #1267.
            #[cfg(target_os = "macos")]
            {
                match delete_generic_password_options(dpk_opts(&self.service, BLOB_KEY)) {
                    Ok(()) => {}
                    Err(ref e) if is_not_found(e) => {}
                    Err(ref e) if is_dpk_unavailable(e) => {}
                    Err(e) => return Err(format!("dpk blob delete: {e}")),
                }
            }

            // Step 4: delete the main blob entry.
            {
                let entry = keyring_entry(&self.service, BLOB_KEY)
                    .map_err(|e| format!("keyring entry constructor blob: {e}"))?;
                match entry.delete_credential() {
                    Ok(()) | Err(keyring::Error::NoEntry) => {}
                    Err(e) if is_keyring_availability_error(&e.to_string()) => {
                        return Err(format!("keyring unavailable: {e}"));
                    }
                    Err(e) => {
                        return Err(format!("keyring blob delete: {e}"));
                    }
                }
            }

            // Step 5: clear the in-memory cache.
            let mut guard = self.cache.lock().unwrap_or_else(|e| e.into_inner());
            *guard = None;
            Ok(())
        }
        #[cfg(not(feature = "system-keyring"))]
        {
            Ok(()) // No-op: no keyring, nothing to delete.
        }
    }

    /// Verify no identity-bearing keychain entry survives in any shape
    /// that `load("identity")` → `migrate_legacy_key` can consume:
    /// main blob, DPK blob (`BLOB_KEY`), and per-key `"identity"`.
    ///
    /// Returns `true` when all three shapes are absent (or inaccessible in an
    /// expected way), `false` when any entry is found or the keychain is
    /// unavailable (fail-closed).
    pub fn verify_fully_wiped(&self) -> bool {
        #[cfg(feature = "system-keyring")]
        {
            // 1. Main blob must be absent.
            match self.read_blob_raw() {
                Ok(None) => {}
                Ok(Some(_)) => return false,
                Err(_) => return false,
            }
            // 2. Per-key "identity" via legacy keyring must be absent.
            match keyring_entry(&self.service, "identity") {
                Ok(entry) => match entry.get_password() {
                    Err(keyring::Error::NoEntry) => {}
                    Ok(_) => return false,
                    // Any other error (availability, unknown, transient) → fail closed.
                    // Only explicit NoEntry is proof of absence.
                    Err(_) => return false,
                },
                // Constructor failure → cannot verify → fail closed.
                Err(_) => return false,
            }
            // 3. DPK blob (macOS only).
            #[cfg(target_os = "macos")]
            {
                match generic_password(dpk_opts(&self.service, BLOB_KEY)) {
                    Err(ref e) if is_not_found(e) => {}
                    // dpk-unavailable is symmetric with load(): if load() can't
                    // consume DPK in this state, a surviving entry is harmless.
                    Err(ref e) if is_dpk_unavailable(e) => {}
                    Ok(_) => return false,
                    // Any other error → fail closed (not proof of absence).
                    Err(_) => return false,
                }
                // 4. Per-key DPK "identity" (macOS only).
                match generic_password(dpk_opts(&self.service, "identity")) {
                    Err(ref e) if is_not_found(e) => {}
                    // dpk-unavailable: symmetric with load() — if load() can't
                    // read DPK, a surviving entry can't resurrect identity.
                    Err(ref e) if is_dpk_unavailable(e) => {}
                    Ok(_) => return false,
                    // Any other error → fail closed.
                    Err(_) => return false,
                }
            }
            true
        }
        #[cfg(not(feature = "system-keyring"))]
        {
            true // No keyring = nothing to verify.
        }
    }
}

#[cfg(all(test, feature = "system-keyring"))]
mod tests {
    use super::*;

    // Test-only constructor: pre-seed the cache without touching the OS keychain.
    impl SecretStore {
        fn with_cache(service: &str, cache: Option<HashMap<String, String>>) -> Self {
            SecretStore {
                service: service.to_string(),
                cache: Mutex::new(cache),
            }
        }
    }

    #[test]
    fn probe_returns_present_when_key_in_cache() {
        let mut map = HashMap::new();
        map.insert("identity".to_string(), "nsec1test".to_string());
        let store = SecretStore::with_cache("buzz-test-cache-hit", Some(map));
        // Cache is warm and contains "identity" — probe must return Present
        // without touching the keychain.
        assert_eq!(store.probe("identity"), KeyringProbe::Present);
    }

    #[test]
    fn test_blob_lockfile_path_is_in_tmp_with_uid() {
        // The lockfile must be at a deterministic per-user path under /tmp —
        // invariant to $TMPDIR — so both a GUI-launched DMG (env-stripped by
        // launchd) and a terminal-launched dev build resolve the same inode and
        // achieve mutual exclusion.
        let path = blob_lockfile_path("buzz-desktop");
        #[cfg(unix)]
        {
            let uid = unsafe { libc::getuid() };
            assert!(
                path.starts_with("/tmp"),
                "lockfile {path:?} must start with /tmp (not $TMPDIR)"
            );
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default();
            assert!(
                name.contains(&uid.to_string()),
                "lockfile {path:?} must contain uid {uid}"
            );
            assert!(
                name.contains("buzz-keychain"),
                "lockfile name must contain 'buzz-keychain'"
            );
        }
        #[cfg(not(unix))]
        {
            assert!(
                path.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.contains("buzz-keychain")),
                "lockfile name must contain 'buzz-keychain'"
            );
        }
    }

    #[test]
    fn test_blob_lock_acquire_and_release() {
        // Verify the advisory lock can be acquired and released without errors.
        // This exercises the real flock/mutex path on the current platform.
        let guard = acquire_blob_lock("buzz-test-lock-smoke");
        assert!(
            guard.is_ok(),
            "advisory lock acquire must succeed: {:?}",
            guard.err()
        );
        // Drop the guard — lock is released. A second acquire must succeed.
        drop(guard);
        let guard2 = acquire_blob_lock("buzz-test-lock-smoke");
        assert!(
            guard2.is_ok(),
            "advisory lock re-acquire after release must succeed: {:?}",
            guard2.err()
        );
    }

    #[test]
    fn availability_error_discriminator() {
        assert!(is_keyring_availability_error("dbus connection failed"));
        assert!(is_keyring_availability_error(
            "org.freedesktop.secrets not provided"
        ));
        assert!(is_keyring_availability_error("No Secret Service"));
        assert!(is_keyring_availability_error(
            "Platform secure storage failure"
        ));
        // A plain "not found" is per-entry, not an availability failure.
        assert!(!is_keyring_availability_error("entry not found"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn dpk_error_discriminators() {
        // errSecMissingEntitlement = -34018 signals unsigned dev build.
        let e = SFError::from_code(-34018);
        assert!(is_dpk_unavailable(&e));
        assert!(!is_not_found(&e));
        // errSecItemNotFound = -25300 is not a DPK-unavailable error.
        let e = SFError::from_code(-25300);
        assert!(is_not_found(&e));
        assert!(!is_dpk_unavailable(&e));
    }

    // Integration tests that exercise the real OS keychain. Skipped in CI
    // (unsigned builds lack keychain entitlements); run locally with:
    //   cargo test -p buzz-desktop -- --ignored blob_
    //
    // Each test uses a unique service name to avoid cross-test pollution.

    #[ignore = "requires real OS keychain (run locally)"]
    #[test]
    fn blob_probe_present_absent_unreachable() {
        let store = SecretStore::keyring("buzz-test-blob-probe");
        // No blob yet — key absent, backend reachable.
        assert_eq!(store.probe("identity"), KeyringProbe::ReachableButEmpty);
        store.store("identity", "nsec1test").unwrap();
        // Key now present.
        assert_eq!(store.probe("identity"), KeyringProbe::Present);
        // Different key — blob exists but key absent.
        assert_eq!(store.probe("other"), KeyringProbe::ReachableButEmpty);
        // Cleanup.
    }

    #[ignore = "requires real OS keychain (run locally)"]
    #[test]
    fn delete_all_with_legacy_cleanup_removes_per_key_identity() {
        let svc = "buzz-test-delete-all-legacy";
        let key = "identity";
        let value = "nsec1legacytest";

        // Seed a legacy per-key entry (old format, pre-blob migration).
        let entry = keyring_entry(svc, key).unwrap();
        entry.set_password(value).unwrap();

        // Also seed a blob with a different key to exercise the full path.
        let store = SecretStore::keyring(svc);
        store.store("agent:abc123", "nsec1agent").unwrap();

        // Legacy per-key identity should be discoverable via probe.
        let store2 = SecretStore::keyring(svc);
        assert_eq!(store2.probe(key), KeyringProbe::Present);

        // Wipe everything via the sign-out path.
        store2.delete_all_with_legacy_cleanup().unwrap();

        // Fresh store — neither the blob nor the per-key entry should remain.
        let store3 = SecretStore::keyring(svc);
        assert_eq!(
            store3.probe(key),
            KeyringProbe::ReachableButEmpty,
            "per-key identity must not survive delete_all_with_legacy_cleanup"
        );
        assert_eq!(store3.probe(key), KeyringProbe::ReachableButEmpty);
        assert_eq!(
            store3.probe("agent:abc123"),
            KeyringProbe::ReachableButEmpty
        );
    }
}
