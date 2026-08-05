//! symphony_client — typed async REST/SSE client for the local x0x-symphonyd.
//!
//! Mirrors the daemon's wire shapes (`crates/x0x-symphony-bin/src/api.rs`) so
//! the desktop gets typed task/worker/status/approval/handoff/proof views
//! without linking the symphony crate. The daemon is always loopback and
//! bearer-authenticated; this client **fails closed** on any non-loopback
//! endpoint (defense in depth — the supervisor already binds `127.0.0.1:0`).
//!
//! The bearer token is read transiently from `<data-dir>/api-token` per call
//! site and never stored on the client beyond the constructor scope needed for
//! a single logical operation; it is redacted from all `Display`/error output.

use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use url::Url;

// ── Errors ──────────────────────────────────────────────────────────────────

/// Structured client error. Carries safe context only — **never** the token.
/// The HTTP variant preserves the daemon's status + body so callers can surface
/// `conflict`/`not_found`/`bad_request` distinctions to the operator.
#[derive(Debug)]
pub enum SymphonyClientError {
    /// The server URL was not a loopback `http://` endpoint. Fail-closed.
    NotLoopback { server: String },
    /// A request failed before a response was received (connect/read/timeout).
    Request { url: String, source: reqwest::Error },
    /// A success body could not be decoded into the expected type.
    Decode { url: String, source: reqwest::Error },
    /// The daemon returned a non-success status. `body` is the daemon's text.
    Http { status: u16, body: String },
}

impl fmt::Display for SymphonyClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotLoopback { server } => write!(
                f,
                "symphony endpoint must be loopback http://; refused {server}"
            ),
            Self::Request { url, source } => {
                write!(f, "symphony request to {url} failed: {source}")
            }
            Self::Decode { url, source } => {
                write!(f, "symphony decode from {url} failed: {source}")
            }
            Self::Http { status, body } => {
                let body = body.trim();
                if body.is_empty() {
                    write!(f, "symphony daemon returned HTTP {status}")
                } else {
                    write!(f, "symphony daemon returned HTTP {status}: {body}")
                }
            }
        }
    }
}

impl std::error::Error for SymphonyClientError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Request { source, .. } | Self::Decode { source, .. } => Some(source),
            _ => None,
        }
    }
}

pub type ClientResult<T> = Result<T, SymphonyClientError>;

// ── DTOs (mirror crates/x0x-symphony-bin/src/api.rs wire shapes) ────────────

/// Task row returned by `GET /symphony/tasks`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SymphonyTask {
    pub id: String,
    pub identifier: String,
    pub title: String,
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<u8>,
    pub labels: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claim_by: Option<String>,
    /// Tracker verification notices (opaque to the desktop).
    #[serde(default)]
    pub verification_notices: Vec<Value>,
}

/// Active claim row returned inside [`SymphonyStatus`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SymphonyClaimInfo {
    pub id: String,
    pub identifier: String,
    pub state: String,
    pub by: String,
    pub heartbeat_at: String,
}

/// Response returned by `GET /symphony/status`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SymphonyStatus {
    pub agent_id: String,
    pub counts: std::collections::BTreeMap<String, usize>,
    pub active_claims: Vec<SymphonyClaimInfo>,
    pub orchestrator_attached: bool,
}

/// Platform details on a worker card.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SymphonyPlatformInfo {
    pub os: String,
    pub arch: String,
    pub version: String,
}

/// One live worker-discovery card.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SymphonyWorkerCard {
    pub schema_version: u32,
    pub agent_id: String,
    pub issued_at: String,
    pub ttl_seconds: u64,
    pub capabilities: Vec<String>,
    pub sandbox_levels: Vec<String>,
    pub runner_presets: Vec<String>,
    pub current_load: u32,
    pub max_load: u32,
    pub platform: SymphonyPlatformInfo,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<Value>,
}

/// Response returned by `GET /symphony/workers`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SymphonyWorkers {
    pub workers: Vec<SymphonyWorkerCard>,
    pub view_epoch: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// One route returned by `GET /symphony/routes`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SymphonyRouteInfo {
    pub method: String,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SymphonyRoutes {
    pub routes: Vec<SymphonyRouteInfo>,
}

/// Response returned by `GET /symphony/proofs`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SymphonyProofList {
    pub proofs: Vec<String>,
}

/// Response returned by `GET /symphony/proofs/{name}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SymphonyProof {
    pub name: String,
    pub content: String,
}

