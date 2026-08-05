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

use std::fmt;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
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

// ── WebSocket subscribe DTOs ───────────────────────────────────────────────

/// Optional backfill spec carried on a subscribe request (ADR-0023 §7 + the
/// M3 `scope` extension, frozen by the thread-contract work). When present,
/// the daemon replays up to `limit` stored rows for `scope` (or, if `scope` is
/// absent, each `topics` entry's `topic:<name>` scope) oldest-first, emits a
/// `live` marker, then forwards live `message` frames.
#[derive(Debug, Clone, Deserialize)]
pub struct X0xBackfillRequest {
    /// Max stored rows to replay (server clamps like `/history`).
    pub limit: usize,
    /// Canonical scope to replay (`group:<id>` | `dm:<agent>` | `topic:<name>`).
    /// Absent ⇒ per-topic replay (legacy behaviour).
    #[serde(default)]
    pub scope: Option<String>,
    /// Keyset cursor: replay rows strictly older than this rowid.
    #[serde(default)]
    pub before_id: Option<i64>,
    /// Inclusive lower bound on `seen_at_ms`.
    #[serde(default)]
    pub since_ms: Option<i64>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    scope: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    before_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    since_ms: Option<i64>,
}

impl From<X0xBackfillRequest> for WsBackfillSpec {
    fn from(r: X0xBackfillRequest) -> Self {
        Self {
            limit: r.limit,
            scope: r.scope,
            before_id: r.before_id,
            since_ms: r.since_ms,
        }
    }
}

/// A single frame on the `/ws` stream, mirroring the daemon's `WsOutbound`
/// (`#[serde(tag = "type")]`). Emitted to the frontend over a Tauri `Channel`.
///
/// The `message` variant carries optional `thread_root` / `thread_parent`:
/// backfill frames include them (once the daemon ships the `scope` extension);
/// live frames may omit them. Both are `#[serde(default)]`-nullable so the
/// frame union is stable regardless of which path produced the row.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all(serialize = "camelCase"))]
pub enum X0xFrame {
    #[serde(rename = "connected")]
    Connected {
        session_id: String,
        agent_id: String,
    },
    #[serde(rename = "message")]
    Message {
        topic: String,
        payload: String,
        #[serde(default)]
        origin: Option<String>,
        #[serde(default)]
        thread_root: Option<String>,
        #[serde(default)]
        thread_parent: Option<String>,
    },
    #[serde(rename = "subscribed")]
    Subscribed { topics: Vec<String> },
    #[serde(rename = "unsubscribed")]
    Unsubscribed { topics: Vec<String> },
    /// Backfill-then-live marker: everything before this on the topic came
    /// from the durable store; everything after is live.
    #[serde(rename = "live")]
    Live { topic: String },
    #[serde(rename = "error")]
    Error { message: String },
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
        let r = self.resolve()?;
        let url = format!("{}{}", r.api_base, path);
        let resp = self
            .http
            .post(&url)
            .bearer_auth(&r.token)
            .timeout(REQUEST_TIMEOUT)
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

