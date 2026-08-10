//! Authenticated loopback `x0xd` REST + WebSocket transport (M3 foundation).
//!
//! A reusable client owned by [`crate::app_state::AppState`] that talks to the
//! embedded `x0xd` daemon over loopback HTTP/WS. It reuses the local-stack
//! supervisor's **transient token/port lifecycle** verbatim — no second
//! convention: each call reads `<data_dir>/api.port` (loopback-validated) and
//! `<data_dir>/api-token`, mints a bearer `Authorization` header, and drops the
//! token at the end of the call. The token is NEVER stored in `AppState`,
//! NEVER logged, and NEVER appears in any error or `Debug` output.
//!
//! # Surfaces
//! - REST: [`history_list`](Self::history_list),
//!   [`history_search`](Self::history_search), [`publish`](Self::publish).
//! - WS: [`run_subscribe`](Self::run_subscribe) — backfill-then-live streaming
//!   over the daemon `/ws` surface (ADR-0023 §7 + the M3 `scope` extension).
//! - Generic authenticated transport: [`get_json`](Self::get_json),
//!   [`post_json`](Self::post_json), [`delete`](Self::delete) for sibling
//!   modules (`pub(crate)`) so identity/contacts/presence/company-template
//!   wiring does not reinvent token resolution.
//!
//! # Auth
//! The durable API token is sent only as an `Authorization: Bearer <token>`
//! header (accepted on every route including `/ws` per `auth::authorize`). It
//! is never placed in a URL query string.

use futures_util::{SinkExt, StreamExt};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::{fmt, time::Duration};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::{
    self, client::IntoClientRequest, protocol::Message as WsMessage,
};

use crate::local_stack::{loopback_api_base, named_data_dir, read_api_port, read_api_token};

/// Per-request deadline for REST calls. The daemon's history store runs on a
/// blocking SQLite thread; 15s is generous even for a full-group backfill.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
/// Cap on the WS connect (upgrade) handshake before declaring the daemon
/// unreachable. Distinct from the read loop, which has no per-frame deadline.
const WS_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

// ── Errors ─────────────────────────────────────────────────────────────────

/// Structured transport error. Carries ONLY safe context — a stage label and a
/// sanitized message. It NEVER contains the bearer token (the token lives only
/// in the request header and is dropped before any error is constructed) and
/// never carries credentials in a URL (all URLs are bare loopback
/// `http(s)://127.0.0.1:<port>`).
#[derive(Debug)]
pub enum X0xClientError {
    /// Daemon artifacts missing: data dir, `api.port`, or `api-token`. The
    /// daemon is not up (or not yet ready). Carries a short stage label only.
    DaemonUnavailable(&'static str),
    /// HTTP/WS transport failure (connect, timeout, send, read). Sanitized.
    Transport(String),
    /// Daemon returned a non-2xx status. `(status_code, body_excerpt)`.
    Status(u16, String),
    /// Response body was not the expected JSON shape.
    Decode(String),
}

impl fmt::Display for X0xClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            X0xClientError::DaemonUnavailable(stage) => {
                write!(f, "x0xd unavailable ({stage})")
            }
            X0xClientError::Transport(msg) => write!(f, "x0xd transport error: {msg}"),
            X0xClientError::Status(code, msg) => {
                write!(f, "x0xd returned HTTP {code}: {msg}")
            }
            X0xClientError::Decode(msg) => write!(f, "x0xd response decode error: {msg}"),
        }
    }
}

impl std::error::Error for X0xClientError {}

impl From<X0xClientError> for String {
    /// Tauri commands return `Result<_, String>`; this lets `?` flow straight
    /// through without an explicit `.map_err(|e| e.to_string())` at every site.
    fn from(e: X0xClientError) -> String {
        e.to_string()
    }
}

// ── Request / response DTOs ────────────────────────────────────────────────

/// `GET /history` / `GET /history/search` request (canonical scope +
/// keyset-pagination cursor). `scope` is a canonical string like
/// `group:<stable_id>`, `dm:<agent_hex>`, or `topic:<name>`.
#[derive(Debug, Clone, Deserialize)]
pub struct HistoryListRequest {
    pub scope: String,
    #[serde(default)]
    pub since_ms: Option<i64>,
    #[serde(default)]
    pub until_ms: Option<i64>,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub before_id: Option<i64>,
}

/// `GET /history/search` request — adds the FTS needle `q`.
#[derive(Debug, Clone, Deserialize)]
pub struct HistorySearchRequest {
    pub scope: String,
    pub q: String,
    #[serde(default)]
    pub since_ms: Option<i64>,
    #[serde(default)]
    pub until_ms: Option<i64>,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub before_id: Option<i64>,
}

