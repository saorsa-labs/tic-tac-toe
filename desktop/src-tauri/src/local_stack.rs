//! Spawn-or-attach lifecycle for the local `x0xd` daemon.
//!
//! The desktop talks directly to x0xd's authenticated loopback REST/WS API.
//! No compatibility transport or intermediary process is started here.

use std::fmt;
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;
use url::Url;

pub(crate) const INSTANCE_NAME: &str = "ttt";
const X0X_DIR_PREFIX: &str = "x0x-";
const POLL_INTERVAL: Duration = Duration::from_millis(200);
const DEFAULT_DAEMON_TIMEOUT: Duration = Duration::from_secs(12);
pub(crate) const X0XD_BINARY_ENV: &str = "TTT_X0XD_BINARY";

pub(crate) fn named_data_dir() -> Option<PathBuf> {
    dirs::data_dir().map(|dir| dir.join(format!("{X0X_DIR_PREFIX}{INSTANCE_NAME}")))
}

pub(crate) fn loopback_api_base(port: u16) -> String {
    format!("http://127.0.0.1:{port}")
}

pub(crate) fn read_api_port(data_dir: &Path) -> Option<u16> {
    let raw = std::fs::read_to_string(data_dir.join("api.port")).ok()?;
    let (host, port) = raw.trim().rsplit_once(':')?;
    if !is_loopback_host(host.trim()) {
        return None;
    }
    let port = port.trim().parse::<u16>().ok()?;
    (port != 0).then_some(port)
}

pub(crate) fn read_api_token(data_dir: &Path) -> Option<String> {
    let raw = std::fs::read_to_string(data_dir.join("api-token")).ok()?;
    let token = raw.trim();
    (!token.is_empty()).then(|| token.to_string())
}

pub(crate) fn is_loopback_host(host: &str) -> bool {
    host == "localhost" || host == "::1" || host.starts_with("127.")
}

#[derive(Debug)]
pub(crate) enum ProbeError {
    Unreachable,
    Unhealthy,
    Malformed,
}

#[derive(Debug)]
pub(crate) enum SpawnError {
    NotFound(&'static str),
    Invalid(&'static str, String),
    System(&'static str, String),
}

#[derive(Debug)]
pub(crate) enum LocalDaemonError {
    NoDataDir,
    Spawn(SpawnError),
    Exited { status: String, log_path: PathBuf },
    Monitor(String),
    Timeout,
}

impl fmt::Display for LocalDaemonError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoDataDir => write!(formatter, "could not resolve the x0xd data directory"),
            Self::Spawn(SpawnError::NotFound(name)) => write!(
                formatter,
                "{name} binary not found adjacent to the app; set {X0XD_BINARY_ENV}"
            ),
            Self::Spawn(SpawnError::Invalid(name, reason)) => {
                write!(formatter, "invalid {name} binary: {reason}")
            }
            Self::Spawn(SpawnError::System(name, reason)) => {
                write!(formatter, "{name} spawn failed: {reason}")
            }
            Self::Exited { status, log_path } => write!(
                formatter,
                "x0xd exited before becoming healthy ({status}); see {}",
                log_path.display()
            ),
            Self::Monitor(reason) => {
                write!(formatter, "could not observe x0xd startup status: {reason}")
            }
            Self::Timeout => write!(formatter, "timed out waiting for x0xd health"),
        }
    }
}

impl std::error::Error for LocalDaemonError {}

pub(crate) trait TimeSource: Send {
    fn now(&self) -> Instant;
    fn sleep(&self, duration: Duration);
}

pub(crate) trait DaemonProbe: Send {
    fn health(&self, api_base: &str, token: &str) -> Result<(), ProbeError>;
}

pub(crate) trait SidecarSpawner: Send {
    fn spawn(&self, command: &SidecarCommand) -> Result<OwnedChild, SpawnError>;
}

pub(crate) trait Killable: Send {
    fn label(&self) -> &'static str;
    fn try_wait(&mut self) -> Result<Option<String>, String> {
        Ok(None)
    }
    fn kill_and_reap(&mut self);
}