    /// `POST /publish` — publish a base64 payload to a gossip topic (the
    /// native workspace/group message-send surface for M3).
    pub async fn publish(&self, topic: &str, payload_b64: &str) -> Result<(), X0xClientError> {
        let body = PublishBody {
            topic,
            payload: payload_b64,
        };
        let _: serde_json::Value = self.post_json("/publish", &body).await?;
        Ok(())
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
    /// reconstructed here.
    pub async fn run_subscribe(
        &self,
        topics: Vec<String>,
        backfill: Option<X0xBackfillRequest>,
        tx: mpsc::Sender<X0xFrame>,
    ) -> Result<(), X0xClientError> {
        let r = self.resolve()?;
        let url = format!("{}/ws", r.ws_base);

        // Build the upgrade request and attach the bearer header (browsers
        // can't set WS headers, but this is a native tungstenite client).
        let mut req = url
            .into_client_request()
            .map_err(|e| X0xClientError::Transport(format!("ws request build: {e}")))?;
        {
            let auth = format!("Bearer {}", r.token);
            let value = tungstenite::http::HeaderValue::from_str(&auth)
                .map_err(|e| X0xClientError::Transport(format!("ws auth header: {e}")))?;
            req.headers_mut()
                .insert(tungstenite::http::header::AUTHORIZATION, value);
        }

        let (mut stream, _resp) =
            tokio::time::timeout(WS_CONNECT_TIMEOUT, tokio_tungstenite::connect_async(req))
                .await
                .map_err(|_| X0xClientError::Transport("ws connect timed out".to_string()))?
                .map_err(|e| X0xClientError::Transport(format!("ws connect: {e}")))?;

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

        // Read loop: parse each text frame, forward to the channel, honour
        // close/error/drop.
        while let Some(msg) = stream.next().await {
            let msg = match msg {
                Ok(m) => m,
                Err(e) => {
                    return Err(X0xClientError::Transport(format!("ws read: {e}")));
                }
            };
            match msg {
                WsMessage::Text(text) => {
                    let s: &str = &text;
                    match serde_json::from_str::<X0xFrame>(s) {
                        Ok(frame) => {
                            let is_error = matches!(frame, X0xFrame::Error { .. });
                            if tx.send(frame).await.is_err() {
                                // Receiver gone (frontend dropped the Channel):
                                // stop reading and let the socket close below.
                                break;
                            }
                            if is_error {
                                // A daemon-reported error ends the stream.
                                break;
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
                WsMessage::Close(_) => break,
                _ => {}
            }
        }

        // Best-effort graceful close; ignore failures (we're tearing down).
        let _ = stream.send(WsMessage::Close(None)).await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_display_never_leaks_token() {
        // The token never flows into an error variant by construction; this
        // guards the contract against a future field addition that surfaces it.
        let cases = [
            X0xClientError::DaemonUnavailable("api-token missing"),
            X0xClientError::Transport("connection refused".into()),
            X0xClientError::Status(401, "unauthorized".into()),
            X0xClientError::Decode("EOF".into()),
        ];
        for e in cases {
            let s = e.to_string();
            assert!(!s.contains("Bearer"), "token prefix leaked: {s}");
        }
    }

    #[test]
    fn history_row_thread_fields_default_null() {
        // A daemon predating the thread contract omits thread_root/parent; the
        // row must still parse with both null.
        let json = serde_json::json!({
            "id": 7i64,
            "msg_id": "deadbeef",
            "scope": "group:abc",
            "author_agent": null,
            "author_machine": null,
            "sent_at_ms": 0i64,
            "seen_at_ms": 0i64,
            "direction": "inbound",
            "content_type": "text/plain",
            "payload": "",
            "signed": false,
            "provenance": "verified_envelope"
        });
        let row: HistoryRow = serde_json::from_value(json).expect("legacy row must parse");
        assert_eq!(row.thread_root, None);
        assert_eq!(row.thread_parent, None);
    }

    #[test]
    fn history_row_root_is_self_referential() {
        let root_id = "cafebabe";
        let json = serde_json::json!({
            "id": 1i64,
            "msg_id": root_id,
            "scope": "topic:m3",
            "author_agent": "00",
            "author_machine": null,
            "sent_at_ms": 1i64,
            "seen_at_ms": 1i64,
            "direction": "outbound",
            "content_type": "text/plain",
            "payload": "aGk=",
            "signed": true,
            "provenance": "verified_envelope",
            "thread_root": root_id,
            "thread_parent": null
        });
        let row: HistoryRow = serde_json::from_value(json).unwrap();
        assert_eq!(row.thread_root.as_deref(), Some(root_id));
        assert_eq!(row.thread_parent, None);
        // Self-referential root invariant: thread_root == msg_id.
        assert_eq!(row.thread_root.as_deref(), Some(row.msg_id.as_str()));
    }

    #[test]
    fn ws_frame_round_trips() {
        // The frame union must deserialize the daemon's WsOutbound shapes.
        let live = r#"{"type":"live","topic":"topic:m3"}"#;
        let f: X0xFrame = serde_json::from_str(live).unwrap();
        assert!(matches!(f, X0xFrame::Live { ref topic } if topic == "topic:m3"));

        let msg = r#"{"type":"message","topic":"topic:m3","payload":"aGk=","origin":"00","thread_root":"cafebabe","thread_parent":"deadbeef"}"#;
        let f: X0xFrame = serde_json::from_str(msg).unwrap();
        match f {
            X0xFrame::Message {
                thread_root,
                thread_parent,
                ..
            } => {
                assert_eq!(thread_root.as_deref(), Some("cafebabe"));
                assert_eq!(thread_parent.as_deref(), Some("deadbeef"));
            }
            _ => panic!("wrong variant"),
        }
    }
    #[test]
    fn ws_frame_serializes_camelcase_for_ts_channel() {
        // The Channel-facing frame MUST serialize camelCase (DesktopNativeApi TS
        // contract) while still deserializing the daemon's snake_case wire.
        let f = X0xFrame::Connected {
            session_id: "s1".into(),
            agent_id: "a1".into(),
        };
        let s = serde_json::to_string(&f).unwrap();
        assert!(s.contains(r#""type":"connected""#), "tag unchanged: {s}");
        assert!(
            s.contains(r#""sessionId":"s1""#),
            "camelCase sessionId: {s}"
        );
        assert!(s.contains(r#""agentId":"a1""#), "camelCase agentId: {s}");
        assert!(
            !s.contains("session_id"),
            "snake_case leaked into serialize: {s}"
        );

        let m = X0xFrame::Message {
            topic: "topic:m3".into(),
            payload: "aGk=".into(),
            origin: None,
            thread_root: Some("cafebabe".into()),
            thread_parent: None,
        };
        let s = serde_json::to_string(&m).unwrap();
        assert!(
            s.contains(r#""threadRoot":"cafebabe""#),
            "camelCase threadRoot: {s}"
        );

        // Round-trip: the daemon's snake_case wire still deserializes.
        let back: X0xFrame = serde_json::from_str(
            r#"{"type":"message","topic":"t","payload":"","origin":null,"thread_root":"x","thread_parent":null}"#,
        )
        .unwrap();
        assert!(
            matches!(back, X0xFrame::Message { thread_root, .. } if thread_root.as_deref() == Some("x"))
        );
    }
}
