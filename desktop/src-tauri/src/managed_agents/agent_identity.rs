//! Per-managed-agent dedicated x0xd child identity.
//!
//! Each interactive managed agent owns a **dedicated persistent x0xd child** —
//! a named x0xd instance with an isolated data dir, transient loopback API
//! token, and its OWN signed AgentCard. The child's `AgentCard.agent_id` is the
//! agent's native x0x identity (the runtime-key `agent_id`). This NEVER reuses
//! the owner daemon identity or the legacy Nostr pubkey, and the relay
//! transport plays no role here.
//!
//! ## Lifecycle (mirrors `local_stack` / `symphony`)
//! On [`AgentChildSupervisor::bring_up`]:
//! 1. **Attach** to an already-running named instance when its artifacts
//!    (`api.port` loopback-valid + nonempty `api-token`) and a bearer `/health`
//!    say OK — without taking ownership (a warm child from a prior session).
//! 2. Else **spawn** `x0xd --name <instance> --skip-update-check`, own the
//!    child, and bounded-poll the named data dir for artifacts + health.
//! 3. **Fetch** the child's signed `AgentCard.agent_id` via an authenticated
//!    `GET /agent` (token read transiently, then dropped).
//!
//! The token is **transient**: read → used for `/health` and `/agent` → dropped.
//! It is never stored on [`AgentChildHandle`] or in `AppState`, never logged,
//! and never appears in any `Debug`/error/serialized output. Callers that need
//! to reach the child re-read `api-token` per request.
//!
//! Shutdown reaps **only app-owned** children (an attached child is `None`),
//! and is idempotent via `take()`.

use std::collections::HashMap;
use std::fmt;
use std::fs::File;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use serde_json::Value;
use sha2::{Digest as _, Sha256};

use crate::local_stack::{
    http_get_json, loopback_api_base, read_api_port, read_api_token, resolve_sidecar, DaemonProbe,
    OwnedChild, ProbeError, SidecarCommand, SidecarSpawner, SpawnError, StdSidecarSpawner,
    TimeSource,
};

// ── Constants ───────────────────────────────────────────────────────────────

/// Prefix namespacing managed-agent x0xd children under the platform data dir,
/// keeping them disjoint from the owner daemon (`x0x`) and any operator-named
/// instance (`x0x-<name>`). Resolves to `<data_dir>/x0x-managed-<instance>/`.
const MANAGED_INSTANCE_PREFIX: &str = "x0x-managed-";

/// `x0xd --name` accepts at most 64 characters. Managed children prepend
/// `managed-` to this stable suffix, leaving 56 characters for the per-agent
/// key. Long keys use the first 224 bits of a SHA-256 digest so Company keys
/// with long common prefixes remain distinct as well as deterministic.
const MANAGED_INSTANCE_KEY_LEN: usize = 64 - "managed-".len();

/// x0x's persisted `agent.key` envelope. The desktop crate intentionally does
/// not depend on the x0x library (it talks to the bundled daemon over HTTP), so
/// durable stopped-child recovery mirrors x0x's small, documented v1/v2
/// envelope here. These sizes are the ML-DSA-65 constants enforced by x0x's
/// `AgentKeypair::from_bytes` parser.
const X0X_KEYFILE_V2_MAGIC: [u8; 4] = *b"X0K2";
const X0X_KEYFILE_LEN_PREFIX_BYTES: usize = 8;
const X0X_AGENT_PUBLIC_KEY_BYTES: usize = 1_952;
const X0X_AGENT_SECRET_KEY_BYTES: u64 = 4_032;
const X0X_AGENT_KEYFILE_V1_BYTES: u64 = 6_000;
const X0X_AGENT_KEYFILE_V2_BYTES: u64 = 6_012;
const X0X_AGENT_ID_DOMAIN: &[u8] = b"AUTONOMI_PEER_ID_V2:";

const POLL_INTERVAL: Duration = Duration::from_millis(200);
const DEFAULT_AGENT_CHILD_TIMEOUT: Duration = Duration::from_secs(15);

/// `x0xd` is reused as the per-agent child binary (a named instance is a full
/// daemon with its own identity). Same env override + sidecar resolution as the
/// owner daemon.
pub(super) const X0XD_BINARY_ENV: &str = "TTT_X0XD_BINARY";
const X0XD_BINARY_NAME: &str = "x0xd";

// ── Named-instance data dir + artifact reads (pure, panic-free) ─────────────

/// `<data_dir>/x0x-managed-<instance>`, the per-agent child's artifact dir.
/// Matches x0xd's own `--name` data-dir derivation
/// (`<data_dir>/x0x-<name>` → here `<name>` = `managed-<instance>`).
pub(super) fn agent_child_data_dir(instance: &str) -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join(format!("{MANAGED_INSTANCE_PREFIX}{instance}")))
}

