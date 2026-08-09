//! Native real-process fixture for the local-stack acceptance tests.
//!
//! Spawns the REAL `x0xd` daemon from `TTT_X0XD_BINARY` (falling back to the
//! sibling debug build at `<repo>/../x0x/target/debug/x0xd`) in a fully
//! isolated configuration: a private temp data dir + HOME, loopback-only
//! binds, no bootstrap peers, and a disabled peer cache. No relay, Nostr,
//! x0x-nostr-bridge, mock IPC, or secure-group cryptography is involved — this
//! talks to x0xd's own authenticated loopback REST API directly.
//!
//! The fixture reuses PRODUCTION components instead of duplicating protocol
//! logic: it spawns through the real [`StdSidecarSpawner`] (so the production
//! process-group + SIGTERM/SIGKILL reap path is what kills the daemon), reads
//! the daemon's `api.port`/`api-token` artifacts through the production
//! readers, and gates readiness on the production [`LoopbackHttpDaemonProbe`]
//! `/health` check.
//!
//! Lifecycle: [`NativeDaemonFixture::start`] blocks until `/health` is healthy
//! (deadline-polled — never an arbitrary sleep). [`Drop`] reaps the owned child
//! if the test forgot to, so a forgotten shutdown can never leak a daemon.
//!
//! Extension point: Main can build a second fixture (a second isolated daemon)
//! and wire a two-daemon public/DM history restart+search acceptance on top of
//! this harness — no bridge fields exist here to work around.

#![allow(dead_code)]
use super::*;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tempfile::TempDir;

/// How long [`NativeDaemonFixture::start`] waits for x0xd to write its
/// artifacts and answer `/health` healthy.
const READINESS_TIMEOUT: Duration = Duration::from_secs(30);
/// Poll cadence for the readiness/death loops (mirrors the supervisor's).
const FIXTURE_POLL: Duration = Duration::from_millis(200);

/// Monotonic instance counter so two fixtures never collide on identity dir.
static INSTANCE_SEQ: AtomicU64 = AtomicU64::new(0);

/// Resolve the x0xd binary: `TTT_X0XD_BINARY` wins, else the sibling debug build.
pub(super) fn resolve_x0xd_binary() -> PathBuf {
    if let Some(raw) = std::env::var(super::X0XD_BINARY_ENV)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
    {
        return PathBuf::from(raw);
    }
    // <manifest> = …/tic-tac-toe/desktop/src-tauri  ⇒  …/tic-tac-toe/../x0x
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let repo = manifest
        .parent()
        .and_then(Path::parent)
        .expect("manifest has project-root parents");
    repo.join("..")
        .join("x0x")
        .join("target")
        .join("debug")
        .join("x0xd")
}

/// Options for [`NativeDaemonFixture::start_with`].
///
/// `bind_port = 0` asks the OS for an ephemeral loopback QUIC port (the
/// single-daemon case). A fixed non-zero loopback port lets a SECOND daemon
/// explicitly bootstrap to this one over loopback — the supported pairing
/// primitive (mirrors the x0xd test harness). `bootstrap_peers` are loopback
/// `host:port` dial targets carried verbatim into the config.
pub(super) struct FixtureOptions {
    pub(super) bind_port: u16,
    pub(super) bootstrap_peers: Vec<String>,
}

impl FixtureOptions {
    /// Single isolated daemon: ephemeral loopback bind, no bootstrap peers.
    pub(super) fn isolated() -> Self {
        Self {
            bind_port: 0,
            bootstrap_peers: Vec::new(),
        }
    }
}

/// Allocate an ephemeral loopback UDP port (bind-and-drop; mirrors the x0xd
/// test harness). Used to hand a fixed loopback QUIC port to a second daemon's
/// `bootstrap_peers` so two fixtures can pair without joining the public net.
#[cfg(unix)]
pub(super) fn allocate_unused_udp_port() -> u16 {
    std::net::UdpSocket::bind("127.0.0.1:0")
        .expect("bind ephemeral UDP port")
        .local_addr()
        .expect("udp local addr")
        .port()
}

/// A real, isolated `x0xd` process owned by the test.
pub(super) struct NativeDaemonFixture {
    child: Option<OwnedChild>,
    data_dir: PathBuf,
    // Kept alive for the daemon's lifetime; dropped (reaped) with the child.
    _home: TempDir,
    api_port: u16,
    api_token: String,
    /// Loopback QUIC bind port (0 = OS-assigned ephemeral).
    bind_port: u16,
    /// Retained spawn recipe so [`restart`] respawns the SAME daemon — same
    /// data dir (durable history + identity survive) on the same loopback bind.
    command: SidecarCommand,
}

impl NativeDaemonFixture {
    /// Spawn an isolated single daemon (ephemeral loopback bind, no peers) and
    /// block until `/health` is healthy.
    ///
    /// Panics with an actionable message if the binary is missing — these are
    /// opt-in `#[ignore]`d tests, so a missing binary is a setup failure, not a
    /// silent pass.
    #[cfg(unix)]
    pub(super) fn start() -> Self {
        Self::start_with(FixtureOptions::isolated())
    }

