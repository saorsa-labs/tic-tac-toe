//! symphony — supervised loopback sidecar for `x0x-symphonyd`.
//!
//! Owns the x0x-symphony daemon lifecycle so the desktop can drive agent work
//! orchestration (tasks/claims/handoffs/proofs/approvals) over a loopback
//! REST + SSE API. This is the M4 vertical slice: the desktop supervises a
//! *local* symphony daemon and exposes typed Tauri commands; it never emits a
//! Nostr relay event and never implements crypto (the daemon owns both).
//!
//! ## Lifecycle (mirrors `local_stack`)
//! On [`bring_up_symphony`]:
//! 1. **Attach** to a healthy named daemon when its artifacts
//!    (`daemon.port` loopback-valid + nonempty `api-token`) and a bearer
//!    `/health` say OK — without taking ownership.
//! 2. Else **spawn** `x0x-symphonyd --config <path> --data-dir <dir>
//!    --bind 127.0.0.1:0`, own the child, and bounded-poll the data dir for
//!    artifacts + health.
//!
//! The daemon writes the resolved ephemeral port to `<data-dir>/daemon.port`
//! and a 32-byte bearer token to `<data-dir>/api-token`. The token is
//! **transient**: read → used for `/health` → dropped. It is never stored in
//! [`crate::app_state::AppState`] or [`SymphonyHandle`], never logged, and
//! never appears in any `Debug`/error/serialized output.
//!
//! Shutdown reaps **only app-owned** children (an attached daemon is `None`),
//! is idempotent, and runs before the x0xd daemon is reaped (symphony depends
//! on x0xd for signing identity).

use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::Value;

use crate::local_stack::{
    http_get_json, loopback_api_base, read_api_token, resolve_sidecar, BlockingTimeSource,
    DaemonProbe, OwnedChild, ProbeError, SidecarCommand, SidecarSpawner, SpawnError,
    StdSidecarSpawner, TimeSource,
};

// ── Constants ───────────────────────────────────────────────────────────────

/// Named data-dir suffix for the supervised symphony daemon (sibling to the
/// `x0x-ttt` daemon dir): `<data_dir>/x0x-symphony-ttt`.
const SYMPHONY_DIR_NAME: &str = "x0x-symphony-ttt";
/// File the daemon writes the resolved ephemeral loopback port into (bare u16).
const PORT_FILE: &str = "daemon.port";
/// Durable record of the WORKFLOW.md config path an OWNED daemon was spawned
/// against, written so a later attach can PROVE the running daemon's config
/// matches the requested one (defending rebind/reconcile against silently
/// re-claiming a warm daemon started under a different config).
const CONFIG_PATH_FILE: &str = "config.path";

const POLL_INTERVAL: Duration = Duration::from_millis(200);
const DEFAULT_SYMPHONY_TIMEOUT: Duration = Duration::from_secs(12);

/// Env override for the `x0x-symphonyd` binary path (dev). Must be executable.
pub(crate) const SYMPHONYD_BINARY_ENV: &str = "TTT_SYMPHONYD_BINARY";
const SYMPHONYD_BINARY_NAME: &str = "x0x-symphonyd";

// ── Named data dir + artifact reads (pure, panic-free) ──────────────────────

/// `<data_dir>/x0x-symphony-ttt`, the supervised daemon's artifact directory.
pub(crate) fn symphony_data_dir() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join(SYMPHONY_DIR_NAME))
}

/// Read `<data_dir>/daemon.port` (bare port text) and return the port iff it
/// parses as a nonzero `u16`. Missing, non-UTF-8, empty, zero, or malformed →
/// `None`. Never panics. (Unlike x0xd's `host:port` `api.port`, symphony writes
/// a bare port because the daemon always binds `127.0.0.1`.)
pub(crate) fn read_symphony_port(data_dir: &Path) -> Option<u16> {
    let raw = std::fs::read_to_string(data_dir.join(PORT_FILE)).ok()?;
    let port = raw.trim().parse::<u16>().ok()?;
    (port != 0).then_some(port)
}

