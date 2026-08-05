// Deterministic tests for the symphony sidecar supervisor.
//
// Included by `symphony.rs` as:
//   #[cfg(test)]
//   #[path = "symphony_tests.rs"]
//   mod tests;
//
// Zero real processes/sleeps/network: a FakeTime advances a synthetic Instant
// on sleep, a FakeProbe gates health, a FakeSpawner records the command and
// simulates the daemon writing its port/token artifacts on spawn.
#![allow(dead_code)]
use super::*;
use crate::local_stack::{DaemonProbe, Killable, OwnedChild, ProbeError, SidecarSpawner};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

// A realistic-looking bearer token used only to prove it never leaks via Debug.
const SENTINEL_TOKEN: &str = "deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef00";
const SYMPHONYD: &str = "x0x-symphonyd";

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    match m.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    }
}

// ── Fakes ───────────────────────────────────────────────────────────────────

#[derive(Clone)]
struct FakeTime {
    now: Arc<Mutex<Instant>>,
    sleeps: Arc<AtomicUsize>,
}
impl FakeTime {
    fn new() -> Self {
        Self {
            now: Arc::new(Mutex::new(Instant::now())),
            sleeps: Arc::new(AtomicUsize::new(0)),
        }
    }
}
impl TimeSource for FakeTime {
    fn now(&self) -> Instant {
        *lock(&self.now)
    }
    fn sleep(&self, dur: Duration) {
        *lock(&self.now) += dur;
        self.sleeps.fetch_add(1, Ordering::SeqCst);
    }
}

struct FakeKillable {
    label: &'static str,
    log: Arc<Mutex<Vec<&'static str>>>,
}
impl Killable for FakeKillable {
    fn label(&self) -> &'static str {
        self.label
    }
    fn kill_and_reap(&mut self) {
        lock(&self.log).push(self.label);
    }
}

/// Health gate: `Ok(())` iff `healthy` is set. The spawner flips it on spawn.
#[derive(Clone)]
struct FakeProbe {
    healthy: Arc<AtomicBool>,
    calls: Arc<AtomicUsize>,
}
impl DaemonProbe for FakeProbe {
    fn health(&self, _api_base: &str, _token: &str) -> Result<(), ProbeError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.healthy.load(Ordering::SeqCst) {
            Ok(())
        } else {
            Err(ProbeError::Unhealthy)
        }
    }
}

/// Records spawn commands, simulates the daemon writing `daemon.port` +
/// `api-token`, flips health on, and returns an OwnedChild whose reap is logged.
#[derive(Clone)]
struct FakeSpawner {
    args: Arc<Mutex<Vec<Vec<String>>>>,
    kill_log: Arc<Mutex<Vec<&'static str>>>,
    healthy: Arc<AtomicBool>,
}
impl SidecarSpawner for FakeSpawner {
    fn spawn(
        &self,
        cmd: &crate::local_stack::SidecarCommand,
    ) -> Result<OwnedChild, crate::local_stack::SpawnError> {
        lock(&self.args).push(cmd.args.clone());
        // Simulate the daemon writing its loopback port + token artifacts.
        if let Some(data_dir) = cmd.log_path.as_ref().and_then(|p| p.parent()) {
            let _ = std::fs::create_dir_all(data_dir);
            let _ = std::fs::write(data_dir.join("daemon.port"), "4321\n");
            let _ = std::fs::write(data_dir.join("api-token"), format!("{SENTINEL_TOKEN}\n"));
        }
        self.healthy.store(true, Ordering::SeqCst);
        Ok(OwnedChild::from_killable(Box::new(FakeKillable {
            label: SYMPHONYD,
            log: self.kill_log.clone(),
        })))
    }
}

struct Harness {
    _temp_dir: tempfile::TempDir,
    data_dir: std::path::PathBuf,
    healthy: Arc<AtomicBool>,
    probe_calls: Arc<AtomicUsize>,
    spawn_args: Arc<Mutex<Vec<Vec<String>>>>,
    kill_log: Arc<Mutex<Vec<&'static str>>>,
    time: FakeTime,
}

