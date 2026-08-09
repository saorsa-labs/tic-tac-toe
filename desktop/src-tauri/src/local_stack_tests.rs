// Deterministic tests for the native local-stack supervisor (M1a spawn-or-attach).
//
// Included by `local_stack.rs` as:
//   #[cfg(test)]
//   #[path = "local_stack_tests.rs"]
//   mod tests;
// so `use super::*` resolves to `crate::local_stack`'s `pub(crate)` items and
// private module consts (INSTANCE_NAME, POLL_INTERVAL, DEFAULT_DAEMON_TIMEOUT,
// X0XD_BINARY_ENV).
//
// Design: zero real processes/sleeps/network in the deterministic suite.
//  - FakeTime advances a synthetic Instant on sleep → readiness polls hit
//    deadlines instantly and deterministically.
//  - FakeKillable records reaps into a shared ordered log → kill order,
//    idempotency, owned-vs-attached, and Drop semantics are observable.
//  - Fakes driven by Arc<Atomic*> / Arc<Mutex<_>> plan flags.
#![allow(dead_code)]
use super::*;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const DAEMON: &str = "x0xd";
// A realistic-looking 64-hex bearer token used only to prove it never leaks.
const SENTINEL_TOKEN: &str = "deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef00";

// ── std::sync::Mutex poison-recovering lock helper ─────────────────────────
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
}
impl FakeTime {
    fn new() -> Self {
        Self {
            now: Arc::new(Mutex::new(Instant::now())),
        }
    }
}
impl TimeSource for FakeTime {
    fn now(&self) -> Instant {
        *lock(&self.now)
    }
    fn sleep(&self, dur: Duration) {
        *lock(&self.now) += dur;
    }
}

/// Recording reap target. Appends to a shared ordered log on kill_and_reap.
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

/// Daemon health gate. `Ok(())` iff `booted` is set — the spawner flips it.
#[derive(Clone)]
struct FakeDaemonProbe {
    booted: Arc<AtomicBool>,
    health_calls: Arc<AtomicUsize>,
}
impl DaemonProbe for FakeDaemonProbe {
    fn health(&self, _api_base: &str, _token: &str) -> Result<(), ProbeError> {
        self.health_calls.fetch_add(1, Ordering::SeqCst);
        if self.booted.load(Ordering::SeqCst) {
            Ok(())
        } else {
            Err(ProbeError::Unhealthy)
        }
    }
}

#[derive(Clone)]
struct SpawnPlan {
    write_artifacts: bool,
    boot_daemon: bool,
    fail: bool,
    port: u16,
    token: String,
}
impl Default for SpawnPlan {
    fn default() -> Self {
        Self {
            write_artifacts: true,
            boot_daemon: true,
            fail: false,
            port: 4847,
            token: SENTINEL_TOKEN.to_string(),
        }
    }
}

#[derive(Clone)]
struct FakeSidecarSpawner {
    data_dir: PathBuf,
    plan: Arc<Mutex<SpawnPlan>>,
    booted: Arc<AtomicBool>,
    spawn_calls: Arc<AtomicUsize>,
    spawned_args: Arc<Mutex<Vec<Vec<String>>>>,
    kill_log: Arc<Mutex<Vec<&'static str>>>,
}
impl FakeSidecarSpawner {
    fn new(
        data_dir: PathBuf,
        booted: Arc<AtomicBool>,
        kill_log: Arc<Mutex<Vec<&'static str>>>,
    ) -> Self {
        Self {
            data_dir,
            plan: Arc::new(Mutex::new(SpawnPlan::default())),
            booted,
            spawn_calls: Arc::new(AtomicUsize::new(0)),
            spawned_args: Arc::new(Mutex::new(Vec::new())),
            kill_log,
        }
    }
}
impl SidecarSpawner for FakeSidecarSpawner {
    fn spawn(&self, cmd: &SidecarCommand) -> Result<OwnedChild, SpawnError> {
        self.spawn_calls.fetch_add(1, Ordering::SeqCst);
        lock(&self.spawned_args).push(cmd.args.clone());
        let plan = lock(&self.plan).clone();
        if plan.fail {
            return Err(SpawnError::System(
                DAEMON,
                "spawn denied by fake".to_string(),
            ));
        }
        if plan.write_artifacts {
            let _ = std::fs::write(
                self.data_dir.join("api.port"),
                format!("127.0.0.1:{}", plan.port),
            );
            let _ = std::fs::write(self.data_dir.join("api-token"), plan.token.as_bytes());
        }
        if plan.boot_daemon {
            self.booted.store(true, Ordering::SeqCst);
        }
        Ok(OwnedChild::from_killable(Box::new(FakeKillable {
            label: DAEMON,
            log: self.kill_log.clone(),
        })))
    }
}