/// Response returned by `POST /symphony/claim/{id}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SymphonyClaimResponse {
    pub id: String,
    pub by: String,
}

/// Request body for `POST /symphony/handoff/{id}`.
#[derive(Debug, Clone, Serialize)]
pub struct SymphonyHandoffRequest {
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
}

/// Response returned by `POST /symphony/handoff/{id}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SymphonyHandoffResponse {
    pub id: String,
    pub recorded: bool,
}

/// Request body for `POST /symphony/issues`.
#[derive(Debug, Clone, Serialize)]
pub struct SymphonyIssueDraft {
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub priority: Option<i32>,
    #[serde(default)]
    pub labels: Vec<String>,
}

/// Stored approval-record counts attached to a pending approval.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SymphonyApprovalSummary {
    pub events: usize,
    pub consumed: usize,
    pub has_deny: bool,
}

/// Serializable source-signature provenance (tagged `kind`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum SymphonyApprovalProvenance {
    Verified { signer_agent_id: String },
    Invalid { reason: String },
    TransportError { reason: String },
}

/// One issue awaiting operator approval (`GET /symphony/approvals/pending`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SymphonyPendingApproval {
    pub issue_id: String,
    pub title: String,
    pub state: String,
    pub content_hash: String,
    pub signer_agent_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance: Option<SymphonyApprovalProvenance>,
    pub approval_summary: SymphonyApprovalSummary,
}

/// Operator verdict for `POST /symphony/approvals/{id}`. Serialized as the
/// bare variant name (`"Approve"`/`"Deny"`) to match the daemon.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
pub enum ApprovalVerdict {
    Approve,
    Deny,
}

#[derive(Debug, Clone, Serialize)]
struct SubmitApprovalRequest {
    verdict: ApprovalVerdict,
    #[serde(skip_serializing_if = "Option::is_none")]
    expected_content_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    expected_signer_agent_id: Option<String>,
}

/// Approval/denial event returned by `POST /symphony/approvals/{id}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SymphonyApprovalEvent {
    pub issue_id: String,
    pub content_hash: String,
    pub signer_agent_id: String,
    pub verdict: ApprovalVerdict,
    pub approved_at: String,
    pub approver_agent_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claim_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<Value>,
}

/// One parsed SSE frame from `/symphony/events`.
#[derive(Debug, Clone)]
pub struct SymphonyEventFrame {
    /// SSE `event:` field (e.g. `task_claimed`, `handoff_recorded`,
    /// `dispatch_*`, or `heartbeat`).
    pub event: String,
    /// SSE `data:` field (raw text; usually JSON for real events, `ok`/`lagged`
    /// for heartbeats).
    pub data: String,
}

// ── Client ──────────────────────────────────────────────────────────────────

/// Authenticated client for the loopback symphony daemon. Constructed per
/// command from the supervised handle's base URL + the transiently-read token
/// + the shared app HTTP client.
#[derive(Clone)]
pub struct SymphonyClient {
    server: String,
    token: String,
    http: reqwest::Client,
}

impl SymphonyClient {
    /// Construct a client. Fails closed unless `server` is a loopback
    /// `http://` URL. The token is held only for this client's lifetime and is
    /// redacted from [`fmt::Debug`].
    pub fn new(server: &str, token: String, http: reqwest::Client) -> ClientResult<Self> {
        validate_loopback(server)?;
        Ok(Self {
            server: server.trim_end_matches('/').to_owned(),
            token,
            http,
        })
    }

    /// `GET /symphony/tasks`, optionally filtered by state (e.g. `todo`).
    pub async fn tasks(&self, state: Option<&str>) -> ClientResult<Vec<SymphonyTask>> {
        let path = match state {
            None => "/symphony/tasks".to_owned(),
            Some(s) => format!("/symphony/tasks?state={s}"),
        };
        self.get_json(&path).await
    }

    /// `GET /symphony/tasks/{id}` — full task detail (opaque JSON; the daemon
    /// flattens a rich `Issue` with adapter-specific `extra`).
    pub async fn task(&self, id: &str) -> ClientResult<Value> {
        self.get_json(&format!("/symphony/tasks/{id}")).await
    }

    /// `GET /symphony/status`.
    pub async fn status(&self) -> ClientResult<SymphonyStatus> {
        self.get_json("/symphony/status").await
    }

    /// `GET /symphony/workers`.
    pub async fn workers(&self) -> ClientResult<SymphonyWorkers> {
        self.get_json("/symphony/workers").await
    }