/// One durable-history row, as serialized by the daemon's `row_json`:
/// `id`, `msg_id` (lowercase hex), canonical `scope`, author fields,
/// timestamps, `direction`, `content_type`, base64 `payload`, `signed` flag,
/// `provenance`, optional `replace_key`, and the canonical thread fields.
///
/// # Thread ancestry (ADR-0023 thread contract)
/// `thread_root` / `thread_parent` are BLAKE3 ids rendered as lowercase hex
/// strings (matching `msg_id`), or JSON `null`:
/// - **Root message**: `thread_root == msg_id` (self-referential — NOT null),
///   `thread_parent == null`.
/// - **Reply**: `thread_root` = the root's id, `thread_parent` = the parent's
///   id.
/// - **Legacy / unknown**: both `null`.
///
/// Self-referential roots let a consumer group rows into threads by pure
/// metadata (`GROUP BY thread_root`; the row where `thread_root == msg_id` is
/// the root) WITHOUT reconstructing ancestry from payloads or ordering. A null
/// `thread_root` unambiguously means "no threading metadata".
///
/// Both fields are `#[serde(default)]` so the client parses responses from
/// daemons that predate the thread contract (they simply come back `null`).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct HistoryRow {
    pub id: i64,
    /// BLAKE3 message id, lowercase hex.
    pub msg_id: String,
    pub scope: String,
    pub author_agent: Option<String>,
    pub author_machine: Option<String>,
    pub sent_at_ms: i64,
    pub seen_at_ms: i64,
    pub direction: String,
    pub content_type: String,
    /// Base64-encoded payload bytes.
    pub payload: String,
    pub signed: bool,
    pub provenance: String,
    #[serde(default)]
    pub replace_key: Option<String>,
    /// Canonical thread root (lowercase hex) — self-referential on the root.
    /// See [struct docs](HistoryRow#thread-ancestry).
    #[serde(default)]
    pub thread_root: Option<String>,
    /// Canonical thread parent (lowercase hex); `null` on the root + legacy.
    #[serde(default)]
    pub thread_parent: Option<String>,
}

/// Raw envelope returned by `GET /history` and `GET /history/search`. `records`
/// is newest-first; `next_before_id` (list only) is the rowid cursor for the
/// next older page.
#[derive(Debug, Deserialize)]
struct HistoryResponse {
    #[serde(default)]
    records: Vec<HistoryRow>,
    #[serde(default)]
    next_before_id: Option<i64>,
}

/// Envelope returned by `GET /history/message/:msg_id`. `record` is `None`
/// only via the client's 404→`None` mapping (the daemon's 200 always carries
/// one); `#[serde(default)]` keeps a malformed body from panic-decoding.
#[derive(Debug, Deserialize)]
struct HistoryMessageResponse {
    #[serde(default)]
    record: Option<HistoryRow>,
}

/// A page of history rows with a computed `has_more` flag and the keyset
/// cursor for the next (older) page. `has_more` is `true` when the server
/// returned a full `limit`-sized page (the standard keyset heuristic); the
/// cursor lets the caller request the next page via `before_id`.
#[derive(Debug, Clone, Serialize)]
pub struct HistoryPage {
    pub rows: Vec<HistoryRow>,
    pub has_more: bool,
    pub next_before_id: Option<i64>,
}

/// `POST /publish` body: `{ topic, payload(base64) }`.
#[derive(Serialize)]
struct PublishBody<'a> {
    topic: &'a str,
    payload: &'a str,
}

/// `POST /groups/:id/send` body: `{ body, kind, thread_root?, thread_parent? }`.
/// The daemon authority-signs the message (SignedPublic only) and records it
/// under `Scope::Group`; MlsEncrypted groups are rejected with 400. Optional
/// `thread_root`/`thread_parent` (ADR-0029) carry 64-hex canonical msg_ids;
/// both are omitted from the wire when `None` (non-threaded v1 messages).
#[derive(Serialize)]
struct SendGroupBody<'a> {
    body: &'a str,
    kind: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    thread_root: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thread_parent: Option<&'a str>,
}

/// `POST /direct/send` body. The daemon validates `agent_id` as a 64-hex
/// AgentId and `payload` as base64; optional `thread_root`/`thread_parent` are
/// 64-hex canonical msg_ids (validated to 32 bytes via `ThreadMeta::from_hex`).
/// All fields are snake_case on the wire (the daemon deserializes verbatim).
#[derive(Serialize)]
struct SendDirectBody<'a> {
    agent_id: &'a str,
    payload: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    thread_root: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thread_parent: Option<&'a str>,
}

/// `POST /direct/send` receipt. The daemon returns the chosen delivery `path`
/// (`loopback` | `gossip_inbox` | `raw_quic` | `raw_quic_acked` | `relayed`),
/// `retries_used`, the hex `request_id`, and an optional `require_ack` probe
/// result. The canonical durable `msg_id` is NOT returned here — it is
/// `compute_local_send_msg_id(request_id, payload)` (BLAKE3), surfaced later via
/// `/history`/`/ws/direct` backfill. `#[serde(default)]` so a minimal `{ ok }`
/// body still parses.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirectSendReceipt {
    #[serde(default)]
    pub ok: bool,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub retries_used: Option<u64>,
    #[serde(default)]
    pub request_id: Option<String>,
}
// ── Group transport resolution (live topic + send route) ────────────────────