/// Read the durable config-path artifact the spawner wrote when it last spawned
/// an owned daemon against this data dir. `None` if missing/unreadable/empty.
fn read_config_path_artifact(data_dir: &Path) -> Option<PathBuf> {
    let raw = std::fs::read_to_string(data_dir.join(CONFIG_PATH_FILE)).ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(PathBuf::from(trimmed))
    }
}

/// Whether two paths refer to the same file. Canonicalizes both (so symlinks /
/// relative paths don't cause false mismatches); falls back to a raw compare
/// when canonicalization is unavailable.
fn paths_match(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(ca), Ok(cb)) => ca == cb,
        _ => a == b,
    }
}

/// Whether the running daemon's RECORDED config matches the requested one.
/// `false` when no artifact exists (attach is then UNPROVEN — the caller must
/// fail closed or spawn, never silently re-claim the daemon).
fn config_path_proven(data_dir: &Path, requested: &Path) -> bool {
    read_config_path_artifact(data_dir)
        .map(|recorded| paths_match(&recorded, requested))
        .unwrap_or(false)
}

/// Record the config path an owned daemon was spawned against, so a later
/// attach can prove the running daemon's config. Best-effort: a missing
/// artifact simply makes a future attach unproven (→ spawn or fail closed).
fn write_config_path_artifact(data_dir: &Path, config_path: &Path) {
    let _ = std::fs::write(
        data_dir.join(CONFIG_PATH_FILE),
        config_path.to_string_lossy().as_bytes(),
    );
}

// ── Errors ──────────────────────────────────────────────────────────────────

/// Which readiness stage a bounded poll is waiting on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SymphonyStage {
    Health,
}

/// Supervisor error. Carries only stage labels and safe context (binary paths,
/// OS error strings) — **never** the daemon token.
#[derive(Debug)]
pub(crate) enum SymphonyError {
    NoDataDir,
    /// The supplied `--config` path does not exist or is not a regular file.
    NoConfig,
    SpawnFailed {
        sidecar: &'static str,
        reason: String,
    },
    Timeout {
        stage: SymphonyStage,
    },
    InvalidOverride {
        which: &'static str,
        reason: String,
    },
    /// A healthy daemon is attached but provably running a DIFFERENT config than
    /// requested. We do not own it (cannot rebind or respawn on the shared data
    /// dir), so the bind fails closed rather than mislabeling an incompatible
    /// daemon. `running` is the recorded config (if any); `requested` is what
    /// the caller asked to bind.
    IncompatibleAttachedConfig {
        running: Option<PathBuf>,
        requested: PathBuf,
    },
}

impl SymphonyError {
    fn from_spawn(e: SpawnError) -> Self {
        match e {
            SpawnError::NotFound(s) => Self::SpawnFailed {
                sidecar: s,
                reason:
                    "binary not found adjacent to the app; set the env override or stage sidecars"
                        .to_string(),
            },
            SpawnError::System(s, r) => Self::SpawnFailed {
                sidecar: s,
                reason: r,
            },
            SpawnError::Invalid(s, r) => Self::InvalidOverride {
                which: s,
                reason: r,
            },
        }
    }
}

impl fmt::Display for SymphonyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoDataDir => write!(f, "could not resolve the symphony data directory"),
            Self::NoConfig => write!(
                f,
                "symphony config file is missing; generate a WORKFLOW.md before starting"
            ),
            Self::SpawnFailed { sidecar, reason } => {
                write!(f, "{sidecar} spawn failed: {reason}")
            }
            Self::Timeout { stage } => match stage {
                SymphonyStage::Health => write!(f, "timed out waiting for symphony daemon health"),
            },
            Self::InvalidOverride { which, reason } => {
                write!(f, "invalid {which} binary override: {reason}")
            }
            Self::IncompatibleAttachedConfig { running, requested } => {
                let running_disp = running
                    .as_deref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "<unknown>".to_string());
                write!(
                    f,
                    "a healthy symphony daemon is already running an incompatible config; requested `{}` but the running daemon was started against `{}` (stop the other daemon or let its owner rebind)",
                    requested.display(),
                    running_disp
                )
            }
        }
    }
}