// ── Harness ─────────────────────────────────────────────────────────────────

struct Fakes {
    _dir: tempfile::TempDir,
    data_dir: PathBuf,
    time: FakeTime,
    probe: FakeDaemonProbe,
    spawner: FakeSidecarSpawner,
    kill_log: Arc<Mutex<Vec<&'static str>>>,
    booted: Arc<AtomicBool>,
}

fn write_artifacts(dir: &std::path::Path, port: u16, token: &str) {
    std::fs::write(dir.join("api.port"), format!("127.0.0.1:{port}\n")).unwrap();
    std::fs::write(dir.join("api-token"), token).unwrap();
}

fn harness(booted_initially: bool) -> Fakes {
    let dir = tempfile::TempDir::new().unwrap();
    let data_dir = dir.path().to_path_buf();
    let time = FakeTime::new();
    let booted = Arc::new(AtomicBool::new(booted_initially));
    let kill_log = Arc::new(Mutex::new(Vec::new()));
    let probe = FakeDaemonProbe {
        booted: booted.clone(),
        health_calls: Arc::new(AtomicUsize::new(0)),
    };
    let spawner = FakeSidecarSpawner::new(data_dir.clone(), booted.clone(), kill_log.clone());
    Fakes {
        _dir: dir,
        data_dir,
        time,
        probe,
        spawner,
        kill_log,
        booted,
    }
}

fn supervisor(f: &Fakes) -> LocalStackSupervisor<FakeDaemonProbe, FakeSidecarSpawner, FakeTime> {
    let cfg = StackConfig {
        data_dir: f.data_dir.clone(),
        x0xd_binary: PathBuf::from("/nonexistent/x0xd"),
        daemon_timeout: DEFAULT_DAEMON_TIMEOUT,
    };
    LocalStackSupervisor::new(cfg, f.probe.clone(), f.spawner.clone(), f.time.clone())
}

fn snapshot(log: &Arc<Mutex<Vec<&'static str>>>) -> Vec<&'static str> {
    lock(log).clone()
}

fn err_of(r: Result<LocalStackHandle, LocalDaemonError>, msg: &str) -> LocalDaemonError {
    match r {
        Err(e) => e,
        Ok(_) => panic!("{msg}"),
    }
}

/// A handle that owns a single (fake) daemon child — the native shape, no bridge.
fn owned_handle(f: &Fakes) -> LocalStackHandle {
    LocalStackHandle {
        daemon: Some(OwnedChild::from_killable(Box::new(FakeKillable {
            label: DAEMON,
            log: f.kill_log.clone(),
        }))),
        api_base: loopback_api_base(4847),
        data_dir: f.data_dir.clone(),
    }
}

// ── Artifact reads (pure parsing boundaries) ────────────────────────────────

#[test]
fn read_api_port_parses_loopback_address() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("api.port"), "127.0.0.1:8080\n").unwrap();
    assert_eq!(read_api_port(dir.path()), Some(8080));
    let d2 = tempfile::TempDir::new().unwrap();
    std::fs::write(d2.path().join("api.port"), "localhost:9090").unwrap();
    assert_eq!(read_api_port(d2.path()), Some(9090));
}

#[test]
fn read_api_port_rejects_non_numeric_port() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("api.port"), "127.0.0.1:not-a-port").unwrap();
    assert_eq!(read_api_port(dir.path()), None);
}

#[test]
fn read_api_port_rejects_empty_and_missing_and_no_colon() {
    let a = tempfile::TempDir::new().unwrap();
    std::fs::write(a.path().join("api.port"), "   \n").unwrap();
    assert_eq!(read_api_port(a.path()), None);
    let b = tempfile::TempDir::new().unwrap();
    assert_eq!(read_api_port(b.path()), None);
    let c = tempfile::TempDir::new().unwrap();
    std::fs::write(c.path().join("api.port"), "localhost8080").unwrap();
    assert_eq!(read_api_port(c.path()), None);
}