/// Reuse the platform artifact readers from `local_stack`: the named x0xd
/// instance writes the same `api.port` (loopback `host:port`) and `api-token`
/// files the owner daemon does.
pub(super) fn read_child_port(data_dir: &Path) -> Option<u16> {
    read_api_port(data_dir)
}

pub(super) fn read_child_token(data_dir: &Path) -> Option<String> {
    read_api_token(data_dir)
}

// ── Errors ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AgentChildStage {
    Health,
    AgentCard,
}

#[derive(Debug)]
pub(crate) enum AgentChildError {
    NoDataDir,
    SpawnFailed {
        sidecar: &'static str,
        reason: String,
    },
    Timeout {
        stage: AgentChildStage,
    },
    /// The child responded but its `AgentCard.agent_id` was absent/malformed.
    InvalidAgentCard {
        reason: String,
    },
}

impl AgentChildError {
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
            SpawnError::Invalid(s, r) => Self::SpawnFailed {
                sidecar: s,
                reason: r,
            },
        }
    }
}

impl fmt::Display for AgentChildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoDataDir => write!(f, "could not resolve the managed-agent data directory"),
            Self::SpawnFailed { sidecar, reason } => {
                write!(f, "{sidecar} spawn failed: {reason}")
            }
            Self::Timeout { stage } => match stage {
                AgentChildStage::Health => write!(f, "timed out waiting for agent-child health"),
                AgentChildStage::AgentCard => {
                    write!(f, "timed out waiting for the agent-child AgentCard")
                }
            },
            Self::InvalidAgentCard { reason } => {
                write!(f, "agent-child AgentCard was invalid: {reason}")
            }
        }
    }
}

impl std::error::Error for AgentChildError {}

// ── Owned handle ────────────────────────────────────────────────────────────

/// The supervisor's output: an owned child (`None` = attached warm instance,
/// never killed), the resolved loopback base URL, the data dir (so the
/// transient token can be re-read per request), and the fetched native
/// `agent_id`. Carries NO token.
pub(super) struct AgentChildHandle {
    pub(super) child: Option<OwnedChild>,
    pub(super) base_url: String,
    pub(super) data_dir: PathBuf,
    /// The child's signed x0x `AgentCard.agent_id` (64 lowercase hex). This is
    /// the managed agent's native identity — the runtime-key `agent_id`.
    pub(super) agent_id: String,
}

impl AgentChildHandle {
    /// Reap the owned child if any. Idempotent via `take()`.
    pub(super) fn shutdown(&mut self) {
        if let Some(mut child) = self.child.take() {
            child.shutdown();
        }
    }

    /// Whether this handle owns (and is responsible for killing) the child.
    pub(super) fn owns_child(&self) -> bool {
        self.child.is_some()
    }
}

impl Drop for AgentChildHandle {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl fmt::Debug for AgentChildHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // No token exists on the handle; identity + endpoints only.
        f.debug_struct("AgentChildHandle")
            .field("agent_id", &self.agent_id)
            .field("base_url", &self.base_url)
            .field("data_dir", &self.data_dir)
            .field("owned", &self.owns_child())
            .finish()
    }
}

// ── Config ──────────────────────────────────────────────────────────────────

pub(super) struct AgentChildConfig {
    pub(super) data_dir: PathBuf,
    pub(super) binary: PathBuf,
    pub(super) timeout: Duration,
}

impl AgentChildConfig {
    /// Resolve the per-agent child config for a stable instance handle. The
    /// `instance` is a filesystem anchor ONLY (e.g. derived from the agent
    /// record's stable storage key) — it is never the agent_id, which the
    /// child itself owns and which is fetched after bring-up.
    pub(super) fn resolve(instance: &str) -> Result<Self, AgentChildError> {
        let bounded_instance = bounded_agent_child_instance(instance);
        let data_dir = agent_child_data_dir(&bounded_instance).ok_or(AgentChildError::NoDataDir)?;
        let binary = resolve_sidecar(X0XD_BINARY_NAME, X0XD_BINARY_ENV)
            .map_err(AgentChildError::from_spawn)?;
        Ok(Self {
            data_dir,
            binary,
            timeout: DEFAULT_AGENT_CHILD_TIMEOUT,
        })
    }

    #[cfg(test)]
    pub(super) fn for_test(data_dir: PathBuf, binary: PathBuf) -> Self {
        Self {
            data_dir,
            binary,
            timeout: DEFAULT_AGENT_CHILD_TIMEOUT,
        }
    }
}