pub(crate) struct SidecarCommand {
    pub(crate) label: &'static str,
    pub(crate) binary: PathBuf,
    pub(crate) args: Vec<String>,
    pub(crate) env: Vec<(String, String)>,
    pub(crate) log_path: Option<PathBuf>,
}

pub(crate) struct OwnedChild {
    inner: Box<dyn Killable>,
    reaped: bool,
}

impl OwnedChild {
    pub(crate) fn from_killable(child: Box<dyn Killable>) -> Self {
        Self {
            inner: child,
            reaped: false,
        }
    }

    pub(crate) fn label(&self) -> &'static str {
        self.inner.label()
    }

    pub(crate) fn try_wait(&mut self) -> Result<Option<String>, String> {
        let status = self.inner.try_wait()?;
        if status.is_some() {
            self.reaped = true;
        }
        Ok(status)
    }

    pub(crate) fn shutdown(&mut self) {
        if !self.reaped {
            self.inner.kill_and_reap();
            self.reaped = true;
        }
    }
}

impl Drop for OwnedChild {
    fn drop(&mut self) {
        if !self.reaped {
            self.inner.kill_and_reap();
            self.reaped = true;
        }
    }
}

pub(crate) struct LocalStackHandle {
    pub(crate) daemon: Option<OwnedChild>,
    pub(crate) api_base: String,
    pub(crate) data_dir: PathBuf,
}

impl LocalStackHandle {
    pub(crate) fn shutdown(&mut self) {
        if let Some(mut daemon) = self.daemon.take() {
            daemon.shutdown();
        }
    }
}

impl fmt::Debug for LocalStackHandle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LocalStackHandle")
            .field("daemon", &self.daemon.as_ref().map(OwnedChild::label))
            .field("api_base", &self.api_base)
            .field("data_dir", &self.data_dir)
            .finish()
    }
}

pub(crate) struct StackConfig {
    pub(crate) data_dir: PathBuf,
    pub(crate) x0xd_binary: PathBuf,
    pub(crate) daemon_timeout: Duration,
}

impl StackConfig {
    pub(crate) fn resolve() -> Result<Self, LocalDaemonError> {
        let data_dir = named_data_dir().ok_or(LocalDaemonError::NoDataDir)?;
        // Ensure the data directory exists before the log-file open in
        // spawn_daemon tries to write x0xd.log inside it. x0xd also creates
        // this directory, but the app opens the log file first.
        let _ = std::fs::create_dir_all(&data_dir);
        let x0xd_binary =
            resolve_sidecar("x0xd", X0XD_BINARY_ENV).map_err(LocalDaemonError::Spawn)?;
        Ok(Self {
            data_dir,
            x0xd_binary,
            daemon_timeout: DEFAULT_DAEMON_TIMEOUT,
        })
    }
}

pub(crate) struct LocalStackSupervisor<P: DaemonProbe, S: SidecarSpawner, T: TimeSource> {
    cfg: StackConfig,
    probe: P,
    spawner: S,
    time: T,
}

impl<P: DaemonProbe, S: SidecarSpawner, T: TimeSource> LocalStackSupervisor<P, S, T> {
    pub(crate) fn new(cfg: StackConfig, probe: P, spawner: S, time: T) -> Self {
        Self {
            cfg,
            probe,
            spawner,
            time,
        }
    }

    pub(crate) fn bring_up(&self) -> Result<LocalStackHandle, LocalDaemonError> {
        let (daemon, api_base) = match self.try_attach() {
            Some(api_base) => (None, api_base),
            None => {
                let mut child = self.spawn_daemon()?;
                let api_base = self.wait_ready(&mut child)?;
                (Some(child), api_base)
            }
        };
        Ok(LocalStackHandle {
            daemon,
            api_base,
            data_dir: self.cfg.data_dir.clone(),
        })
    }