#[test]
fn read_api_port_rejects_out_of_range_and_zero() {
    let big = tempfile::TempDir::new().unwrap();
    std::fs::write(big.path().join("api.port"), "127.0.0.1:70000").unwrap();
    assert_eq!(read_api_port(big.path()), None);
    let zero = tempfile::TempDir::new().unwrap();
    std::fs::write(zero.path().join("api.port"), "127.0.0.1:0").unwrap();
    assert_eq!(read_api_port(zero.path()), None);
}

#[test]
fn read_api_port_rejects_non_loopback_host() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("api.port"), "10.0.0.5:8080").unwrap();
    assert_eq!(
        read_api_port(dir.path()),
        None,
        "non-loopback bind must never attach",
    );
}

#[test]
fn read_api_token_parses_valid_token() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("api-token"), "abc-123\n").unwrap();
    assert_eq!(read_api_token(dir.path()), Some("abc-123".to_string()));
}

#[test]
fn read_api_token_rejects_empty_and_missing() {
    let a = tempfile::TempDir::new().unwrap();
    std::fs::write(a.path().join("api-token"), "\n").unwrap();
    assert_eq!(read_api_token(a.path()), None);
    let b = tempfile::TempDir::new().unwrap();
    assert_eq!(read_api_token(b.path()), None);
}

#[test]
fn daemon_endpoint_is_loopback_only() {
    assert_eq!(loopback_api_base(12_345), "http://127.0.0.1:12345");
    assert!(is_loopback_host("127.0.0.1"));
    assert!(is_loopback_host("::1"));
    assert!(!is_loopback_host("example.com"));
}

// ── Attach lifecycle (spawn-or-attach state transition) ─────────────────────

#[test]
fn attach_when_healthy_avoids_spawn_and_keeps_daemon_unowned() {
    let f = harness(true);
    write_artifacts(&f.data_dir, 4847, SENTINEL_TOKEN);

    let sup = supervisor(&f);
    let mut handle = sup.bring_up().expect("attach must succeed");

    assert_eq!(f.spawner.spawn_calls.load(Ordering::SeqCst), 0);
    assert!(
        handle.daemon.is_none(),
        "attached daemon must not be owned (not spawned)",
    );
    assert_eq!(handle.api_base, loopback_api_base(4847));

    handle.shutdown();
    assert!(
        snapshot(&f.kill_log).is_empty(),
        "attached daemon must never be reaped",
    );
}

#[test]
fn stale_port_present_but_daemon_unhealthy_triggers_spawn() {
    let f = harness(false);
    write_artifacts(&f.data_dir, 4847, SENTINEL_TOKEN);
    let sup = supervisor(&f);
    let handle = sup.bring_up().expect("stale daemon ⇒ fresh spawn succeeds");
    assert!(
        f.spawner.spawn_calls.load(Ordering::SeqCst) >= 1,
        "stale daemon must spawn",
    );
    assert!(handle.daemon.is_some());
}

#[test]
fn missing_artifacts_triggers_spawn() {
    let f = harness(false);
    let sup = supervisor(&f);
    let handle = sup.bring_up().expect("missing artifacts ⇒ spawn succeeds");
    assert!(f.spawner.spawn_calls.load(Ordering::SeqCst) >= 1);
    assert!(handle.daemon.is_some());
    let args = lock(&f.spawner.spawned_args)
        .iter()
        .flatten()
        .cloned()
        .collect::<Vec<_>>();
    assert!(
        args.iter().any(|a| a == INSTANCE_NAME),
        "spawned with --name {INSTANCE_NAME}: {args:?}",
    );
}

// ── Readiness timeout kills/reaps the owned child (error mapping) ───────────

#[test]
fn daemon_readiness_timeout_kills_owned_child() {
    let f = harness(false);
    lock(&f.spawner.plan).write_artifacts = false;
    lock(&f.spawner.plan).boot_daemon = false;
    let sup = supervisor(&f);
    let err = err_of(sup.bring_up(), "daemon never ready must time out");
    match err {
        LocalDaemonError::Timeout => {}
        other => panic!("expected LocalDaemonError::Timeout, got {other:?}"),
    }
    assert!(f.spawner.spawn_calls.load(Ordering::SeqCst) >= 1);
    // The spawned child is dropped on the timeout path and reaped exactly once.
    assert_eq!(snapshot(&f.kill_log), vec![DAEMON]);
}