    /// `GET /symphony/approvals/pending`.
    pub async fn approvals_pending(&self) -> ClientResult<Vec<SymphonyPendingApproval>> {
        self.get_json("/symphony/approvals/pending").await
    }

    /// `POST /symphony/approvals/{id}` with an `Approve` verdict.
    pub async fn approve(
        &self,
        id: &str,
        expected_content_hash: Option<&str>,
        expected_signer_agent_id: Option<&str>,
    ) -> ClientResult<SymphonyApprovalEvent> {
        self.submit_approval(
            id,
            ApprovalVerdict::Approve,
            expected_content_hash,
            expected_signer_agent_id,
        )
        .await
    }

    /// `POST /symphony/approvals/{id}` with a `Deny` verdict.
    pub async fn deny(
        &self,
        id: &str,
        expected_content_hash: Option<&str>,
        expected_signer_agent_id: Option<&str>,
    ) -> ClientResult<SymphonyApprovalEvent> {
        self.submit_approval(
            id,
            ApprovalVerdict::Deny,
            expected_content_hash,
            expected_signer_agent_id,
        )
        .await
    }

    /// `POST /symphony/issues` — create a symphony-owned issue.
    pub async fn create_issue(&self, draft: &SymphonyIssueDraft) -> ClientResult<Value> {
        self.post_json("/symphony/issues", draft).await
    }

    /// `POST /symphony/claim/{id}` — claim an issue for this daemon's agent.
    pub async fn claim(&self, id: &str) -> ClientResult<SymphonyClaimResponse> {
        self.post_empty_json(&format!("/symphony/claim/{id}")).await
    }

    /// `POST /symphony/handoff/{id}` — record a handoff for a claimed issue.
    pub async fn handoff(
        &self,
        id: &str,
        message: String,
        file: Option<String>,
    ) -> ClientResult<SymphonyHandoffResponse> {
        let body = SymphonyHandoffRequest { message, file };
        self.post_json(&format!("/symphony/handoff/{id}"), &body)
            .await
    }

    /// `GET /symphony/routes` — the daemon's own route table.
    pub async fn routes(&self) -> ClientResult<SymphonyRoutes> {
        self.get_json("/symphony/routes").await
    }

    /// `GET /symphony/proofs` — proof artefact names.
    pub async fn proofs(&self) -> ClientResult<SymphonyProofList> {
        self.get_json("/symphony/proofs").await
    }

    /// `GET /symphony/proofs/{name}` — one proof artefact's UTF-8 content.
    pub async fn proof(&self, name: &str) -> ClientResult<SymphonyProof> {
        self.get_json(&format!("/symphony/proofs/{name}")).await
    }

    /// URL for the SSE event stream (`GET /symphony/events`). The token is
    /// carried as `?token=` per the daemon's EventSource-compatible auth
    /// exemption (browser `EventSource` cannot set headers). Used by the
    /// desktop SSE forwarder command.
    pub fn event_stream_url(&self) -> String {
        format!("{}/symphony/events?token={}", self.server, self.token)
    }

    async fn submit_approval(
        &self,
        id: &str,
        verdict: ApprovalVerdict,
        expected_content_hash: Option<&str>,
        expected_signer_agent_id: Option<&str>,
    ) -> ClientResult<SymphonyApprovalEvent> {
        let body = SubmitApprovalRequest {
            verdict,
            expected_content_hash: expected_content_hash.map(str::to_owned),
            expected_signer_agent_id: expected_signer_agent_id.map(str::to_owned),
        };
        self.post_json(&format!("/symphony/approvals/{id}"), &body)
            .await
    }

    async fn get_json<T: serde::de::DeserializeOwned>(&self, path: &str) -> ClientResult<T> {
        let url = self.url(path);
        let response = self
            .http
            .get(&url)
            .bearer_auth(&self.token)
            .header(reqwest::header::ACCEPT, "application/json")
            .send()
            .await
            .map_err(|source| SymphonyClientError::Request {
                url: url.clone(),
                source,
            })?;
        decode(response, &url).await
    }

    async fn post_empty_json<T: serde::de::DeserializeOwned>(&self, path: &str) -> ClientResult<T> {
        let url = self.url(path);
        let response = self
            .http
            .post(&url)
            .bearer_auth(&self.token)
            .send()
            .await
            .map_err(|source| SymphonyClientError::Request {
                url: url.clone(),
                source,
            })?;
        decode(response, &url).await
    }