    fn try_attach(&self) -> Option<String> {
        let port = read_api_port(&self.cfg.data_dir)?;
        let token = read_api_token(&self.cfg.data_dir)?;
        let api_base = loopback_api_base(port);
        self.probe.health(&api_base, &token).ok()?;
        Some(api_base)
    }

    fn spawn_daemon(&self) -> Result<OwnedChild, LocalDaemonError> {
        let command = SidecarCommand {
            label: "x0xd",
            binary: self.cfg.x0xd_binary.clone(),
            args: vec![
                "--name".to_string(),
                INSTANCE_NAME.to_string(),
                "--skip-update-check".to_string(),
            ],
            env: Vec::new(),
            log_path: Some(self.cfg.data_dir.join("x0xd.log")),
        };
        self.spawner
            .spawn(&command)
            .map_err(LocalDaemonError::Spawn)
    }

    fn wait_ready(&self, child: &mut OwnedChild) -> Result<String, LocalDaemonError> {
        let deadline = self.time.now() + self.cfg.daemon_timeout;
        loop {
            if let Some(status) = child.try_wait().map_err(LocalDaemonError::Monitor)? {
                return Err(LocalDaemonError::Exited {
                    status,
                    log_path: self.cfg.data_dir.join("x0xd.log"),
                });
            }
            if let (Some(port), Some(token)) = (
                read_api_port(&self.cfg.data_dir),
                read_api_token(&self.cfg.data_dir),
            ) {
                let api_base = loopback_api_base(port);
                if self.probe.health(&api_base, &token).is_ok() {
                    return Ok(api_base);
                }
            }
            if self.time.now() >= deadline {
                return Err(LocalDaemonError::Timeout);
            }
            self.time.sleep(POLL_INTERVAL);
        }
    }
}

pub(crate) struct BlockingTimeSource;
impl TimeSource for BlockingTimeSource {
    fn now(&self) -> Instant {
        Instant::now()
    }

    fn sleep(&self, duration: Duration) {
        std::thread::sleep(duration);
    }
}

pub(crate) struct LoopbackHttpDaemonProbe;
impl DaemonProbe for LoopbackHttpDaemonProbe {
    fn health(&self, api_base: &str, token: &str) -> Result<(), ProbeError> {
        let url = format!("{}/health", api_base.trim_end_matches('/'));
        let response = http_get_json(&url, Some(token))?;
        match response.get("ok").and_then(Value::as_bool) {
            Some(true) => Ok(()),
            _ => Err(ProbeError::Unhealthy),
        }
    }
}

pub(crate) struct StdSidecarSpawner;
impl SidecarSpawner for StdSidecarSpawner {
    fn spawn(&self, spec: &SidecarCommand) -> Result<OwnedChild, SpawnError> {
        let (stdout, stderr) = open_log_pair(spec.log_path.as_deref());
        let mut command = Command::new(&spec.binary);
        crate::util::configure_no_window(&mut command);
        set_new_process_group(&mut command);
        command.stdin(Stdio::null()).stdout(stdout).stderr(stderr);
        command.args(&spec.args);
        command.envs(spec.env.iter().map(|(key, value)| (key, value)));
        let child = command
            .spawn()
            .map_err(|error| SpawnError::System(spec.label, error.to_string()))?;
        Ok(OwnedChild::from_killable(Box::new(StdChild::new(
            child, spec.label,
        ))))
    }
}

struct StdChild {
    child: std::process::Child,
    label: &'static str,
    #[cfg(windows)]
    job: Option<crate::managed_agents::JobHandle>,
}

impl StdChild {
    fn new(child: std::process::Child, label: &'static str) -> Self {
        #[cfg(windows)]
        let job = crate::managed_agents::create_job_for_child(child.id());
        Self {
            child,
            label,
            #[cfg(windows)]
            job,
        }
    }
}