/// Bound the filesystem suffix and matching `x0xd --name` suffix together.
/// Both are derived from this value, so the daemon always writes readiness
/// artifacts into the directory polled by [`AgentChildSupervisor`].
fn bounded_agent_child_instance(instance: &str) -> String {
    let canonical = instance.trim().to_ascii_lowercase();
    if canonical.is_ascii() && canonical.len() <= MANAGED_INSTANCE_KEY_LEN {
        return canonical;
    }
    hex::encode(Sha256::digest(canonical.as_bytes()))
        .chars()
        .take(MANAGED_INSTANCE_KEY_LEN)
        .collect()
}

/// Resolve the identity directory x0xd itself chooses for a named instance.
/// Unlike readiness/history artifacts, named-instance identity material lives
/// under `$HOME/.x0x-<name>` rather than the platform application-data dir.
fn agent_child_identity_dir(home_dir: &Path, bounded_instance: &str) -> PathBuf {
    home_dir.join(format!(".x0x-managed-{bounded_instance}"))
}

/// Read only the public-key portion of an x0x `agent.key` and derive the same
/// native AgentId x0x exposes from `/agent`. The secret bytes are never read or
/// retained. Every structural field and the exact file length is checked, so a
/// truncated, extended, wrong-key-type, or unknown-version file fails closed.
fn agent_id_from_persisted_key(key_path: &Path) -> Option<String> {
    let path_metadata = std::fs::symlink_metadata(key_path).ok()?;
    if !path_metadata.file_type().is_file() {
        return None;
    }

    let mut key_file = File::open(key_path).ok()?;
    let file_len = key_file.metadata().ok()?.len();

    let mut prefix = [0_u8; X0X_KEYFILE_V2_MAGIC.len()];
    key_file.read_exact(&mut prefix).ok()?;

    let is_v2 = prefix == X0X_KEYFILE_V2_MAGIC;
    let expected_file_len = if is_v2 {
        X0X_AGENT_KEYFILE_V2_BYTES
    } else {
        X0X_AGENT_KEYFILE_V1_BYTES
    };
    if file_len != expected_file_len {
        return None;
    }

    let mut public_len_bytes = [0_u8; X0X_KEYFILE_LEN_PREFIX_BYTES];
    if is_v2 {
        key_file.read_exact(&mut public_len_bytes).ok()?;
    } else {
        public_len_bytes[..prefix.len()].copy_from_slice(&prefix);
        key_file
            .read_exact(&mut public_len_bytes[prefix.len()..])
            .ok()?;
    }
    if u64::from_le_bytes(public_len_bytes) != X0X_AGENT_PUBLIC_KEY_BYTES as u64 {
        return None;
    }

    let mut public_key = [0_u8; X0X_AGENT_PUBLIC_KEY_BYTES];
    key_file.read_exact(&mut public_key).ok()?;

    let mut secret_len_bytes = [0_u8; X0X_KEYFILE_LEN_PREFIX_BYTES];
    key_file.read_exact(&mut secret_len_bytes).ok()?;
    if u64::from_le_bytes(secret_len_bytes) != X0X_AGENT_SECRET_KEY_BYTES {
        return None;
    }

    let mut hasher = Sha256::new();
    hasher.update(X0X_AGENT_ID_DOMAIN);
    hasher.update(public_key);
    Some(hex::encode(hasher.finalize()))
}

/// Recover one stopped child's durable native identity from paths anchored to
/// its stable managed-record key. This never consults display names, cached
/// rosters, or any other mutable product metadata.
fn recover_managed_agent_child_identity(
    record_key: &str,
    key_path: &Path,
    data_dir: PathBuf,
) -> Option<ManagedAgentChild> {
    if !is_canonical_lower_hex_id(record_key) {
        return None;
    }
    let agent_id = agent_id_from_persisted_key(key_path)?;
    if agent_id == record_key {
        return None;
    }
    Some(ManagedAgentChild { agent_id, data_dir })
}