// ── Shutdown idempotency & ownership ────────────────────────────────────────

#[test]
fn shutdown_is_idempotent() {
    let f = harness(false);
    let mut handle = owned_handle(&f);
    handle.shutdown();
    handle.shutdown();
    handle.shutdown();
    assert_eq!(snapshot(&f.kill_log), vec![DAEMON]);
}

#[test]
fn shutdown_does_not_reap_an_unowned_handle() {
    let f = harness(false);
    let mut handle = LocalStackHandle {
        daemon: None,
        api_base: loopback_api_base(4847),
        data_dir: f.data_dir.clone(),
    };
    handle.shutdown();
    assert!(
        snapshot(&f.kill_log).is_empty(),
        "a handle owning no child must reap nothing",
    );
}

// ── Token safety: the bearer never leaks into error/handle surfaces ─────────

#[test]
fn error_strings_never_contain_token() {
    let f = harness(false);
    lock(&f.spawner.plan).fail = true;
    let sup = supervisor(&f);
    let err = err_of(sup.bring_up(), "daemon spawn failure");
    assert!(
        !format!("{err:?}").contains(SENTINEL_TOKEN),
        "token leaked into spawn error: {err:?}",
    );

    let f2 = harness(false);
    lock(&f2.spawner.plan).write_artifacts = false;
    lock(&f2.spawner.plan).boot_daemon = false;
    let sup2 = supervisor(&f2);
    let err2 = err_of(sup2.bring_up(), "timeout");
    assert!(
        !format!("{err2:?}").contains(SENTINEL_TOKEN),
        "token leaked into timeout error: {err2:?}",
    );
}

#[test]
fn handle_exposes_only_loopback_api_base_never_token() {
    let f = harness(false);
    let sup = supervisor(&f);
    let handle = sup.bring_up().expect("spawn succeeds");
    assert_eq!(handle.api_base, loopback_api_base(4847));
    assert!(
        !handle.api_base.contains(SENTINEL_TOKEN),
        "token leaked into api_base: {}",
        handle.api_base,
    );
    let dbg = format!("{handle:?}");
    assert!(
        !dbg.contains(SENTINEL_TOKEN),
        "token leaked into handle Debug: {dbg}",
    );
}

// ── Drop reaps without double-kill ──────────────────────────────────────────

#[test]
fn owned_child_drop_reaps_without_explicit_shutdown() {
    let kill_log: Arc<Mutex<Vec<&'static str>>> = Arc::new(Mutex::new(Vec::new()));
    {
        let _child = OwnedChild::from_killable(Box::new(FakeKillable {
            label: DAEMON,
            log: kill_log.clone(),
        }));
        // dropped here without shutdown() — Drop must reap exactly once.
    }
    assert_eq!(
        snapshot(&kill_log),
        vec![DAEMON],
        "Drop must reap an unshutdown child",
    );
}

#[test]
fn owned_child_shutdown_then_drop_does_not_double_kill() {
    let kill_log: Arc<Mutex<Vec<&'static str>>> = Arc::new(Mutex::new(Vec::new()));
    {
        let mut child = OwnedChild::from_killable(Box::new(FakeKillable {
            label: DAEMON,
            log: kill_log.clone(),
        }));
        child.shutdown(); // reaps once, sets the reaped flag.
                          // child dropped here — Drop must NOT reap again.
    }
    assert_eq!(
        snapshot(&kill_log),
        vec![DAEMON],
        "explicit shutdown then drop must reap exactly once (no double-kill)",
    );
}

#[test]
fn handle_shutdown_then_drop_does_not_double_kill() {
    let f = harness(false);
    let kill_log = f.kill_log.clone();
    {
        let mut handle = owned_handle(&f);
        handle.shutdown();
        // handle + its taken child drop here — no additional reap.
    }
    assert_eq!(snapshot(&kill_log), vec![DAEMON]);
}

#[path = "local_stack_sidecar_tests.rs"]
mod sidecar;

#[path = "local_stack_native_fixture.rs"]
mod native_fixture;

#[path = "local_stack_smoke_tests.rs"]
mod smoke;
