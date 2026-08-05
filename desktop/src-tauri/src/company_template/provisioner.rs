//! The provisioner boundary: a trait abstracting the native x0xd
//! group/store/task-list APIs, plus a deterministic recording test double and
//! an HTTP-backed implementation generic over an [`X0xTransport`].
//!
//! ## Why a trait
//! The real x0xd HTTP client (`crate::x0x_client::X0xClient`, owned by
//! `AppState`) is built by a sibling agent and resolves the transient daemon
//! bearer token per call. Decoupling behind [`CompanyProvisioner`] lets the
//! template planner/executor compile and unit-test today against
//! [`RecordingProvisioner`], with a single one-line adapter
//! (`impl X0xTransport for X0xClient`) wiring the live daemon later.
//!
//! ## Idempotency
//! x0xd exposes **no** idempotency key, so `ensure_*` is made idempotent in
//! this layer:
//! - groups: dedup by `name` via `GET /groups` (adopt first match), else create;
//! - stores / task-lists: rely on x0xd `409 CONFLICT` (keyed by `topic`) to
//!   detect pre-existence and adopt.
//!
//! Combined with the durable step manifest, a replay skips or adopts every
//! resource — never creating a duplicate.
//!
//! No relay events are emitted by anything in this module.

use std::future::Future;
use std::pin::Pin;

use serde_json::{json, Value};

use crate::company_template::spec::{GroupVisibility, StorePolicy};

/// Boxed, `Send` future used by provisioner methods so trait methods are
/// object-safe and usable from Tauri's multi-threaded runtime through a
/// generic call site.
pub type BoxFut<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Error from a provisioning call. Mirrors the daemon transport's error space.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProvisionError {
    /// The local x0xd daemon is unreachable / not yet brought up.
    DaemonUnavailable(String),
    /// A transport-level failure (connection, TLS, timeout).
    Transport(String),
    /// A non-success HTTP status. `409` is expected for stores/task-lists and
    /// is intercepted by the `ensure_*` wrappers as "already exists".
    Status { code: u16, body: String },
    /// The response body could not be decoded into the expected shape.
    Decode(String),
}

impl std::fmt::Display for ProvisionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProvisionError::DaemonUnavailable(m) => write!(f, "x0xd daemon unavailable: {m}"),
            ProvisionError::Transport(m) => write!(f, "x0xd transport error: {m}"),
            ProvisionError::Status { code, body } => {
                write!(f, "x0xd returned status {code}: {body}")
            }
            ProvisionError::Decode(m) => write!(f, "x0xd response decode error: {m}"),
        }
    }
}

impl std::error::Error for ProvisionError {}

/// Result of an idempotent `ensure_*` call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnsureResult {
    /// The x0xd resource id (group_id / store topic / task-list topic).
    pub id: String,
    /// `true` if the resource was newly created this call; `false` if it
    /// pre-existed and was adopted.
    pub created: bool,
}

/// Request to ensure a native x0xd group exists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnsureGroupRequest {
    /// The x0xd group `name` (info.name) — the dedup key.
    pub name: String,
    /// Group description (x0xd `description`).
    pub description: String,
    pub visibility: GroupVisibility,
    /// Manifest/dedup key for tracing. NOT transmitted to x0xd (it has no
    /// idempotency-key API).
    pub idempotency_key: String,
}

/// Request to ensure a native x0xd store exists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnsureStoreRequest {
    pub name: String,
    /// The x0xd store `topic` (== the returned id) — the dedup key.
    pub topic: String,
    pub policy: StorePolicy,
}

/// Request to ensure a native x0xd task-list exists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnsureTaskListRequest {
    pub name: String,
    /// The x0xd task-list `topic` (== the returned id) — the dedup key.
    pub topic: String,
}

/// The provisioning boundary. Implementations call native x0xd REST APIs.
///
/// Methods return boxed `Send` futures (see [`BoxFut`]) so the trait is usable
/// from Tauri's multi-threaded runtime through a generic executor without
/// requiring `async-trait`.
pub trait CompanyProvisioner: Send + Sync {
    fn ensure_group(
        &self,
        req: EnsureGroupRequest,
    ) -> BoxFut<'_, Result<EnsureResult, ProvisionError>>;
    fn ensure_store(
        &self,
        req: EnsureStoreRequest,
    ) -> BoxFut<'_, Result<EnsureResult, ProvisionError>>;
    fn ensure_task_list(
        &self,
        req: EnsureTaskListRequest,
    ) -> BoxFut<'_, Result<EnsureResult, ProvisionError>>;
}

// ── Recording test double ──────────────────────────────────────────────────