/// `policy.confidentiality` projection from `GET /groups/:id`. The daemon
/// serializes the enum snake_case (`"signed_public"` | `"mls_encrypted"`).
#[derive(Debug, Default, Deserialize)]
struct GroupPolicyWire {
    #[serde(default)]
    confidentiality: Option<String>,
}

/// `GET /groups/:id` projection — only the transport-relevant fields.
#[derive(Debug, Deserialize)]
struct GroupDetailWire {
    #[serde(default)]
    chat_topic: String,
    #[serde(default)]
    policy: GroupPolicyWire,
}

/// `GET /groups/:id/state` projection — `group_id` is the **stable** id
/// (Phase D.3), which can differ from the mls id used for REST routing and is
/// what the public-message gossip topic is keyed by.
#[derive(Debug, Deserialize)]
struct GroupStateWire {
    #[serde(default)]
    group_id: String,
}

// ── WebSocket subscribe DTOs ───────────────────────────────────────────────

/// Optional backfill spec carried on a `Subscribe` frame (ADR-0023 §7). The
/// daemon's `WsBackfill` honours **only** `limit`: it replays up to `limit`
/// stored rows for each subscribed topic's `topic:<name>` scope oldest-first,
/// emits a `live` marker, then forwards live `message` frames.
///
/// `scope` / `before_id` / `since_ms` are NOT honoured on the WS path — group
/// durable history lives under `Scope::Group` and DM history under `Scope::Dm`,
/// neither of which the topic-keyed WS backfill reads. Cold-load those scopes
/// via REST [`X0xClient::history_list`](Self::history_list) and open the WS
/// live-only (`backfill: None`). DM cold-load+live uses `/ws/direct` (see
/// [`X0xClient::run_subscribe_direct`](Self::run_subscribe_direct)).
#[derive(Debug, Clone, Deserialize)]
pub struct X0xBackfillRequest {
    /// Max stored rows to replay per subscribed topic (server clamps).
    pub limit: usize,
}

/// `WsInbound::Subscribe` — the client→server frame.
#[derive(Serialize)]
struct WsSubscribe {
    #[serde(rename = "type")]
    kind: &'static str,
    topics: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    backfill: Option<WsBackfillSpec>,
}

#[derive(Serialize)]
struct WsBackfillSpec {
    limit: usize,
}

impl From<X0xBackfillRequest> for WsBackfillSpec {
    fn from(r: X0xBackfillRequest) -> Self {
        Self { limit: r.limit }
    }
}

/// A single frame on a daemon WebSocket stream, mirroring `WsOutbound`
/// (`#[serde(tag = "type")]`). Emitted to the frontend over a Tauri `Channel`.
///
/// The `message` variant (topic gossip) and `direct_message` variant (DM,
/// `/ws/direct`) both carry optional `thread_root` / `thread_parent` and an
/// optional `msg_id`. `msg_id` is populated once the daemon ships its
/// always-present `msg_id` addition; it is `None` on older daemons. All
/// optional fields are `#[serde(default)]`-nullable so the union is stable
/// regardless of which daemon version or path produced the frame.
#[derive(Debug, Clone, Serialize, Deserialize)]
// NOTE: an internally-tagged enum's container `rename_all` only renames the
// variant discriminator (the `type` tag), NOT struct-variant fields. Each
// multi-word field therefore carries an explicit asymmetry: serialize as
// camelCase (the Tauri-Channel / TS contract) and deserialize the daemon's
// snake_case wire. Single-word fields (topic/payload/origin/sender/verified)
// match in both directions and need no rename.
#[serde(tag = "type")]
pub enum X0xFrame {
    #[serde(rename = "connected")]
    Connected {
        #[serde(rename(serialize = "sessionId", deserialize = "session_id"))]
        session_id: String,
        #[serde(rename(serialize = "agentId", deserialize = "agent_id"))]
        agent_id: String,
    },
    #[serde(rename = "message")]
    Message {
        topic: String,
        payload: String,
        #[serde(default)]
        origin: Option<String>,
        /// Canonical message id (64-hex). Populated once the daemon ships the
        /// `msg_id` addition on `WsOutbound::Message`; absent on older daemons.
        #[serde(default, rename(serialize = "msgId", deserialize = "msg_id"))]
        msg_id: Option<String>,
        #[serde(default, rename(serialize = "threadRoot", deserialize = "thread_root"))]
        thread_root: Option<String>,
        #[serde(
            default,
            rename(serialize = "threadParent", deserialize = "thread_parent")
        )]
        thread_parent: Option<String>,
    },
    /// A direct-message frame from `/ws/direct`. Mirrors the daemon's
    /// `WsOutbound::DirectMessage`. `payload` is base64 application bytes;
    /// `sender`/`machine_id` are 64-hex ids; `received_at` is unix-ms.
    #[serde(rename = "direct_message")]
    DirectMessage {
        #[serde(default, rename(serialize = "msgId", deserialize = "msg_id"))]
        msg_id: Option<String>,
        sender: String,
        #[serde(rename(serialize = "machineId", deserialize = "machine_id"))]
        machine_id: String,
        payload: String,
        #[serde(rename(serialize = "receivedAt", deserialize = "received_at"))]
        received_at: u64,
        verified: bool,
        #[serde(
            default,
            rename(serialize = "trustDecision", deserialize = "trust_decision")
        )]
        trust_decision: Option<String>,
        #[serde(default, rename(serialize = "threadRoot", deserialize = "thread_root"))]
        thread_root: Option<String>,
        #[serde(
            default,
            rename(serialize = "threadParent", deserialize = "thread_parent")
        )]
        thread_parent: Option<String>,
    },
    #[serde(rename = "subscribed")]
    Subscribed { topics: Vec<String> },
    #[serde(rename = "unsubscribed")]
    Unsubscribed { topics: Vec<String> },
    /// Backfill-then-live marker: everything before this came from the
    /// durable store; everything after is live. On `/ws/direct` the daemon
    /// emits `{ type: "live", topic: "direct" }`.
    #[serde(rename = "live")]
    Live { topic: String },
    #[serde(rename = "error")]
    Error { message: String },
}

