//! Test-only injection for the Company instances root, so the
//! single-active reservation + reconcile-skip logic can be exercised
//! hermetically against a temp dir instead of the real
//! `<data_dir>/x0x-company-ttt`. Production builds compile this out.

use std::path::PathBuf;
use std::sync::{LazyLock, Mutex};

static ROOT: LazyLock<Mutex<Option<PathBuf>>> = LazyLock::new(|| Mutex::new(None));

/// Serializes every `ScopedRoot` so two override-using tests never overlap
/// the process-wide root under parallel test execution. Held for the
/// `ScopedRoot`'s lifetime.
static SERIALIZE: Mutex<()> = Mutex::new(());

/// The currently-injected root, if any. `None` → production path.
pub(super) fn current_root() -> Option<PathBuf> {
    ROOT.lock().ok().and_then(|guard| guard.clone())
}

/// RAII scope: points Company instance resolution at a fresh temp dir until
/// dropped, then restores the prior root.
pub(super) struct ScopedRoot {
    #[allow(dead_code)] // held for its Drop, which reaps the temp tree
    dir: Option<tempfile::TempDir>,
    prev: Option<PathBuf>,
    // Held until drop: keeps overlapping tests out of the override.
    _guard: std::sync::MutexGuard<'static, ()>,
}

impl ScopedRoot {
    pub(super) fn new() -> Self {
        // `SERIALIZE` is static, so the guard borrows for 'static and can
        // be stored in the returned struct.
        let _guard = SERIALIZE.lock().unwrap_or_else(|p| p.into_inner());
        let prev = current_root();
        let dir = tempfile::TempDir::new().expect("temp instances root");
        if let Ok(mut cell) = ROOT.lock() {
            *cell = Some(dir.path().to_path_buf());
        }
        Self {
            dir: Some(dir),
            prev,
            _guard,
        }
    }
}

impl Drop for ScopedRoot {
    fn drop(&mut self) {
        // Restore the root before the temp tree is reaped and the
        // serialization guard releases.
        if let Ok(mut cell) = ROOT.lock() {
            *cell = self.prev.take();
        }
    }
}