impl Killable for StdChild {
    fn label(&self) -> &'static str {
        self.label
    }

    fn try_wait(&mut self) -> Result<Option<String>, String> {
        self.child
            .try_wait()
            .map(|status| status.map(|status| status.to_string()))
            .map_err(|error| error.to_string())
    }

    fn kill_and_reap(&mut self) {
        #[cfg(unix)]
        {
            let process_group = -(self.child.id() as i32);
            // SAFETY: the child was placed in its own process group at spawn.
            unsafe {
                libc::kill(process_group, libc::SIGTERM);
            }
            let deadline = Instant::now() + Duration::from_secs(2);
            loop {
                match self.child.try_wait() {
                    Ok(Some(_)) => break,
                    Ok(None) if Instant::now() < deadline => {
                        std::thread::sleep(Duration::from_millis(50));
                    }
                    Ok(None) | Err(_) => break,
                }
            }
            // SAFETY: a zero signal checks only whether the owned child exists.
            let still_running = unsafe { libc::kill(self.child.id() as i32, 0) == 0 };
            if still_running {
                // SAFETY: the negative id targets only the child's process group.
                unsafe {
                    libc::kill(process_group, libc::SIGKILL);
                }
            }
        }

        #[cfg(windows)]
        {
            self.job.take();
            let _ = self.child.kill();
        }

        let _ = self.child.wait();
    }
}

#[cfg(unix)]
fn set_new_process_group(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    command.process_group(0);
}

#[cfg(not(unix))]
fn set_new_process_group(_command: &mut Command) {}

pub(crate) fn resolve_sidecar(name: &'static str, env_key: &str) -> Result<PathBuf, SpawnError> {
    if let Some(raw) = std::env::var(env_key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
    {
        let path = PathBuf::from(raw);
        validate_executable(&path, name)?;
        return Ok(path);
    }

    let executable =
        std::env::current_exe().map_err(|error| SpawnError::System(name, error.to_string()))?;
    let parent = executable.parent().ok_or(SpawnError::NotFound(name))?;
    let candidates = [
        parent.join("binaries"),
        parent.to_path_buf(),
        parent.join("..").join("Resources").join("binaries"),
        parent.join("..").join("Resources"),
    ];
    candidates
        .iter()
        .find_map(|directory| find_sidecar_in(directory, name))
        .ok_or(SpawnError::NotFound(name))
}

pub(crate) fn find_sidecar_in(directory: &Path, name: &str) -> Option<PathBuf> {
    let entries = std::fs::read_dir(directory).ok()?;
    let executable_suffix = if cfg!(windows) { ".exe" } else { "" };
    let exact = format!("{name}{executable_suffix}");
    let prefix = format!("{name}-");
    let mut suffixed = None;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let file_name = entry.file_name();
        let Some(file_name) = file_name.to_str() else {
            continue;
        };
        if file_name == exact {
            return Some(path);
        }
        let stem = file_name
            .strip_suffix(executable_suffix)
            .unwrap_or(file_name);
        if stem.starts_with(&prefix) && suffixed.is_none() {
            suffixed = Some(path);
        }
    }
    suffixed
}