/// Confidentiality of a named group, as reported by the daemon's
/// `policy.confidentiality`. Determines the live topic and the available send
/// route.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupConfidentiality {
    /// Signed-but-readable plaintext; chat flows on
    /// `x0x.groups.public.{stable_group_id}` and sends go through
    /// `POST /groups/:id/send` (authority-signed, durable).
    SignedPublic,
    /// MLS end-to-end encryption; chat flows on the daemon-reported
    /// `chat_topic`. The desktop send boundary REJECTS MlsEncrypted groups
    /// (secure-group crypto is not approved), so no `/groups/:id/secure/send`
    /// route is ever reached from Tauri; this variant only drives live-topic
    /// resolution for already-joined MLS groups.
    MlsEncrypted,
}

/// Resolved transport plan for a named group's live + send surfaces.
#[derive(Debug, Clone)]
pub struct GroupTransport {
    /// The gossip topic live chat frames arrive on.
    pub live_topic: String,
    /// The group's confidentiality (drives the send route).
    pub confidentiality: GroupConfidentiality,
    /// The stable group id (Phase D.3). Equal to the REST-routing id for
    /// pre-D.3 groups; may differ once a state-commit chain is established.
    /// This is the scope durable group history is recorded under
    /// (`Scope::Group(stable_id)`).
    pub stable_group_id: String,
}

/// The gossip topic SignedPublic group chat flows on, keyed by the **stable**
/// group id. Mirrors the daemon's `groups::public_topic_for(stable_id)`
/// (`PUBLIC_GROUP_TOPIC_PREFIX.{group_id}`). Centralized so the client never
/// hand-synthesizes a divergent topic string.
fn group_public_topic(stable_group_id: &str) -> String {
    format!("x0x.groups.public.{stable_group_id}")
}

// ── Client ─────────────────────────────────────────────────────────────────

/// Authenticated loopback `x0xd` client. Stateless beyond the shared HTTP
/// client (cheaply [`Clone`] — `reqwest::Client` is internally an `Arc`): the
/// bearer token + base URL are resolved transiently on each call from the
/// local-stack named data dir.
#[derive(Clone)]
pub struct X0xClient {
    http: reqwest::Client,
}

/// Resolved loopback endpoints for one call. `token` is dropped at the end of
/// the resolving call; it never escapes the client.
struct Resolved {
    api_base: String,
    ws_base: String,
    token: String,
}

/// The concrete WS stream produced by `connect_async` over loopback. Aliased
/// so the connect helper and read loop share one signature.
type WsStream =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

impl X0xClient {
    /// Build from the app-wide HTTP client (the same localhost-resolved,
    /// pooled client built in [`crate::app_state::try_build_app_state`]).
    pub fn new(http: reqwest::Client) -> Self {
        Self { http }
    }

    /// Read the named data dir's `api.port` + `api-token` and derive the
    /// loopback REST/WS bases. Fail-closed: any missing/non-loopback artifact
    /// is an error (the daemon is unavailable), never a silent fallback.
    fn resolve(&self) -> Result<Resolved, X0xClientError> {
        let data_dir =
            named_data_dir().ok_or(X0xClientError::DaemonUnavailable("data dir unavailable"))?;
        let port = read_api_port(&data_dir).ok_or(X0xClientError::DaemonUnavailable(
            "api.port missing or non-loopback",
        ))?;
        let token = read_api_token(&data_dir)
            .ok_or(X0xClientError::DaemonUnavailable("api-token missing"))?;
        Ok(Resolved {
            api_base: loopback_api_base(port),
            ws_base: format!("ws://127.0.0.1:{port}"),
            token,
        })
    }