/// A stateless-by-default provisioner that records call counts via atomics and
/// returns deterministic ids. Used by unit tests to assert idempotency
/// (a replayed instantiation must issue **zero** new provisioning calls).
///
/// Determinism: the returned id for a request is derived only from the request
/// fields, so identical requests yield identical ids.
#[cfg(test)]
pub struct RecordingProvisioner {
    group_calls: std::sync::atomic::AtomicUsize,
    store_calls: std::sync::atomic::AtomicUsize,
    task_list_calls: std::sync::atomic::AtomicUsize,
    /// When set, the next N calls fail with `DaemonUnavailable` to exercise
    /// the resumable-failure path.
    fail_next: std::sync::atomic::AtomicUsize,
}

#[cfg(test)]
impl Default for RecordingProvisioner {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
impl RecordingProvisioner {
    pub fn new() -> Self {
        Self {
            group_calls: 0.into(),
            store_calls: 0.into(),
            task_list_calls: 0.into(),
            fail_next: 0.into(),
        }
    }

    pub fn fail_next(&self, count: usize) {
        self.fail_next
            .store(count, std::sync::atomic::Ordering::SeqCst);
    }

    pub fn group_calls(&self) -> usize {
        self.group_calls.load(std::sync::atomic::Ordering::SeqCst)
    }

    pub fn store_calls(&self) -> usize {
        self.store_calls.load(std::sync::atomic::Ordering::SeqCst)
    }

    pub fn task_list_calls(&self) -> usize {
        self.task_list_calls
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    pub fn total_calls(&self) -> usize {
        self.group_calls() + self.store_calls() + self.task_list_calls()
    }

    fn maybe_fail(&self) -> Result<(), ProvisionError> {
        use std::sync::atomic::Ordering;
        // Decrement-then-check; if it was already 0, no failure.
        let prev = self
            .fail_next
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |v| {
                if v > 0 {
                    Some(v - 1)
                } else {
                    None
                }
            });
        if prev.is_ok() {
            Err(ProvisionError::DaemonUnavailable(
                "simulated daemon down (RecordingProvisioner)".into(),
            ))
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
impl CompanyProvisioner for RecordingProvisioner {
    fn ensure_group(
        &self,
        req: EnsureGroupRequest,
    ) -> BoxFut<'_, Result<EnsureResult, ProvisionError>> {
        self.group_calls
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let fail = self.maybe_fail();
        let id = deterministic_id("group", &format!("{}|{}", req.name, req.visibility));
        Box::pin(async move {
            fail?;
            Ok(EnsureResult { id, created: true })
        })
    }

    fn ensure_store(
        &self,
        req: EnsureStoreRequest,
    ) -> BoxFut<'_, Result<EnsureResult, ProvisionError>> {
        self.store_calls
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let fail = self.maybe_fail();
        let id = deterministic_id("store", &req.topic);
        Box::pin(async move {
            fail?;
            Ok(EnsureResult { id, created: true })
        })
    }

    fn ensure_task_list(
        &self,
        req: EnsureTaskListRequest,
    ) -> BoxFut<'_, Result<EnsureResult, ProvisionError>> {
        self.task_list_calls
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let fail = self.maybe_fail();
        let id = deterministic_id("tasklist", &req.topic);
        Box::pin(async move {
            fail?;
            Ok(EnsureResult { id, created: true })
        })
    }
}

/// Deterministic resource id derived from a kind tag + canonical key.
#[cfg(test)]
fn deterministic_id(kind: &str, key: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(kind.as_bytes());
    h.update(b":");
    h.update(key.as_bytes());
    format!("{kind}-{}", &hex::encode(h.finalize())[..12])
}

// ── HTTP-backed provisioner (generic over X0xTransport) ────────────────────

/// Minimal authenticated transport the HTTP provisioner needs. The live
/// `crate::x0x_client::X0xClient` implements this via a one-line adapter:
/// `impl X0xTransport for X0xClient { ... }`.
///
/// `GET`/`POST` target the loopback daemon and carry the transient bearer
/// token; the adapter owns that plumbing.
pub trait X0xTransport: Send + Sync {
    fn get_json(&self, path: &str) -> BoxFut<'_, Result<Value, ProvisionError>>;
    fn post_json(&self, path: &str, body: Value) -> BoxFut<'_, Result<Value, ProvisionError>>;
}

impl X0xTransport for crate::x0x_client::X0xClient {
    fn get_json(&self, path: &str) -> BoxFut<'_, Result<Value, ProvisionError>> {
        let path = path.to_string();
        Box::pin(async move {
            crate::x0x_client::X0xClient::get_json(self, &path, &[])
                .await
                .map_err(map_x0x_error)
        })
    }