    /// Spawn an isolated daemon with the given loopback bind/bootstrap options
    /// and block until `/health` is healthy.
    ///
    /// `bind_port = 0` ⇒ ephemeral loopback QUIC port (single-daemon tests). A
    /// fixed non-zero port plus a `bootstrap_peers` entry pointing at another
    /// fixture's bind port is the supported loopback pairing primitive — the
    /// second daemon dials the first over loopback only and never touches the
    /// public network (`--no-hard-coded-bootstrap` + `bootstrap_peers = [...]`).
    #[cfg(unix)]
    pub(super) fn start_with(opts: FixtureOptions) -> Self {
        let binary = resolve_x0xd_binary();
        assert!(
            binary.is_file(),
            "x0xd binary not found at {} (set {})",
            binary.display(),
            super::X0XD_BINARY_ENV,
        );

        let home = TempDir::new().expect("temp home");
        let data_dir = home.path().join("data");
        std::fs::create_dir_all(&data_dir).expect("create data dir");

        // Isolated TOML config: loopback-only binds, explicit (possibly empty)
        // bootstrap peers, temp data dir, unique instance name.
        // `--no-hard-coded-bootstrap` skips the embedded public seed peers so
        // the daemon never dials the public network; when `bootstrap_peers` is
        // set it is honoured verbatim (loopback dial targets only).
        let seq = INSTANCE_SEQ.fetch_add(1, Ordering::SeqCst);
        let instance_name = format!("ttt-native-{seq}");
        let data_dir_str = data_dir.display().to_string();
        let peers = opts
            .bootstrap_peers
            .iter()
            .map(|p| format!("\"{p}\""))
            .collect::<Vec<_>>()
            .join(", ");
        let config = format!(
            "bind_address = \"127.0.0.1:{bind}\"\n\
             api_address = \"127.0.0.1:0\"\n\
             data_dir = \"{data_dir_str}\"\n\
             log_level = \"warn\"\n\
             bootstrap_peers = [{peers}]\n\
             instance_name = \"{instance_name}\"\n",
            bind = opts.bind_port,
        );
        let config_path = home.path().join("config.toml");
        std::fs::write(&config_path, config).expect("write config");

        // Spawn through the PRODUCTION spawner so the real process-group +
        // reap path is exercised end-to-end. HOME is scoped to the child only
        // (never the test process) so x0xd's identity dir lands in the temp dir.
        let command = SidecarCommand {
            label: "x0xd",
            binary: binary.clone(),
            args: vec![
                "--config".to_string(),
                config_path.to_string_lossy().into_owned(),
                "--skip-update-check".to_string(),
                "--no-hard-coded-bootstrap".to_string(),
                "--disable-peer-cache".to_string(),
            ],
            env: vec![(
                "HOME".to_string(),
                home.path().to_string_lossy().into_owned(),
            )],
            log_path: Some(data_dir.join("x0xd.log")),
        };
        let child = StdSidecarSpawner
            .spawn(&command)
            .unwrap_or_else(|e| panic!("spawn x0xd at {}: {e:?}", binary.display()));

        let mut fixture = Self {
            child: Some(child),
            data_dir: data_dir.clone(),
            _home: home,
            api_port: 0,
            api_token: String::new(),
            bind_port: opts.bind_port,
            command,
        };
        fixture.wait_ready();
        fixture
    }

    /// Deadline-polled readiness against the production probe — no fixed sleep.
    #[cfg(unix)]
    fn wait_ready(&mut self) {
        let deadline = Instant::now() + READINESS_TIMEOUT;
        loop {
            if let (Some(port), Some(token)) = (
                read_api_port(&self.data_dir),
                read_api_token(&self.data_dir),
            ) {
                let api_base = loopback_api_base(port);
                if LoopbackHttpDaemonProbe.health(&api_base, &token).is_ok() {
                    self.api_port = port;
                    self.api_token = token;
                    return;
                }
            }
            assert!(
                Instant::now() < deadline,
                "x0xd did not become healthy within {READINESS_TIMEOUT:?}; see {}",
                self.data_dir.join("x0xd.log").display(),
            );
            std::thread::sleep(FIXTURE_POLL);
        }
    }

    /// Loopback API base (`http://127.0.0.1:<port>`).
    pub(super) fn api_base(&self) -> String {
        loopback_api_base(self.api_port)
    }

    pub(super) fn api_port(&self) -> u16 {
        self.api_port
    }

    pub(super) fn api_token(&self) -> &str {
        &self.api_token
    }

    pub(super) fn bind_port(&self) -> u16 {
        self.bind_port
    }