impl Harness {
    /// `healthy_initial` controls the attach-vs-spawn decision.
    fn new(healthy_initial: bool) -> Self {
        let dir = tempfile::TempDir::new().unwrap();
        let healthy = Arc::new(AtomicBool::new(healthy_initial));
        let probe_calls = Arc::new(AtomicUsize::new(0));
        let spawn_args = Arc::new(Mutex::new(Vec::new()));
        let kill_log = Arc::new(Mutex::new(Vec::new()));
        Self {
            data_dir: dir.path().to_path_buf(),
            _temp_dir: dir,
            healthy: healthy.clone(),
            probe_calls: probe_calls.clone(),
            spawn_args: spawn_args.clone(),
            kill_log: kill_log.clone(),
            time: FakeTime::new(),
        }
    }

    fn supervisor(&self) -> SymphonySupervisor<FakeProbe, FakeSpawner, FakeTime> {
        let cfg = SymphonyConfig::for_test(
            self.data_dir.clone(),
            std::path::PathBuf::from("/fake/x0x-symphonyd"),
        );
        let probe = FakeProbe {
            healthy: self.healthy.clone(),
            calls: self.probe_calls.clone(),
        };
        let spawner = FakeSpawner {
            args: self.spawn_args.clone(),
            kill_log: self.kill_log.clone(),
            healthy: self.healthy.clone(),
        };
        SymphonySupervisor::new(cfg, probe, spawner, self.time.clone())
    }

    fn write_artifacts(&self, port: &str) {
        std::fs::write(self.data_dir.join("daemon.port"), format!("{port}\n")).unwrap();
        std::fs::write(
            self.data_dir.join("api-token"),
            format!("{SENTINEL_TOKEN}\n"),
        )
        .unwrap();
    }
}

// ── Artifact reads ──────────────────────────────────────────────────────────

#[test]
fn read_symphony_port_parses_bare_port() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("daemon.port"), b"4321\n").unwrap();
    assert_eq!(read_symphony_port(dir.path()), Some(4321));
}

#[test]
fn read_symphony_port_rejects_zero_and_garbage() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("daemon.port"), b"0\n").unwrap();
    assert_eq!(read_symphony_port(dir.path()), None);
    std::fs::write(dir.path().join("daemon.port"), b"not-a-port").unwrap();
    assert_eq!(read_symphony_port(dir.path()), None);
}

#[test]
fn read_symphony_port_none_when_missing() {
    let dir = tempfile::TempDir::new().unwrap();
    assert_eq!(read_symphony_port(dir.path()), None);
}

// ── Supervisor decision logic ───────────────────────────────────────────────

#[test]
fn attaches_when_healthy_and_artifacts_present() {
    // Daemon already up: artifacts present + health Ok → attach, no owned child.
    let h = Harness::new(true);
    h.write_artifacts("4321");
    let supervisor = h.supervisor();
    let handle = supervisor
        .bring_up(std::path::Path::new("/cfg/WORKFLOW.md"))
        .expect("attach bring_up");
    assert!(handle.child.is_none(), "attached daemon must not be owned");
    assert_eq!(handle.base_url, "http://127.0.0.1:4321");
    // No spawn happened.
    assert!(lock(&h.spawn_args).is_empty());
}

#[test]
fn spawns_when_unhealthy_then_becomes_ready() {
    // No artifacts + unhealthy → spawn → spawner writes artifacts + flips health
    // → wait_ready returns.
    let h = Harness::new(false);
    let supervisor = h.supervisor();
    let handle = supervisor
        .bring_up(std::path::Path::new("/cfg/WORKFLOW.md"))
        .expect("spawn bring_up");
    assert!(handle.child.is_some(), "spawned daemon must be owned");
    assert_eq!(handle.base_url, "http://127.0.0.1:4321");
    assert_eq!(lock(&h.spawn_args).len(), 1);
}