// ── Owned handle ────────────────────────────────────────────────────────────

/// The supervisor's output: an owned child (`None` = attached, never killed),
/// the resolved loopback base URL, and the data dir (so the transient token can
/// be re-read per client call). Carries NO token.
pub(crate) struct SymphonyHandle {
    pub(crate) child: Option<OwnedChild>,
    pub(crate) base_url: String,
    pub(crate) data_dir: PathBuf,
    /// The WORKFLOW.md config path this daemon was brought up against. Used to
    /// detect that a second Company instance needs a rebind (different config
    /// → shut down the owned child and re-bring-up against the new config).
    pub(crate) config_path: PathBuf,
}

impl SymphonyHandle {
    /// Reap the owned child if any. Idempotent via `take()`.
    pub(crate) fn shutdown(&mut self) {
        if let Some(mut child) = self.child.take() {
            child.shutdown();
        }
    }

    /// Whether this handle owns (and is responsible for killing) the daemon.
    pub(crate) fn owns_child(&self) -> bool {
        self.child.is_some()
    }
}

impl fmt::Debug for SymphonyHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // No token exists on the handle; base URL + ownership only.
        f.debug_struct("SymphonyHandle")
            .field("base_url", &self.base_url)
            .field("data_dir", &self.data_dir)
            .field("config_path", &self.config_path)
            .field("owned", &self.owns_child())
            .finish()
    }
}

// ── Config ──────────────────────────────────────────────────────────────────

pub(crate) struct SymphonyConfig {
    pub(crate) data_dir: PathBuf,
    pub(crate) binary: PathBuf,
    pub(crate) timeout: Duration,
    /// Owner x0xd bearer passed transiently to the child process. Never stored
    /// on [`SymphonyHandle`] and redacted from all diagnostics.
    x0xd_api_token: Option<String>,
}

impl SymphonyConfig {
    pub(crate) fn resolve() -> Result<Self, SymphonyError> {
        let data_dir = symphony_data_dir().ok_or(SymphonyError::NoDataDir)?;
        let binary = resolve_sidecar(SYMPHONYD_BINARY_NAME, SYMPHONYD_BINARY_ENV)
            .map_err(SymphonyError::from_spawn)?;
        let x0xd_api_token =
            crate::local_stack::named_data_dir().and_then(|dir| read_api_token(&dir));
        Ok(Self {
            data_dir,
            binary,
            timeout: DEFAULT_SYMPHONY_TIMEOUT,
            x0xd_api_token,
        })
    }

    /// Test constructor: explicit paths (fakes ignore the binaries).
    #[cfg(test)]
    #[allow(dead_code)]
    pub(crate) fn for_test(data_dir: PathBuf, binary: PathBuf) -> Self {
        Self {
            data_dir,
            binary,
            timeout: DEFAULT_SYMPHONY_TIMEOUT,
            x0xd_api_token: None,
        }
    }
}

// ── Supervisor ──────────────────────────────────────────────────────────────

pub(crate) struct SymphonySupervisor<P: DaemonProbe, S: SidecarSpawner, T: TimeSource> {
    cfg: SymphonyConfig,
    probe: P,
    spawner: S,
    time: T,
}

impl<P: DaemonProbe, S: SidecarSpawner, T: TimeSource> SymphonySupervisor<P, S, T> {
    pub(crate) fn new(cfg: SymphonyConfig, probe: P, spawner: S, time: T) -> Self {
        Self {
            cfg,
            probe,
            spawner,
            time,
        }
    }