    /// Restart the daemon on the SAME data dir using the retained spawn recipe.
    ///
    /// Defends the v1 zero-server proof point: durable history (SQLite in the
    /// data dir) and identity (agent.key in the data dir) survive a full
    /// process terminate + respawn. The old child is reaped first via the
    /// production path; readiness is deadline-polled, never a fixed sleep.
    #[cfg(unix)]
    pub(super) fn restart(&mut self) {
        if let Some(mut child) = self.child.take() {
            child.shutdown();
        }
        // Wait for the old daemon to release its API listener so the respawn
        // never races the dying process for the data dir / loopback bind.
        let old_port = self.api_port;
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline && !listen_pids_on(old_port).is_empty() {
            std::thread::sleep(FIXTURE_POLL);
        }
        let child = StdSidecarSpawner
            .spawn(&self.command)
            .expect("respawn x0xd for restart");
        self.child = Some(child);
        self.wait_ready();
    }

    pub(super) fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    /// Take ownership of the daemon child for an explicit reap test. After this
    /// [`Drop`] is a no-op — the caller owns the lifecycle.
    pub(super) fn take_child(&mut self) -> Option<OwnedChild> {
        self.child.take()
    }

    /// Shut the daemon down via the production reap path. Idempotent.
    pub(super) fn shutdown(&mut self) {
        if let Some(mut child) = self.child.take() {
            child.shutdown();
        }
    }
}

impl Drop for NativeDaemonFixture {
    fn drop(&mut self) {
        // Fail-closed: reap an unclaimed child so a forgotten shutdown can
        // never leak a daemon past the test.
        if let Some(mut child) = self.child.take() {
            child.shutdown();
        }
    }
}

// ─── Listener/process inspection helpers (lsof-backed; unix only) ───────────
//
// Used by the real-process listener-isolation test. A missing `lsof` makes the
// caller skip (never a false pass).

/// Explicit capability probe for `lsof` (macOS/Linux).
#[cfg(unix)]
pub(super) fn lsof_available() -> bool {
    std::process::Command::new("lsof")
        .arg("-v")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// PIDs owning a TCP LISTEN socket on `port` (scoped port lookup; empty on miss).
#[cfg(unix)]
pub(super) fn listen_pids_on(port: u16) -> Vec<u32> {
    // `-iTCP:{port}` must be a SINGLE argv token; splitting it into `-iTCP` +
    // `:port` makes lsof treat the latter as a filespec and the lookup misses.
    let sel = format!("-iTCP:{port}");
    let out = std::process::Command::new("lsof")
        .args(["-nP", "-t", "-a", "-sTCP:LISTEN"])
        .arg(&sel)
        .output();
    match out {
        Ok(o) => String::from_utf8_lossy(&o.stdout)
            .lines()
            .filter_map(|l| l.trim().parse::<u32>().ok())
            .collect(),
        Err(_) => Vec::new(),
    }
}

/// Host component of an lsof NAME field: `[::1]:3300` → `::1`, `127.0.0.1:9` → `127.0.0.1`.
#[cfg(unix)]
fn host_of(addr: &str) -> String {
    if let Some(rest) = addr.strip_prefix('[') {
        if let Some(end) = rest.find(']') {
            return rest[..end].to_string();
        }
    }
    match addr.rfind(':') {
        Some(i) => addr[..i].to_string(),
        None => addr.to_string(),
    }
}

/// Port component of an lsof NAME field (0 if unparseable).
#[cfg(unix)]
fn port_of(addr: &str) -> u16 {
    let after_brackets = if let Some(rest) = addr.strip_prefix('[') {
        rest.find(']').map(|e| &rest[e + 1..]).unwrap_or(rest)
    } else {
        addr
    };
    after_brackets
        .rsplit(':')
        .next()
        .and_then(|p| p.parse::<u16>().ok())
        .unwrap_or(0)
}

/// Every TCP LISTEN socket owned by `pid`, parsed from `lsof`'s NAME column.
/// Returns `(host, port)` pairs; `*` / `0.0.0.0` survive untouched so the caller
/// can reject them.
#[cfg(unix)]
pub(super) fn pid_listen_sockets(pid: u32) -> Vec<(String, u16)> {
    let out = std::process::Command::new("lsof")
        .args(["-nP", "-a", "-p", &pid.to_string(), "-iTCP", "-sTCP:LISTEN"])
        .output();
    let txt = match out {
        Ok(o) => String::from_utf8_lossy(&o.stdout).into_owned(),
        Err(_) => return Vec::new(),
    };
    let mut socks = Vec::new();
    for line in txt.lines() {
        // NAME column renders `127.0.0.1:3300 (LISTEN)`; the address token sits
        // immediately before the `(LISTEN)` marker.
        let f: Vec<&str> = line.split_whitespace().collect();
        if let Some(idx) = f.iter().position(|x| *x == "(LISTEN)") {
            if idx > 0 {
                let addr = f[idx - 1];
                socks.push((host_of(addr), port_of(addr)));
            }
        }
    }
    socks
}

/// `true` iff `pid` is still alive (signal-0 probe).
#[cfg(unix)]
pub(super) fn process_running(pid: u32) -> bool {
    // SAFETY: a zero signal checks liveness without delivering a signal.
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

#[cfg(unix)]
pub(super) fn unique_sorted_u32(v: Vec<u32>) -> Vec<u32> {
    let mut s = v;
    s.sort_unstable();
    s.dedup();
    s
}