#[test]
fn times_out_when_never_healthy() {
    // A spawner that never writes artifacts/never flips health → bounded timeout.
    let dir = tempfile::TempDir::new().unwrap();
    let healthy = Arc::new(AtomicBool::new(false));
    let cfg = SymphonyConfig::for_test(
        dir.path().to_path_buf(),
        std::path::PathBuf::from("/fake/x0x-symphonyd"),
    );
    let probe = FakeProbe {
        healthy: healthy.clone(),
        calls: Arc::new(AtomicUsize::new(0)),
    };
    let spawner = NoArtifactSpawner;
    let time = FakeTime::new();
    let supervisor = SymphonySupervisor::new(cfg, probe, spawner, time);
    let err = supervisor
        .bring_up(std::path::Path::new("/cfg/WORKFLOW.md"))
        .expect_err("must time out");
    assert!(matches!(
        err,
        SymphonyError::Timeout {
            stage: SymphonyStage::Health
        }
    ));
}

#[derive(Clone)]
struct NoArtifactSpawner;
impl SidecarSpawner for NoArtifactSpawner {
    fn spawn(
        &self,
        _cmd: &crate::local_stack::SidecarCommand,
    ) -> Result<OwnedChild, crate::local_stack::SpawnError> {
        // Never writes artifacts and never flips health → readiness never met.
        Ok(OwnedChild::from_killable(Box::new(FakeKillable {
            label: SYMPHONYD,
            log: Arc::new(Mutex::new(Vec::new())),
        })))
    }
}

// ── Spawn args (loopback-only bind) ─────────────────────────────────────────

#[test]
fn spawn_args_bind_loopback_ephemeral_zero() {
    let h = Harness::new(false);
    let supervisor = h.supervisor();
    supervisor
        .bring_up(std::path::Path::new("/cfg/WORKFLOW.md"))
        .ok();
    let args = lock(&h.spawn_args).first().expect("one spawn").clone();
    let bind = args
        .windows(2)
        .find(|w| w[0] == "--bind")
        .map(|w| w[1].as_str())
        .expect("--bind present");
    assert_eq!(bind, "127.0.0.1:0", "bind must be loopback ephemeral");
    // Config + data-dir are forwarded.
    assert!(args.iter().any(|a| a == "--config"));
    assert!(args.iter().any(|a| a == "--data-dir"));
    assert!(args.iter().any(|a| a == "/cfg/WORKFLOW.md"));
}

// ── Handle shutdown + token secrecy ─────────────────────────────────────────

#[test]
fn shutdown_reaps_owned_child_once() {
    let h = Harness::new(false);
    let supervisor = h.supervisor();
    let mut handle = supervisor
        .bring_up(std::path::Path::new("/cfg/WORKFLOW.md"))
        .unwrap();
    assert!(handle.owns_child());
    handle.shutdown();
    handle.shutdown(); // idempotent
    assert!(!handle.owns_child());
    assert_eq!(*lock(&h.kill_log), vec![SYMPHONYD]);
}

#[test]
fn handle_debug_never_contains_token() {
    let h = Harness::new(false);
    let supervisor = h.supervisor();
    let mut handle = supervisor
        .bring_up(std::path::Path::new("/cfg/WORKFLOW.md"))
        .unwrap();
    let debug = format!("{handle:?}");
    assert!(
        !debug.contains(SENTINEL_TOKEN),
        "token leaked into handle Debug: {debug}"
    );
    handle.shutdown();
}

#[test]
fn error_display_carries_no_token() {
    let err = SymphonyError::Timeout {
        stage: SymphonyStage::Health,
    };
    let msg = format!("{err}");
    assert!(!msg.contains(SENTINEL_TOKEN));
    assert!(msg.contains("health"));
}