    /// Authenticated `GET` that deserializes the JSON body into `T`. `query`
    /// is a list of `(key, value)` pairs serialized as query params.
    pub(crate) async fn get_json<T: DeserializeOwned>(
        &self,
        path: &str,
        query: &[(String, String)],
    ) -> Result<T, X0xClientError> {
        let r = self.resolve()?;
        let url = format!("{}{}", r.api_base, path);
        let resp = self
            .http
            .get(&url)
            .bearer_auth(&r.token)
            .query(query)
            .timeout(REQUEST_TIMEOUT)
            .send()
            .await
            .map_err(|e| X0xClientError::Transport(format!("GET {path}: {e}")))?;
        Self::decode_json::<T>(resp, path).await
    }

    /// Authenticated `POST` with a JSON body, deserializing the response into
    /// `T`.
    pub(crate) async fn post_json<B: Serialize, T: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T, X0xClientError> {
        self.post_json_with_timeout(path, body, REQUEST_TIMEOUT)
            .await
    }

    /// Authenticated `POST` with an explicit bounded deadline. This exists for
    /// daemon operations whose own documented bound exceeds the ordinary REST
    /// request deadline (currently exact-AgentId connect only).
    pub(crate) async fn post_json_with_timeout<B: Serialize, T: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
        timeout: Duration,
    ) -> Result<T, X0xClientError> {
        let r = self.resolve()?;
        let url = format!("{}{}", r.api_base, path);
        let resp = self
            .http
            .post(&url)
            .bearer_auth(&r.token)
            .timeout(timeout)
            .json(body)
            .send()
            .await
            .map_err(|e| X0xClientError::Transport(format!("POST {path}: {e}")))?;
        Self::decode_json::<T>(resp, path).await
    }

    /// Authenticated `PUT` with a JSON body, deserializing the response into
    /// `T`. Used by native KV-store writes.
    pub(crate) async fn put_json<B: Serialize, T: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T, X0xClientError> {
        let r = self.resolve()?;
        let url = format!("{}{}", r.api_base, path);
        let resp = self
            .http
            .put(&url)
            .bearer_auth(&r.token)
            .timeout(REQUEST_TIMEOUT)
            .json(body)
            .send()
            .await
            .map_err(|e| X0xClientError::Transport(format!("PUT {path}: {e}")))?;
        Self::decode_json::<T>(resp, path).await
    }

    /// Authenticated `PATCH` with a JSON body, deserializing the response into
    /// `T`. Used by named-group membership and collaborative task mutations.
    pub(crate) async fn patch_json<B: Serialize, T: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T, X0xClientError> {
        let r = self.resolve()?;
        let url = format!("{}{}", r.api_base, path);
        let resp = self
            .http
            .request(reqwest::Method::PATCH, &url)
            .bearer_auth(&r.token)
            .timeout(REQUEST_TIMEOUT)
            .json(body)
            .send()
            .await
            .map_err(|e| X0xClientError::Transport(format!("PATCH {path}: {e}")))?;
        Self::decode_json::<T>(resp, path).await
    }

    /// Authenticated `DELETE`, deserializing the response body into `T`.
    pub(crate) async fn delete_json<T: DeserializeOwned>(
        &self,
        path: &str,
    ) -> Result<T, X0xClientError> {
        let r = self.resolve()?;
        let url = format!("{}{}", r.api_base, path);
        let resp = self
            .http
            .delete(&url)
            .bearer_auth(&r.token)
            .timeout(REQUEST_TIMEOUT)
            .send()
            .await
            .map_err(|e| X0xClientError::Transport(format!("DELETE {path}: {e}")))?;
        Self::decode_json::<T>(resp, path).await
    }

    /// Authenticated `DELETE` that ignores a successful response body.
    #[allow(dead_code)] // pub(crate) transport exposed for sibling command modules (group/identity leaves)
    pub(crate) async fn delete(&self, path: &str) -> Result<(), X0xClientError> {
        let r = self.resolve()?;
        let url = format!("{}{}", r.api_base, path);
        let resp = self
            .http
            .delete(&url)
            .bearer_auth(&r.token)
            .timeout(REQUEST_TIMEOUT)
            .send()
            .await
            .map_err(|e| X0xClientError::Transport(format!("DELETE {path}: {e}")))?;
        if !resp.status().is_success() {
            let code = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            return Err(X0xClientError::Status(
                code,
                body.chars().take(300).collect(),
            ));
        }
        Ok(())
    }

    /// Shared success/body decode: non-2xx → `Status` with a 300-char excerpt;
    /// 2xx → deserialize into `T`.
    async fn decode_json<T: DeserializeOwned>(
        resp: reqwest::Response,
        path: &str,
    ) -> Result<T, X0xClientError> {
        let status = resp.status();
        if !status.is_success() {
            let code = status.as_u16();
            let body = resp.text().await.unwrap_or_default();
            return Err(X0xClientError::Status(
                code,
                format!("{path}: {}", body.chars().take(300).collect::<String>()),
            ));
        }
        resp.json::<T>()
            .await
            .map_err(|e| X0xClientError::Decode(format!("{path}: {e}")))
    }

    // ── M3 surfaces ────────────────────────────────────────────────────────

    /// `GET /history` — scoped durable-history listing, newest-first.
    pub async fn history_list(
        &self,
        req: &HistoryListRequest,
    ) -> Result<HistoryPage, X0xClientError> {
        let limit = req.limit.unwrap_or(50);
        let mut query = vec![
            ("scope".to_string(), req.scope.clone()),
            ("limit".to_string(), limit.to_string()),
        ];
        if let Some(v) = req.since_ms {
            query.push(("since_ms".to_string(), v.to_string()));
        }
        if let Some(v) = req.until_ms {
            query.push(("until_ms".to_string(), v.to_string()));
        }
        if let Some(v) = req.before_id {
            query.push(("before_id".to_string(), v.to_string()));
        }
        let resp: HistoryResponse = self.get_json("/history", &query).await?;
        let has_more = resp.records.len() >= limit && resp.next_before_id.is_some();
        Ok(HistoryPage {
            rows: resp.records,
            has_more,
            next_before_id: resp.next_before_id,
        })
    }

    /// `GET /history/search` — FTS5 search over text payloads within a scope.
    pub async fn history_search(
        &self,
        req: &HistorySearchRequest,
    ) -> Result<HistoryPage, X0xClientError> {
        let limit = req.limit.unwrap_or(20);
        let mut query = vec![
            ("scope".to_string(), req.scope.clone()),
            ("q".to_string(), req.q.clone()),
            ("limit".to_string(), limit.to_string()),
        ];
        if let Some(v) = req.since_ms {
            query.push(("since_ms".to_string(), v.to_string()));
        }
        if let Some(v) = req.until_ms {
            query.push(("until_ms".to_string(), v.to_string()));
        }
        if let Some(v) = req.before_id {
            query.push(("before_id".to_string(), v.to_string()));
        }
        let resp: HistoryResponse = self.get_json("/history/search", &query).await?;
        let has_more = resp.records.len() >= limit;
        Ok(HistoryPage {
            rows: resp.records,
            has_more,
            next_before_id: resp.next_before_id,
        })
    }

    /// `GET /history/message/:msg_id` — single durable-history row by canonical
    /// BLAKE3 `msg_id` (lowercase 64-hex). Index-backed point lookup on the
    /// daemon; no scope hint is needed (`msg_id` is globally unique in one
    /// store) and no network/cross-user lookup occurs.
    ///
    /// Returns `Ok(None)` on `404 NOT_FOUND` (well-formed id, no matching row) —
    /// **distinct** from a transport or decode error, which propagates as
    /// `Err`. A malformed id surfaces as the daemon's `400` (`Err::Status`).
    pub async fn history_get(
        &self,
        msg_id_hex: &str,
    ) -> Result<Option<HistoryRow>, X0xClientError> {
        let path = format!("/history/message/{msg_id_hex}");
        match self.get_json::<HistoryMessageResponse>(&path, &[]).await {
            Ok(resp) => Ok(resp.record),
            // Not-found is a normal outcome, not a transport fault.
            Err(X0xClientError::Status(404, _)) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// `POST /publish` — publish a base64 payload to a true gossip topic.
    ///
    /// This is the **topic-scope** send surface only. It gossips and does NOT
    /// record durable history, so it MUST NOT be used for group scopes — group
    /// sends go through [`Self::send_group_message`] (SignedPublic, durable) and
    /// are unsupported for MlsEncrypted (no plaintext-send route). See
    /// [`Self::resolve_group_transport`] for the confidentiality-driven routing.
    pub async fn publish(&self, topic: &str, payload_b64: &str) -> Result<(), X0xClientError> {
        let body = PublishBody {
            topic,
            payload: payload_b64,
        };
        let _: serde_json::Value = self.post_json("/publish", &body).await?;
        Ok(())
    }

    /// Resolve the live transport topic + send route for a named group.
    ///
    /// Mirrors the reference GUI client:
    /// - **SignedPublic** → live chat flows on `x0x.groups.public.{stable_id}`;
    ///   the stable id is fetched via `GET /groups/:id/state` (it can differ
    ///   from the mls id used for REST routing).
    /// - **MlsEncrypted** → live chat flows on the daemon-reported `chat_topic`
    ///   (`x0x.group.{prefix}.chat/general`).
    ///
    /// Both are authoritative daemon values — the client never synthesizes a
    /// topic string. An unknown/missing confidentiality is never defaulted to
    /// MLS: it fails closed here so an unrecognised group can never reach the
    /// secure-send route.
    pub async fn resolve_group_transport(
        &self,
        group_id: &str,
    ) -> Result<GroupTransport, X0xClientError> {
        let path = format!("/groups/{group_id}");
        let detail: GroupDetailWire = self.get_json(&path, &[]).await?;
        let confidentiality = match detail.policy.confidentiality.as_deref() {
            Some("signed_public") => GroupConfidentiality::SignedPublic,
            Some("mls_encrypted") => GroupConfidentiality::MlsEncrypted,
            // Never default an unknown/missing confidentiality to MLS — that
            // would launder an unrecognised group onto the secure-send route.
            // Fail closed here so the send boundary only ever observes
            // explicitly-tagged groups (and the secure send itself rejects
            // MlsEncrypted).
            other => {
                return Err(X0xClientError::Transport(format!(
                    "group {group_id} reports unsupported confidentiality ({other:?}); refusing transport (only signed_public groups are sendable)"
                )));
            }
        };
        match confidentiality {
            GroupConfidentiality::SignedPublic => {
                let state: GroupStateWire = self
                    .get_json(&format!("/groups/{group_id}/state"), &[])
                    .await?;
                let stable = state.group_id;
                Ok(GroupTransport {
                    live_topic: group_public_topic(&stable),
                    confidentiality,
                    stable_group_id: stable,
                })
            }
            GroupConfidentiality::MlsEncrypted => Ok(GroupTransport {
                // chat_topic already carries the `/general` suffix
                // (general_chat_topic); no synthesis here.
                live_topic: detail.chat_topic,
                confidentiality,
                stable_group_id: group_id.to_string(),
            }),
        }
    }

    /// `POST /groups/:id/send` — durable SignedPublic group send.
    ///
    /// The daemon authority-signs the message, publishes the signed envelope
    /// on `x0x.groups.public.{stable_id}`, and records it under
    /// `Scope::Group`. It returns 400 for MlsEncrypted groups — callers MUST
    /// branch on [`GroupTransport::confidentiality`] first and never fall back
    /// to [`Self::publish`] for a group scope.
    ///
    /// Optional `thread_root`/`thread_parent` (ADR-0029) carry 64-hex canonical
    /// msg_ids for threaded replies; both `None` produces a v1 non-threaded
    /// message. The response carries the daemon-computed `msg_id`
    /// (`BLAKE3(signable_bytes)`) which callers use as the canonical identity
    /// for optimistic reconciliation and thread ancestry.
    pub async fn send_group_message(
        &self,
        group_id: &str,
        body: &str,
        kind: &str,
        thread_root: Option<&str>,
        thread_parent: Option<&str>,
    ) -> Result<Option<String>, X0xClientError> {
        let req = SendGroupBody {
            body,
            kind,
            thread_root,
            thread_parent,
        };
        let resp: serde_json::Value = self
            .post_json(&format!("/groups/{group_id}/send"), &req)
            .await?;
        Ok(resp
            .get("msg_id")
            .and_then(|v| v.as_str())
            .map(str::to_string))
    }

    /// `POST /direct/send` — native one-to-one direct message send.
    ///
    /// Sends base64 application `payload` to the 64-hex recipient `agent_id`
    /// over the daemon's authenticated DM path (raw-QUIC preferred when a live
    /// connection exists, gossip-inbox fallback otherwise). Optional
    /// `thread_root`/`thread_parent` are 64-hex canonical msg_ids, validated
    /// daemon-side via `ThreadMeta::from_hex` (32 bytes each).
    ///
    /// The daemon records the outbound row under `Scope::Dm(<recipient_hex>)`
    /// with `msg_id = compute_local_send_msg_id(request_id, payload)`; that
    /// canonical id is reconciled with the optimistic (clientId-keyed) row via
    /// the shared `localKey` when history/live rehydrates — the receipt itself
    /// carries only the `request_id`, never the canonical msg_id.
    pub async fn send_direct_message(
        &self,
        agent_id: &str,
        payload_b64: &str,
        thread_root: Option<&str>,
        thread_parent: Option<&str>,
    ) -> Result<DirectSendReceipt, X0xClientError> {
        let body = SendDirectBody {
            agent_id,
            payload: payload_b64,
            thread_root,
            thread_parent,
        };
        self.post_json("/direct/send", &body).await
    }

    /// Open a backfill-then-live stream over the daemon `/ws` surface and
    /// forward every frame to `tx`. Connects with a bearer `Authorization`
    /// header, sends `Subscribe { topics, backfill }`, then reads until the
    /// daemon closes the socket, an error frame arrives, or `tx` is dropped
    /// (the frontend tore down its `Channel`).
    ///
    /// This is a STREAM, not a poll loop: it holds one WS connection open for
    /// the life of the subscription. Thread ancestry arrives from the server
    /// (`thread_root`/`thread_parent` on backfill frames) and is never
    /// reconstructed here. Backfill replays per-topic `Scope::Topic` rows
    /// only — group/dm durable history is NOT readable on this path; cold-load
    /// those via [`Self::history_list`] (groups) or [`Self::run_subscribe_direct`].
    pub async fn run_subscribe(
        &self,
        topics: Vec<String>,
        backfill: Option<X0xBackfillRequest>,
        tx: mpsc::Sender<X0xFrame>,
    ) -> Result<(), X0xClientError> {
        let mut stream = self
            .ws_connect(&format!("{}/ws", self.resolve()?.ws_base))
            .await?;

        // Send the Subscribe frame.
        let sub = WsSubscribe {
            kind: "subscribe",
            topics,
            backfill: backfill.map(Into::into),
        };
        let sub_json = serde_json::to_string(&sub)
            .map_err(|e| X0xClientError::Transport(format!("subscribe encode: {e}")))?;
        stream
            .send(WsMessage::Text(sub_json.into()))
            .await
            .map_err(|e| X0xClientError::Transport(format!("subscribe send: {e}")))?;

        Self::ws_read_loop(&mut stream, &tx).await;
        // Best-effort graceful close; ignore failures (we're tearing down).
        let _ = stream.send(WsMessage::Close(None)).await;
        Ok(())
    }

    /// Open a live direct-message stream over `/ws/direct` and forward every
    /// frame to `tx`. The daemon auto-subscribes the session to direct
    /// delivery — **no `Subscribe` frame is sent** — and replays up to
    /// `backfill` stored DM rows (all `dm:` scopes, oldest→newest) before a
    /// `live` marker, then streams live `direct_message` frames.
    ///
    /// Frames are mapped to [`X0xFrame`] (`direct_message`, `connected`,
    /// `live`, `error`). This is the DM-scope live path: `dm:<peer>` triggers
    /// it; the peer filter is applied by the consumer (the daemon delivers all
    /// DMs to the session).
    pub async fn run_subscribe_direct(
        &self,
        backfill: Option<usize>,
        tx: mpsc::Sender<X0xFrame>,
    ) -> Result<(), X0xClientError> {
        let ws_base = self.resolve()?.ws_base;
        let url = match backfill {
            Some(n) => format!("{ws_base}/ws/direct?backfill={n}"),
            None => format!("{ws_base}/ws/direct"),
        };
        let mut stream = self.ws_connect(&url).await?;
        Self::ws_read_loop(&mut stream, &tx).await;
        let _ = stream.send(WsMessage::Close(None)).await;
        Ok(())
    }

    /// Build an authenticated WS upgrade request to `url` and complete the
    /// handshake within [`WS_CONNECT_TIMEOUT`]. Returns the live socket.
    async fn ws_connect(&self, url: &str) -> Result<WsStream, X0xClientError> {
        let token = self.resolve()?.token;
        let mut req = url
            .into_client_request()
            .map_err(|e| X0xClientError::Transport(format!("ws request build: {e}")))?;
        {
            // Browsers can't set WS headers, but this is a native tungstenite
            // client — attach the bearer token as an Authorization header.
            let auth = format!("Bearer {token}");
            let value = tungstenite::http::HeaderValue::from_str(&auth)
                .map_err(|e| X0xClientError::Transport(format!("ws auth header: {e}")))?;
            req.headers_mut()
                .insert(tungstenite::http::header::AUTHORIZATION, value);
        }
        let (stream, _resp) =
            tokio::time::timeout(WS_CONNECT_TIMEOUT, tokio_tungstenite::connect_async(req))
                .await
                .map_err(|_| X0xClientError::Transport("ws connect timed out".to_string()))?
                .map_err(|e| X0xClientError::Transport(format!("ws connect: {e}")))?;
        Ok(stream)
    }

    /// Shared read loop for `/ws` and `/ws/direct`: parse each text frame,
    /// forward to the channel, honour close/error/drop. Returns when the
    /// stream ends (the caller owns the graceful close).
    async fn ws_read_loop(stream: &mut WsStream, tx: &mpsc::Sender<X0xFrame>) {
        while let Some(msg) = stream.next().await {
            let msg = match msg {
                Ok(m) => m,
                Err(e) => {
                    // Surface the transport failure as an error frame so the
                    // frontend learns why the stream died.
                    let _ = tx
                        .send(X0xFrame::Error {
                            message: format!("ws read: {e}"),
                        })
                        .await;
                    return;
                }
            };
            match msg {
                WsMessage::Text(text) => {
                    let s: &str = &text;
                    match serde_json::from_str::<X0xFrame>(s) {
                        Ok(frame) => {
                            let is_error = matches!(frame, X0xFrame::Error { .. });
                            if tx.send(frame).await.is_err() {
                                // Receiver gone (frontend dropped the Channel).
                                return;
                            }
                            if is_error {
                                // A daemon-reported error ends the stream.
                                return;
                            }
                        }
                        Err(_) => {
                            // Unrecognized frame shape — skip rather than kill
                            // the whole stream (forward-compat with new frame
                            // variants the client doesn't model yet).
                        }
                    }
                }
                WsMessage::Binary(_) => { /* daemon is text-only; ignore */ }
                WsMessage::Ping(p) => {
                    let _ = stream.send(WsMessage::Pong(p)).await;
                }
                WsMessage::Close(_) => return,
                _ => {}
            }
        }
    }
}

#[cfg(test)]
#[path = "x0x_client_tests.rs"]
mod tests;