    /// Bring up the symphony daemon against `config_path`. Returns the handle
    /// owning any spawned child and the resolved loopback base URL. The daemon
    /// token is dropped on return.
    pub(crate) fn bring_up(&self, config_path: &Path) -> Result<SymphonyHandle, SymphonyError> {
        let data_dir = self.cfg.data_dir.clone();

        // 1. Attach to a healthy named daemon ONLY if its config is proven to
        //    match `config_path`; otherwise spawn and own one.
        let (child, base_url) = match self.try_attach(&data_dir, config_path)? {
            Some(base_url) => (None, base_url),
            None => {
                let child = self.spawn_daemon(config_path)?;
                let base_url = self.wait_ready(&data_dir)?;
                // Record the durable config proof so a later attach (e.g. a
                // restart reconcile) can verify the running daemon's config.
                write_config_path_artifact(&data_dir, config_path);
                (Some(child), base_url)
            }
        };

        Ok(SymphonyHandle {
            child,
            base_url,
            data_dir,
            config_path: config_path.to_path_buf(),
        })
    }

    /// Attach when `daemon.port` + `api-token` are present and bearer `/health`
    /// is OK, AND the running daemon's config is PROVEN to match `config_path`.
    /// Returns the base URL (`Some`), `None` to spawn instead, or `Err` when a
    /// healthy daemon is provably running a different config (fail closed: we
    /// do not own it, so we can neither rebind nor respawn on the shared data
    /// dir — preserving attached-process ownership safety).
    fn try_attach(
        &self,
        data_dir: &Path,
        config_path: &Path,
    ) -> Result<Option<String>, SymphonyError> {
        let Some(port) = read_symphony_port(data_dir) else {
            return Ok(None);
        };
        let Some(token) = read_api_token(data_dir) else {
            return Ok(None);
        };
        let base_url = loopback_api_base(port);
        match self.probe.health(&base_url, &token) {
            Ok(()) => {
                if config_path_proven(data_dir, config_path) {
                    Ok(Some(base_url))
                } else {
                    Err(SymphonyError::IncompatibleAttachedConfig {
                        running: read_config_path_artifact(data_dir),
                        requested: config_path.to_path_buf(),
                    })
                }
            }
            Err(_) => Ok(None),
        }
    }

    fn spawn_daemon(&self, config_path: &Path) -> Result<OwnedChild, SymphonyError> {
        let cmd = SidecarCommand {
            label: "x0x-symphonyd",
            binary: self.cfg.binary.clone(),
            args: vec![
                "--config".to_string(),
                config_path.to_string_lossy().into_owned(),
                "--data-dir".to_string(),
                self.cfg.data_dir.to_string_lossy().into_owned(),
                // Ephemeral loopback bind; the daemon writes the resolved port
                // to `<data-dir>/daemon.port`. Server-enforced loopback only.
                "--bind".to_string(),
                "127.0.0.1:0".to_string(),
            ],
            env: self
                .cfg
                .x0xd_api_token
                .as_ref()
                .map(|token| vec![("X0X_API_TOKEN".to_string(), token.clone())])
                .unwrap_or_default(),
            log_path: Some(self.cfg.data_dir.join("symphonyd.log")),
        };
        self.spawner.spawn(&cmd).map_err(SymphonyError::from_spawn)
    }

    /// Bounded-poll the data dir for port + token artifacts and bearer health.
    fn wait_ready(&self, data_dir: &Path) -> Result<String, SymphonyError> {
        let deadline = self.time.now() + self.cfg.timeout;
        loop {
            if let (Some(port), Some(token)) =
                (read_symphony_port(data_dir), read_api_token(data_dir))
            {
                let base_url = loopback_api_base(port);
                if self.probe.health(&base_url, &token).is_ok() {
                    return Ok(base_url);
                }
            }
            if self.time.now() >= deadline {
                return Err(SymphonyError::Timeout {
                    stage: SymphonyStage::Health,
                });
            }
            self.time.sleep(POLL_INTERVAL);
        }
    }
}

// ── Concrete health probe ───────────────────────────────────────────────────

/// Bearer-authenticated `GET /health` for the symphony daemon. Unlike x0xd's
/// `{"ok":true}`, symphony returns `{"status":"ok"}`.
struct LoopbackSymphonyProbe;