    fn post_json(&self, path: &str, body: Value) -> BoxFut<'_, Result<Value, ProvisionError>> {
        let path = path.to_string();
        Box::pin(async move {
            crate::x0x_client::X0xClient::post_json(self, &path, &body)
                .await
                .map_err(map_x0x_error)
        })
    }
}

fn map_x0x_error(error: crate::x0x_client::X0xClientError) -> ProvisionError {
    match error {
        crate::x0x_client::X0xClientError::DaemonUnavailable(stage) => {
            ProvisionError::DaemonUnavailable(stage.to_string())
        }
        crate::x0x_client::X0xClientError::Transport(message) => ProvisionError::Transport(message),
        crate::x0x_client::X0xClientError::Status(code, body) => {
            ProvisionError::Status { code, body }
        }
        crate::x0x_client::X0xClientError::Decode(message) => ProvisionError::Decode(message),
    }
}

/// HTTP-backed [`CompanyProvisioner`] over any [`X0xTransport`].
pub struct X0xHttpProvisioner<'t, T: X0xTransport> {
    transport: &'t T,
}

impl<'t, T: X0xTransport> X0xHttpProvisioner<'t, T> {
    pub fn new(transport: &'t T) -> Self {
        Self { transport }
    }
}

impl<T: X0xTransport> CompanyProvisioner for X0xHttpProvisioner<'_, T> {
    fn ensure_group(
        &self,
        req: EnsureGroupRequest,
    ) -> BoxFut<'_, Result<EnsureResult, ProvisionError>> {
        let transport = self.transport;
        Box::pin(async move {
            // 1. Dedup by name via GET /groups.
            let listed = transport.get_json("/groups").await?;
            if let Some(existing) =
                listed
                    .get("groups")
                    .and_then(|g| g.as_array())
                    .and_then(|arr| {
                        arr.iter().find(|g| {
                            g.get("name").and_then(|n| n.as_str()) == Some(req.name.as_str())
                        })
                    })
            {
                let id = existing
                    .get("group_id")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        ProvisionError::Decode("GET /groups entry missing group_id".into())
                    })?
                    .to_string();
                return Ok(EnsureResult { id, created: false });
            }
            // 2. Create.
            let body = json!({
                "name": req.name,
                "description": req.description,
                "preset": req.visibility.as_x0xd_preset(),
            });
            let resp = transport.post_json("/groups", body).await?;
            let id = resp
                .get("group_id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ProvisionError::Decode("POST /groups missing group_id".into()))?
                .to_string();
            Ok(EnsureResult { id, created: true })
        })
    }

    fn ensure_store(
        &self,
        req: EnsureStoreRequest,
    ) -> BoxFut<'_, Result<EnsureResult, ProvisionError>> {
        let transport = self.transport;
        Box::pin(async move {
            let body = json!({
                "name": req.name,
                "topic": req.topic,
                "policy": req.policy.as_x0xd_policy(),
            });
            match transport.post_json("/stores", body).await {
                Ok(_) => Ok(EnsureResult {
                    id: req.topic,
                    created: true,
                }),
                Err(ProvisionError::Status { code: 409, .. }) => {
                    // x0xd keyed conflict on topic → already exists; adopt.
                    Ok(EnsureResult {
                        id: req.topic,
                        created: false,
                    })
                }
                Err(e) => Err(e),
            }
        })
    }

    fn ensure_task_list(
        &self,
        req: EnsureTaskListRequest,
    ) -> BoxFut<'_, Result<EnsureResult, ProvisionError>> {
        let transport = self.transport;
        Box::pin(async move {
            let body = json!({ "name": req.name, "topic": req.topic });
            match transport.post_json("/task-lists", body).await {
                Ok(_) => Ok(EnsureResult {
                    id: req.topic,
                    created: true,
                }),
                Err(ProvisionError::Status { code: 409, .. }) => Ok(EnsureResult {
                    id: req.topic,
                    created: false,
                }),
                Err(e) => Err(e),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn recording_provisioner_is_deterministic() {
        let p = RecordingProvisioner::new();
        let r1 = p
            .ensure_group(EnsureGroupRequest {
                name: "Engineering".into(),
                description: "".into(),
                visibility: GroupVisibility::PrivateSecure,
                idempotency_key: "k".into(),
            })
            .await
            .unwrap();
        let r2 = p
            .ensure_group(EnsureGroupRequest {
                name: "Engineering".into(),
                description: "".into(),
                visibility: GroupVisibility::PrivateSecure,
                idempotency_key: "k".into(),
            })
            .await
            .unwrap();
        assert_eq!(r1.id, r2.id, "ids must be deterministic for identical reqs");
        assert!(r1.created);
        assert_eq!(p.group_calls(), 2);
    }
}