fn is_canonical_lower_hex_id(candidate: &str) -> bool {
    candidate.len() == 64
        && candidate
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

// ── Supervisor ──────────────────────────────────────────────────────────────

pub(super) struct AgentChildSupervisor<P: DaemonProbe, S: SidecarSpawner, T: TimeSource> {
    cfg: AgentChildConfig,
    probe: P,
    spawner: S,
    time: T,
}

impl<P: DaemonProbe, S: SidecarSpawner, T: TimeSource> AgentChildSupervisor<P, S, T> {
    pub(super) fn new(cfg: AgentChildConfig, probe: P, spawner: S, time: T) -> Self {
        Self {
            cfg,
            probe,
            spawner,
            time,
        }
    }

    /// Bring up (or attach to) the managed-agent x0xd child and fetch its
    /// signed `AgentCard.agent_id`. The daemon token is dropped on return;
    /// callers re-read it per request via [`AgentChildHandle::transient_token`].
    pub(super) fn bring_up(&self) -> Result<AgentChildHandle, AgentChildError> {
        let data_dir = self.cfg.data_dir.clone();

        // 1. Attach to a healthy named instance, else spawn and own one.
        let (child, base_url) = match self.try_attach(&data_dir)? {
            Some(base_url) => (None, base_url),
            None => {
                let child = self.spawn_child()?;
                let base_url = self.wait_ready(&data_dir)?;
                (Some(child), base_url)
            }
        };

        // 2. Fetch the child's signed AgentCard.agent_id (token read transiently).
        let agent_id = self.fetch_agent_id(&data_dir, &base_url)?;

        Ok(AgentChildHandle {
            child,
            base_url,
            data_dir,
            agent_id,
        })
    }

    /// Attach when `api.port` + `api-token` are present and bearer `/health`
    /// is OK. Returns the base URL, or `None` to spawn instead.
    fn try_attach(&self, data_dir: &Path) -> Result<Option<String>, AgentChildError> {
        let Some(port) = read_child_port(data_dir) else {
            return Ok(None);
        };
        let Some(token) = read_child_token(data_dir) else {
            return Ok(None);
        };
        let base_url = loopback_api_base(port);
        match self.probe.health(&base_url, &token) {
            Ok(()) => Ok(Some(base_url)),
            Err(_) => Ok(None),
        }
    }

    fn spawn_child(&self) -> Result<OwnedChild, AgentChildError> {
        let cmd = SidecarCommand {
            label: "x0xd",
            binary: self.cfg.binary.clone(),
            args: vec![
                // Named instance ⇒ isolated identity + data dir
                // (`<data_dir>/x0x-managed-<instance>/`). The child generates
                // and persists its OWN AgentCard here on first run.
                "--name".to_string(),
                self.instance_name_from_data_dir(),
                // Keep the child stable under our ownership: a startup
                // self-update+restart would orphan our tracked PID.
                "--skip-update-check".to_string(),
            ],
            env: Vec::new(),
            log_path: Some(self.cfg.data_dir.join("x0xd.log")),
        };
        self.spawner
            .spawn(&cmd)
            .map_err(AgentChildError::from_spawn)
    }

    /// Recover the `--name` value (`managed-<instance>`) from the resolved data
    /// dir's final component (`x0x-managed-<instance>`). Keeps the instance name
    /// and the on-disk dir in lockstep with x0xd's own `--name` derivation.
    fn instance_name_from_data_dir(&self) -> String {
        self.cfg
            .data_dir
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| n.strip_prefix("x0x-"))
            .map(str::to_string)
            .unwrap_or_else(|| "managed-agent".to_string())
    }

    /// Bounded-poll the data dir for port + token artifacts and bearer health.
    fn wait_ready(&self, data_dir: &Path) -> Result<String, AgentChildError> {
        let deadline = self.time.now() + self.cfg.timeout;
        loop {
            if let (Some(port), Some(token)) =
                (read_child_port(data_dir), read_child_token(data_dir))
            {
                let base_url = loopback_api_base(port);
                if self.probe.health(&base_url, &token).is_ok() {
                    return Ok(base_url);
                }
            }
            if self.time.now() >= deadline {
                return Err(AgentChildError::Timeout {
                    stage: AgentChildStage::Health,
                });
            }
            self.time.sleep(POLL_INTERVAL);
        }
    }

    /// Authenticated `GET /agent` → signed `AgentCard.agent_id` (64 lowercase
    /// hex). Mirrors the owner-daemon identity fetch, validated as a canonical
    /// agent_id. The token is read transiently and dropped on return.
    fn fetch_agent_id(&self, data_dir: &Path, base_url: &str) -> Result<String, AgentChildError> {
        let deadline = self.time.now() + self.cfg.timeout;
        let url = format!("{base_url}/agent");
        loop {
            if let Some(token) = read_child_token(data_dir) {
                match http_get_json(&url, Some(&token)) {
                    Ok(value) => {
                        if let Some(id) = extract_agent_id(&value) {
                            return Ok(id);
                        }
                        // Present but malformed — fail fast rather than spin.
                        return Err(AgentChildError::InvalidAgentCard {
                            reason: "/agent response missing a valid agent_id".to_string(),
                        });
                    }
                    Err(ProbeError::Unreachable) | Err(ProbeError::Malformed) => {
                        // Not ready yet; keep polling until the deadline.
                    }
                    Err(ProbeError::Unhealthy) => {
                        return Err(AgentChildError::InvalidAgentCard {
                            reason: "/agent rejected the child token".to_string(),
                        });
                    }
                }
            }
            if self.time.now() >= deadline {
                return Err(AgentChildError::Timeout {
                    stage: AgentChildStage::AgentCard,
                });
            }
            self.time.sleep(POLL_INTERVAL);
        }
    }
}