    async fn post_json<T: serde::de::DeserializeOwned, B: serde::Serialize + ?Sized>(
        &self,
        path: &str,
        body: &B,
    ) -> ClientResult<T> {
        let url = self.url(path);
        let response = self
            .http
            .post(&url)
            .bearer_auth(&self.token)
            .json(body)
            .send()
            .await
            .map_err(|source| SymphonyClientError::Request {
                url: url.clone(),
                source,
            })?;
        decode(response, &url).await
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.server)
    }
}

impl fmt::Debug for SymphonyClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SymphonyClient")
            .field("server", &self.server)
            .field("token", &"<redacted>")
            .finish()
    }
}

async fn decode<T: serde::de::DeserializeOwned>(
    response: reqwest::Response,
    url: &str,
) -> ClientResult<T> {
    let status = response.status();
    if status.is_success() {
        return response
            .json::<T>()
            .await
            .map_err(|source| SymphonyClientError::Decode {
                url: url.to_owned(),
                source,
            });
    }
    let body = response.text().await.unwrap_or_default();
    Err(SymphonyClientError::Http {
        status: status.as_u16(),
        body,
    })
}

/// Reject any non-loopback or non-`http://` server URL. Fail-closed: returns
/// `NotLoopback` rather than ever talking off-loopback.
fn validate_loopback(server: &str) -> ClientResult<()> {
    let parsed = Url::parse(server).map_err(|_| SymphonyClientError::NotLoopback {
        server: server.to_owned(),
    })?;
    if parsed.scheme() != "http" {
        return Err(SymphonyClientError::NotLoopback {
            server: server.to_owned(),
        });
    }
    let is_loopback = match parsed.host() {
        Some(url::Host::Domain(host)) => host.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(address)) => address.is_loopback(),
        Some(url::Host::Ipv6(address)) => address.is_loopback(),
        None => false,
    };
    if !is_loopback {
        return Err(SymphonyClientError::NotLoopback {
            server: server.to_owned(),
        });
    }
    Ok(())
}

/// Build a no-redirect reqwest client for the SSE forwarder. The SSE URL
/// carries the token as `?token=`; a 3xx would forward it off-origin, so the
/// stream connection must never follow redirects (redirect-hop SSRF guard,
/// mirroring `app_state::build_media_fetch_client`).
pub fn build_sse_client() -> reqwest::Result<reqwest::Client> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .pool_idle_timeout(std::time::Duration::from_secs(10))
        .build()
}

/// Parse a complete SSE frame from accumulated text between blank-line
/// separators. Returns `Some` only when a full `event:`+`data:` pair (or a
/// data-only frame) is present; heartbeats (`event:heartbeat`) parse too.
///
/// `buffer` is drained of the consumed frame on success. Comments (`:`) and
/// retry/keep-alive lines are ignored.
pub fn parse_sse_frame(buffer: &mut String) -> Option<SymphonyEventFrame> {
    loop {
        // A frame is delimited from the next by a blank line. The daemon emits
        // `\r\n` (axum/SSE) — tolerate `\n` too.
        let sep = buffer.find("\n\n").or_else(|| buffer.find("\r\n\r\n"))?;
        let is_crlf = buffer[sep..].starts_with("\r\n\r\n");
        let frame_end = if is_crlf { sep + 4 } else { sep + 2 };
        let frame: String = buffer.drain(..frame_end).collect();
        let mut event: Option<String> = None;
        let mut data_lines: Vec<&str> = Vec::new();
        for line in frame.lines() {
            let line = line.strip_suffix('\r').unwrap_or(line);
            if line.is_empty() || line.starts_with(':') {
                continue;
            }
            let (field, value) = line.split_once(':').unwrap_or((line, ""));
            // Per the SSE spec a single leading space after `:` is trimmed.
            let value = value.strip_prefix(' ').unwrap_or(value);
            match field {
                "event" => event = Some(value.to_owned()),
                "data" => data_lines.push(value),
                _ => {} // id/retry/keep-alive ignored
            }
        }
        if data_lines.is_empty() && event.is_none() {
            // Empty frame (e.g. keep-alive padding) — keep scanning.
            continue;
        }
        return Some(SymphonyEventFrame {
            event: event.unwrap_or_else(|| "message".to_owned()),
            data: data_lines.join("\n"),
        });
    }
}

#[cfg(test)]
#[path = "symphony_client_tests.rs"]
mod tests;