impl DaemonProbe for LoopbackSymphonyProbe {
    fn health(&self, api_base: &str, token: &str) -> Result<(), ProbeError> {
        let url = format!("{}/health", api_base.trim_end_matches('/'));
        let doc: Value = http_get_json(&url, Some(token))?;
        match doc.get("status").and_then(|v| v.as_str()) {
            Some("ok") => Ok(()),
            _ => Err(ProbeError::Unhealthy),
        }
    }
}

// ── App integration entrypoints ─────────────────────────────────────────────

/// Bring up the supervised symphony daemon during/after app setup, bound to the
/// supplied `WORKFLOW.md` config path. Best-effort and synchronous: on success
/// stores the handle in [`AppState`]; on failure captures the typed error.
///
/// If a daemon is already supervised against the SAME config path, this is a
/// no-op (idempotent resume). If supervised against a DIFFERENT config path
/// (a second Company instance), the owned child is shut down and re-bound to
/// the new config. An attached (not-owned) daemon cannot be rebound and is
/// left as-is. The token never reaches `AppState`.
pub(crate) fn bring_up_symphony(app: &tauri::AppHandle, config_path: &Path) {
    use tauri::Manager;
    let state = app.state::<crate::app_state::AppState>();

    // Same config → idempotent resume. Different config → rebind.
    let needs_rebind = state
        .local_symphony
        .lock()
        .map(|guard| {
            guard
                .as_ref()
                .is_none_or(|handle| handle.config_path != config_path)
        })
        .unwrap_or(true);

    if !needs_rebind {
        return;
    }

    // Different config or no handle: shut down any existing owned child first.
    shutdown_symphony_state(&state);

    if !config_path.is_file() {
        record_error(&state, SymphonyError::NoConfig.to_string());
        return;
    }

    let cfg = match SymphonyConfig::resolve() {
        Ok(c) => c,
        Err(e) => {
            record_error(&state, e.to_string());
            return;
        }
    };

    // Ensure the artifact dir exists for port/token/log files.
    let _ = std::fs::create_dir_all(&cfg.data_dir);

    let supervisor = SymphonySupervisor::new(
        cfg,
        LoopbackSymphonyProbe,
        StdSidecarSpawner,
        BlockingTimeSource,
    );
    match supervisor.bring_up(config_path) {
        Ok(handle) => {
            eprintln!("symphony: ready at {}", handle.base_url);
            match state.local_symphony.lock() {
                Ok(mut guard) => *guard = Some(handle),
                Err(poisoned) => *poisoned.into_inner() = Some(handle),
            }
        }
        Err(e) => {
            let msg = e.to_string();
            eprintln!("symphony: bring-up failed: {msg}");
            record_error(&state, msg);
        }
    }
}

/// State-level shutdown (testable without an `AppHandle`): recovers a poisoned
/// mutex, takes the handle (idempotent), and reaps the owned child.
pub(crate) fn shutdown_symphony_state(state: &crate::app_state::AppState) {
    let mut guard = state
        .local_symphony
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let Some(mut handle) = guard.take() else {
        return;
    };
    handle.shutdown();
}

/// Reap the app-owned symphony child. Idempotent and poison-safe; an attached
/// daemon (`None`) is never touched. Called from the Tauri exit path and the
/// signal handler in `shutdown.rs`, before the x0xd daemon is reaped.
pub(crate) fn shutdown_symphony_owned(app: &tauri::AppHandle) {
    use tauri::Manager;
    shutdown_symphony_state(&app.state::<crate::app_state::AppState>());
}

fn record_error(state: &crate::app_state::AppState, msg: String) {
    eprintln!("symphony: {msg}");
    match state.symphony_error.lock() {
        Ok(mut guard) => *guard = Some(msg),
        Err(poisoned) => *poisoned.into_inner() = Some(msg),
    }
}

#[cfg(test)]
#[path = "symphony_tests.rs"]
mod tests;