/// Pull a canonical 64-lowercase-hex `agent_id` out of a `GET /agent` body,
/// tolerating both the flat (`{agent_id}`) and wrapped (`{data:{agent_id}}`)
/// shapes the daemon may serialize. Rejects anything that is not exactly
/// 64 lowercase hex so a malformed child can never become a runtime identity.
pub(super) fn extract_agent_id(value: &Value) -> Option<String> {
    let candidate = value.get("agent_id").and_then(|v| v.as_str()).or_else(|| {
        value
            .get("data")
            .and_then(|d| d.get("agent_id"))
            .and_then(|v| v.as_str())
    })?;
    let id = candidate.trim();
    if is_canonical_lower_hex_id(id) {
        Some(id.to_string())
    } else {
        None
    }
}

/// Public Company-facing identity summary. The API token remains transient and
/// is intentionally absent from this serializable shape.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CompanyAgentIdentity {
    pub(crate) role: String,
    pub(crate) agent_id: String,
    pub(crate) data_dir: PathBuf,
}

static COMPANY_AGENT_CHILDREN: LazyLock<Mutex<HashMap<String, AgentChildHandle>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Ensure every Company role has a dedicated named x0xd identity. Each role's
/// child has an isolated data directory and token artifact; only its AgentId is
/// returned to the product layer. Handles are retained solely for process
/// ownership and are never a protocol-state store.
///
/// Fail-closed for orphans: if any role fails to come up, the handles inserted
/// during THIS call are dropped (reaping their owned children) before returning
/// the error. A resumable identity failure therefore owns no process; a resume
/// re-provisions every role (attaching to a warm child data dir, or spawning).
pub(crate) fn provision_company_agent_identities(
    instance_id: &str,
    roles: &[crate::company_template::spec::RoleSpec],
) -> Result<Vec<CompanyAgentIdentity>, AgentChildError> {
    let instance = crate::company_template::plan::instance_key(instance_id);
    let mut identities = Vec::with_capacity(roles.len());
    let mut children = COMPANY_AGENT_CHILDREN
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    // Keys freshly inserted during THIS call. On any failure we drop them so a
    // resumable run owns no partial/owned children.
    let mut inserted: Vec<String> = Vec::new();

    for role in roles {
        let key = format!(
            "{instance}-{}",
            crate::company_template::plan::instance_key(&role.id)
        );
        if let Some(handle) = children.get(&key) {
            identities.push(CompanyAgentIdentity {
                role: role.id.clone(),
                agent_id: handle.agent_id.clone(),
                data_dir: handle.data_dir.clone(),
            });
            continue;
        }

        let cfg = match AgentChildConfig::resolve(&key) {
            Ok(cfg) => cfg,
            Err(error) => {
                drop_partial_company_children(&mut children, &inserted);
                return Err(error);
            }
        };
        if let Err(error) = std::fs::create_dir_all(&cfg.data_dir) {
            drop_partial_company_children(&mut children, &inserted);
            return Err(AgentChildError::SpawnFailed {
                sidecar: "x0xd",
                reason: format!("failed to create isolated data directory: {error}"),
            });
        }
        let supervisor = std_supervisor(cfg);
        match supervisor.bring_up() {
            Ok(handle) => {
                identities.push(CompanyAgentIdentity {
                    role: role.id.clone(),
                    agent_id: handle.agent_id.clone(),
                    data_dir: handle.data_dir.clone(),
                });
                children.insert(key.clone(), handle);
                inserted.push(key);
            }
            Err(error) => {
                drop_partial_company_children(&mut children, &inserted);
                return Err(error);
            }
        }
    }
    Ok(identities)
}

/// Remove and drop the handles inserted during a provisioning attempt so their
/// owned children are reaped. A resumable identity failure must leave no owned
/// process behind; resume re-provisions (attaching to warm data dirs or
/// re-spawning). Identities from a prior successful call are left untouched.
fn drop_partial_company_children(
    children: &mut HashMap<String, AgentChildHandle>,
    inserted: &[String],
) {
    for key in inserted {
        children.remove(key);
    }
}

/// Release app-owned native x0xd children for one Company instance. Attached
/// warm daemons are left running by [`AgentChildHandle::shutdown`].
pub(crate) fn shutdown_company_agent_identities(instance_id: &str) {
    let prefix = format!(
        "{}-",
        crate::company_template::plan::instance_key(instance_id)
    );
    let mut children = COMPANY_AGENT_CHILDREN
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    children.retain(|key, _| !key.starts_with(&prefix));
}