pub(crate) fn validate_executable(path: &Path, name: &'static str) -> Result<(), SpawnError> {
    let metadata =
        std::fs::metadata(path).map_err(|error| SpawnError::Invalid(name, error.to_string()))?;
    if !metadata.is_file() {
        return Err(SpawnError::Invalid(
            name,
            format!("{} is not a regular file", path.display()),
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            return Err(SpawnError::Invalid(
                name,
                format!("{} is not executable", path.display()),
            ));
        }
    }
    Ok(())
}

fn open_log_pair(path: Option<&Path>) -> (Stdio, Stdio) {
    fn open(path: &Path) -> Option<std::fs::File> {
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .ok()
    }
    match path {
        Some(path) => match (open(path), open(path)) {
            (Some(stdout), Some(stderr)) => (Stdio::from(stdout), Stdio::from(stderr)),
            _ => (Stdio::null(), Stdio::null()),
        },
        None => (Stdio::null(), Stdio::null()),
    }
}

pub(crate) fn http_get_json(url: &str, bearer: Option<&str>) -> Result<Value, ProbeError> {
    let parsed = Url::parse(url).map_err(|_| ProbeError::Unreachable)?;
    if parsed.scheme() != "http" {
        return Err(ProbeError::Unreachable);
    }
    let host = parsed.host_str().ok_or(ProbeError::Unreachable)?;
    if !is_loopback_host(host) {
        return Err(ProbeError::Unreachable);
    }
    let port = parsed.port().unwrap_or(80);
    let path = if parsed.path().is_empty() {
        "/"
    } else {
        parsed.path()
    };
    let address = (host, port)
        .to_socket_addrs()
        .map_err(|_| ProbeError::Unreachable)?
        .find(|address| address.ip().is_loopback())
        .ok_or(ProbeError::Unreachable)?;
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_millis(500))
        .map_err(|_| ProbeError::Unreachable)?;
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));

    let mut request = format!("GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n");
    if let Some(token) = bearer {
        request.push_str("Authorization: Bearer ");
        request.push_str(token);
        request.push_str("\r\n");
    }
    request.push_str("\r\n");
    stream
        .write_all(request.as_bytes())
        .map_err(|_| ProbeError::Unreachable)?;

    let mut bytes = Vec::with_capacity(4096);
    stream
        .read_to_end(&mut bytes)
        .map_err(|_| ProbeError::Unreachable)?;
    let response = String::from_utf8_lossy(&bytes);
    let (status, body) = split_http(&response)?;
    if !(200..300).contains(&status) {
        return Err(ProbeError::Unhealthy);
    }
    serde_json::from_str(body).map_err(|_| ProbeError::Malformed)
}

fn split_http(response: &str) -> Result<(u16, &str), ProbeError> {
    let header_end = response.find("\r\n\r\n").ok_or(ProbeError::Malformed)?;
    let headers = &response[..header_end];
    let body = &response[header_end + 4..];
    let status = headers
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|value| value.parse::<u16>().ok())
        .ok_or(ProbeError::Malformed)?;
    Ok((status, body))
}

pub(crate) fn fetch_agent() -> Result<Value, String> {
    let data_dir = named_data_dir().ok_or_else(|| "daemon data dir unavailable".to_string())?;
    let port = read_api_port(&data_dir)
        .ok_or_else(|| "daemon api.port missing or non-loopback".to_string())?;
    let token = read_api_token(&data_dir).ok_or_else(|| "daemon api-token missing".to_string())?;
    http_get_json(&format!("{}/agent", loopback_api_base(port)), Some(&token))
        .map_err(|_| "daemon /agent unreachable".to_string())
}

pub(crate) fn bring_up_local_stack(app: &tauri::AppHandle) {
    use tauri::Manager;

    let state = app.state::<crate::app_state::AppState>();
    let config = match StackConfig::resolve() {
        Ok(config) => config,
        Err(error) => {
            record_error(&state, error.to_string());
            return;
        }
    };
    let supervisor = LocalStackSupervisor::new(
        config,
        LoopbackHttpDaemonProbe,
        StdSidecarSpawner,
        BlockingTimeSource,
    );
    match supervisor.bring_up() {
        Ok(handle) => {
            eprintln!("local-x0xd: ready at {}", handle.api_base);
            match state.local_stack.lock() {
                Ok(mut guard) => *guard = Some(handle),
                Err(poisoned) => *poisoned.into_inner() = Some(handle),
            }
        }
        Err(error) => record_error(&state, error.to_string()),
    }
}

pub(crate) fn shutdown_owned(app: &tauri::AppHandle) {
    use tauri::Manager;

    let state = app.state::<crate::app_state::AppState>();
    let mut guard = state
        .local_stack
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(mut handle) = guard.take() {
        handle.shutdown();
    }
}

fn record_error(state: &crate::app_state::AppState, message: String) {
    eprintln!("local-x0xd: {message}");
    match state.local_stack_error.lock() {
        Ok(mut guard) => *guard = Some(message),
        Err(poisoned) => *poisoned.into_inner() = Some(message),
    }
}

#[cfg(test)]
#[path = "local_stack_tests.rs"]
mod tests;