/// Release every app-owned per-role x0xd child during desktop shutdown.
pub(crate) fn shutdown_all_company_agent_identities() {
    COMPANY_AGENT_CHILDREN
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clear();
}

// ── Individual (non-Company) managed-agent child identities ────────────────
//
// Each interactive managed agent that is NOT part of a Company template still
// gets a dedicated x0xd child so observer telemetry + control travel over
// authenticated PQC direct messaging (child AgentId → owner AgentId) instead of
// the legacy kind:24200 relay. The child is keyed by the agent's stable pubkey
// (hex, filesystem-safe) so its identity + data dir persist across restarts.

/// Native identity of one individually-managed agent, surfaced to the product
/// layer. Carries NO token (callers re-read it per request from `data_dir`).
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ManagedAgentChild {
    pub(crate) agent_id: String,
    pub(crate) data_dir: PathBuf,
}

static MANAGED_AGENT_CHILDREN: LazyLock<Mutex<HashMap<String, AgentChildHandle>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Stable per-agent filesystem instance key (the `x0x-managed-<instance>` dir
/// anchor). Hex pubkeys are filesystem-safe; lowercased for canonical equality.
fn managed_agent_instance(pubkey: &str) -> String {
    pubkey.trim().to_ascii_lowercase()
}

/// Bring up (or reuse) the dedicated x0xd child for one managed agent and
/// return its native identity. Idempotent: a warm child from a prior session is
/// attached, not re-spawned. The handle is retained for process ownership only.
pub(crate) fn provision_managed_agent_child(
    pubkey: &str,
) -> Result<ManagedAgentChild, AgentChildError> {
    let instance = managed_agent_instance(pubkey);
    let mut children = MANAGED_AGENT_CHILDREN
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(handle) = children.get(&instance) {
        if handle.agent_id == instance || !is_canonical_lower_hex_id(&handle.agent_id) {
            return Err(AgentChildError::InvalidAgentCard {
                reason:
                    "child native identity must be canonical and differ from its managed record key"
                        .to_string(),
            });
        }
        return Ok(ManagedAgentChild {
            agent_id: handle.agent_id.clone(),
            data_dir: handle.data_dir.clone(),
        });
    }
    let cfg = AgentChildConfig::resolve(&instance)?;
    std::fs::create_dir_all(&cfg.data_dir).map_err(|error| AgentChildError::SpawnFailed {
        sidecar: "x0xd",
        reason: format!("failed to create isolated data directory: {error}"),
    })?;
    let supervisor = std_supervisor(cfg);
    let handle = supervisor.bring_up()?;
    if handle.agent_id == instance || !is_canonical_lower_hex_id(&handle.agent_id) {
        return Err(AgentChildError::InvalidAgentCard {
            reason:
                "child native identity must be canonical and differ from its managed record key"
                    .to_string(),
        });
    }
    let identity = ManagedAgentChild {
        agent_id: handle.agent_id.clone(),
        data_dir: handle.data_dir.clone(),
    };
    children.insert(instance, handle);
    Ok(identity)
}

/// Bring up a managed-agent child only when its durable identity already
/// exists and still matches the identity observed by startup reconciliation.
///
/// Unlike [`provision_managed_agent_child`], this path never creates the data
/// directory and performs a second key-file recovery immediately before the
/// daemon is started. The exact AgentId is checked again after bring-up, so a
/// stale or replaced key can never be accepted as the intended group member.
pub(crate) fn bring_up_existing_managed_agent_child(
    pubkey: &str,
    expected_agent_id: &str,
) -> Result<ManagedAgentChild, AgentChildError> {
    let instance = managed_agent_instance(pubkey);
    let mut children = MANAGED_AGENT_CHILDREN
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(handle) = children.get(&instance) {
        if handle.agent_id != expected_agent_id
            || handle.agent_id == instance
            || !is_canonical_lower_hex_id(&handle.agent_id)
        {
            return Err(AgentChildError::InvalidAgentCard {
                reason: "running child identity no longer matches the durable startup identity"
                    .to_string(),
            });
        }
        return Ok(ManagedAgentChild {
            agent_id: handle.agent_id.clone(),
            data_dir: handle.data_dir.clone(),
        });
    }

    let bounded_instance = bounded_agent_child_instance(&instance);
    let home_dir = dirs::home_dir().ok_or(AgentChildError::NoDataDir)?;
    let key_path = agent_child_identity_dir(&home_dir, &bounded_instance).join("agent.key");
    let data_dir = agent_child_data_dir(&bounded_instance).ok_or(AgentChildError::NoDataDir)?;
    let recovered = recover_managed_agent_child_identity(&instance, &key_path, data_dir.clone())
        .filter(|identity| identity.agent_id == expected_agent_id)
        .ok_or_else(|| AgentChildError::InvalidAgentCard {
            reason: "durable managed-agent identity disappeared or changed before startup"
                .to_string(),
        })?;

    let cfg = AgentChildConfig::resolve(&instance)?;
    if cfg.data_dir != recovered.data_dir {
        return Err(AgentChildError::InvalidAgentCard {
            reason: "durable managed-agent identity resolved to an unexpected data directory"
                .to_string(),
        });
    }
    let supervisor = std_supervisor(cfg);
    let handle = supervisor.bring_up()?;
    if handle.agent_id != expected_agent_id
        || handle.agent_id == instance
        || !is_canonical_lower_hex_id(&handle.agent_id)
    {
        return Err(AgentChildError::InvalidAgentCard {
            reason: "started child identity does not match the durable startup identity"
                .to_string(),
        });
    }
    let identity = ManagedAgentChild {
        agent_id: handle.agent_id.clone(),
        data_dir: handle.data_dir.clone(),
    };
    children.insert(instance, handle);
    Ok(identity)
}

/// Look up the native identity for a managed agent. A live handle is the
/// authoritative fast path; after stop/desktop restart, recover the same
/// identity without spawning by reading the bounded named-instance key file.
pub(crate) fn managed_agent_child_identity(pubkey: &str) -> Option<ManagedAgentChild> {
    let instance = managed_agent_instance(pubkey);
    {
        let children = MANAGED_AGENT_CHILDREN
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(handle) = children.get(&instance) {
            if handle.agent_id == instance || !is_canonical_lower_hex_id(&handle.agent_id) {
                return None;
            }
            return Some(ManagedAgentChild {
                agent_id: handle.agent_id.clone(),
                data_dir: handle.data_dir.clone(),
            });
        }
    }

    let bounded_instance = bounded_agent_child_instance(&instance);
    let home_dir = dirs::home_dir()?;
    let key_path = agent_child_identity_dir(&home_dir, &bounded_instance).join("agent.key");
    let data_dir = agent_child_data_dir(&bounded_instance)?;
    recover_managed_agent_child_identity(&instance, &key_path, data_dir)
}

/// Release the app-owned x0xd child for one managed agent. Called from the
/// per-agent full-stop path ([`crate::managed_agents::runtime::stop_managed_agent_process`])
/// and from agent delete. Dropping the handle reaps the child process; the
/// identity + data dir persist (keyed by the stable pubkey) for a re-provision.
pub(crate) fn shutdown_managed_agent_child(pubkey: &str) {
    let instance = managed_agent_instance(pubkey);
    let mut children = MANAGED_AGENT_CHILDREN
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    children.remove(&instance);
}

/// Release every app-owned individual managed-agent child on desktop shutdown.
pub(crate) fn shutdown_all_managed_agent_children() {
    MANAGED_AGENT_CHILDREN
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clear();
}

/// Default production supervisor: real probe + spawner + wall clock.
pub(super) fn std_supervisor(
    cfg: AgentChildConfig,
) -> AgentChildSupervisor<impl DaemonProbe, impl SidecarSpawner, impl TimeSource> {
    AgentChildSupervisor::new(
        cfg,
        crate::local_stack::LoopbackHttpDaemonProbe,
        StdSidecarSpawner,
        crate::local_stack::BlockingTimeSource,
    )
}

#[cfg(test)]
mod persisted_identity_tests {
    use super::*;

    const EXPECTED_TEST_AGENT_ID: &str =
        "72304d2d521ec7647603869401a2f052bc23d7365bfd5256180dd30b591dd9cd";

    fn keyfile_fixture(v2: bool) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(if v2 {
            X0X_AGENT_KEYFILE_V2_BYTES as usize
        } else {
            X0X_AGENT_KEYFILE_V1_BYTES as usize
        });
        if v2 {
            bytes.extend_from_slice(&X0X_KEYFILE_V2_MAGIC);
        }
        bytes.extend_from_slice(&(X0X_AGENT_PUBLIC_KEY_BYTES as u64).to_le_bytes());
        bytes.extend_from_slice(&vec![0xa5; X0X_AGENT_PUBLIC_KEY_BYTES]);
        bytes.extend_from_slice(&X0X_AGENT_SECRET_KEY_BYTES.to_le_bytes());
        bytes.extend_from_slice(&vec![0x5a; X0X_AGENT_SECRET_KEY_BYTES as usize]);
        if v2 {
            bytes.extend_from_slice(&1_900_000_000_u64.to_le_bytes());
        }
        bytes
    }

    fn write_fixture(path: &Path, bytes: &[u8]) {
        std::fs::write(path, bytes).expect("write test agent.key");
    }

    #[test]
    fn stopped_child_recovers_native_identity_from_v1_key_without_a_handle() {
        let temp = tempfile::tempdir().expect("tempdir");
        let key_path = temp.path().join("agent.key");
        write_fixture(&key_path, &keyfile_fixture(false));
        let data_dir = temp.path().join("data");
        let record_key = "11".repeat(32);

        let identity =
            recover_managed_agent_child_identity(&record_key, &key_path, data_dir.clone())
                .expect("valid stopped-child identity");

        assert_eq!(identity.agent_id, EXPECTED_TEST_AGENT_ID);
        assert_eq!(identity.data_dir, data_dir);
        assert_ne!(identity.agent_id, record_key);
    }

    #[test]
    fn stopped_child_recovers_native_identity_from_v2_key() {
        let temp = tempfile::tempdir().expect("tempdir");
        let key_path = temp.path().join("agent.key");
        write_fixture(&key_path, &keyfile_fixture(true));

        assert_eq!(
            agent_id_from_persisted_key(&key_path).as_deref(),
            Some(EXPECTED_TEST_AGENT_ID)
        );
    }

    #[test]
    fn persisted_identity_rejects_corrupt_truncated_and_wrong_key_envelopes() {
        let temp = tempfile::tempdir().expect("tempdir");
        let key_path = temp.path().join("agent.key");

        let mut truncated = keyfile_fixture(false);
        truncated.pop();
        write_fixture(&key_path, &truncated);
        assert!(agent_id_from_persisted_key(&key_path).is_none());

        let mut wrong_public_size = keyfile_fixture(false);
        wrong_public_size[..X0X_KEYFILE_LEN_PREFIX_BYTES].copy_from_slice(&1_184_u64.to_le_bytes());
        write_fixture(&key_path, &wrong_public_size);
        assert!(agent_id_from_persisted_key(&key_path).is_none());

        let mut wrong_secret_size = keyfile_fixture(false);
        let secret_len_offset = X0X_KEYFILE_LEN_PREFIX_BYTES + X0X_AGENT_PUBLIC_KEY_BYTES;
        wrong_secret_size[secret_len_offset..secret_len_offset + X0X_KEYFILE_LEN_PREFIX_BYTES]
            .copy_from_slice(&2_528_u64.to_le_bytes());
        write_fixture(&key_path, &wrong_secret_size);
        assert!(agent_id_from_persisted_key(&key_path).is_none());

        let mut extended = keyfile_fixture(false);
        extended.push(0);
        write_fixture(&key_path, &extended);
        assert!(agent_id_from_persisted_key(&key_path).is_none());

        let mut unknown_version = keyfile_fixture(true);
        unknown_version[..X0X_KEYFILE_V2_MAGIC.len()].copy_from_slice(b"X0K3");
        write_fixture(&key_path, &unknown_version);
        assert!(agent_id_from_persisted_key(&key_path).is_none());
    }

    #[test]
    fn persisted_identity_rejects_record_child_equality_and_non_record_keys() {
        let temp = tempfile::tempdir().expect("tempdir");
        let key_path = temp.path().join("agent.key");
        write_fixture(&key_path, &keyfile_fixture(false));

        assert!(recover_managed_agent_child_identity(
            EXPECTED_TEST_AGENT_ID,
            &key_path,
            temp.path().join("equal")
        )
        .is_none());
        assert!(recover_managed_agent_child_identity(
            "Guide",
            &key_path,
            temp.path().join("display-name")
        )
        .is_none());
        assert!(recover_managed_agent_child_identity(
            &"AA".repeat(32),
            &key_path,
            temp.path().join("noncanonical")
        )
        .is_none());
    }

    #[cfg(unix)]
    #[test]
    fn persisted_identity_does_not_follow_symlinks() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().expect("tempdir");
        let real_key_path = temp.path().join("real-agent.key");
        let symlink_path = temp.path().join("agent.key");
        write_fixture(&real_key_path, &keyfile_fixture(false));
        symlink(&real_key_path, &symlink_path).expect("create key symlink");

        assert!(agent_id_from_persisted_key(&symlink_path).is_none());
    }

    #[test]
    fn durable_identity_path_matches_bounded_x0xd_named_instance() {
        let record_key = "ab".repeat(32);
        let bounded = bounded_agent_child_instance(&record_key);
        let home = Path::new("/Users/tester");

        assert_eq!(
            agent_child_identity_dir(home, &bounded).join("agent.key"),
            home.join(format!(".x0x-managed-{bounded}/agent.key"))
        );
    }
}

#[cfg(test)]
#[path = "agent_identity_tests.rs"]
mod tests;
