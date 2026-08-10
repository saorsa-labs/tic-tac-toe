use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use base64::Engine as _;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::{self, client::IntoClientRequest, Message};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const HISTORY_PAGE_SIZE: usize = 200;
const MAX_HISTORY_PAGES: usize = 100;
const WS_RECONNECT_DELAY: Duration = Duration::from_secs(1);
const MAX_API_TOKEN_BYTES: usize = 8 * 1024;

#[derive(Debug, Error)]
pub enum X0xError {
    #[error("x0xd artifact unavailable: {0}")]
    Artifact(&'static str),
    #[error("x0xd transport failed: {0}")]
    Transport(String),
    #[error("x0xd returned HTTP {status}: {body}")]
    Status { status: u16, body: String },
    #[error("x0xd response decode failed: {0}")]
    Decode(String),
    #[error("x0xd identity mismatch: expected X0X_AGENT_ID {expected}, got {actual}")]
    IdentityMismatch { expected: String, actual: String },
    #[error("group is not signed_public")]
    UnsupportedGroup,
    #[error("history pagination exceeded the {MAX_HISTORY_PAGES}-page safety bound")]
    HistoryPageLimit,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ChannelEnvelope {
    pub text: String,
    pub created_at: u64,
    pub client_id: String,
    #[serde(default)]
    pub mentions: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct HistoryRow {
    pub id: i64,
    pub msg_id: String,
    pub scope: String,
    pub author_agent: Option<String>,
    pub direction: String,
    #[serde(rename = "content_type")]
    pub _content_type: String,
    pub payload: String,
    pub provenance: String,
    #[serde(rename = "sent_at_ms")]
    pub _sent_at_ms: i64,
    #[serde(rename = "seen_at_ms")]
    pub _seen_at_ms: i64,
    pub signed: bool,
    #[serde(default)]
    pub thread_root: Option<String>,
    #[serde(default)]
    pub thread_parent: Option<String>,
}

impl HistoryRow {
    pub fn envelope(&self) -> Option<ChannelEnvelope> {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(self.payload.as_bytes())
            .ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    pub fn is_verified_inbound(&self, stable_group_id: &str) -> bool {
        self.scope == format!("group:{stable_group_id}")
            && self.direction == "Inbound"
            && self.signed
            && self.provenance == "VerifiedEnvelope"
    }

    pub fn is_safe_context(&self, stable_group_id: &str, local_agent_id: &str) -> bool {
        self.is_verified_inbound(stable_group_id)
            || (self.scope == format!("group:{stable_group_id}")
                && self.direction == "Outbound"
                && self.signed
                && self.provenance == "LocalSend"
                && self.author_agent.as_deref() == Some(local_agent_id))
    }
}

#[derive(Debug, Deserialize)]
struct HistoryResponse {
    #[serde(default)]
    records: Vec<HistoryRow>,
    #[serde(default)]
    next_before_id: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct AgentResponse {
    agent_id: String,
}

#[derive(Debug, Deserialize)]
struct GroupDetail {
    policy: GroupPolicy,
}

#[derive(Debug, Deserialize)]
struct GroupPolicy {
    confidentiality: String,
}

#[derive(Debug, Deserialize)]
struct GroupState {
    group_id: String,
}

#[derive(Debug, Deserialize)]
struct SendResponse {
    #[serde(default)]
    msg_id: Option<String>,
}

#[derive(Debug, Serialize)]
struct SendBody<'a> {
    body: &'a str,
    kind: &'static str,
    thread_root: &'a str,
    thread_parent: &'a str,
}

#[derive(Debug, Serialize)]
struct SubscribeFrame<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    topics: [&'a str; 1],
}

#[derive(Clone)]
pub struct X0xClient {
    data_dir: PathBuf,
    http: reqwest::Client,
}

struct Resolved {
    api_base: String,
    ws_base: String,
    token: String,
}

impl X0xClient {
    pub fn new(data_dir: PathBuf) -> Result<Self, X0xError> {
        let http = reqwest::Client::builder()
            .no_proxy()
            .connect_timeout(REQUEST_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|error| X0xError::Transport(format!("HTTP client setup: {error}")))?;
        Ok(Self { data_dir, http })
    }

    pub async fn verify_identity(&self, expected_agent_id: &str) -> Result<(), X0xError> {
        let response: AgentResponse = self.get_json("/agent", &[]).await?;
        let actual = response.agent_id.to_ascii_lowercase();
        if actual != expected_agent_id {
            return Err(X0xError::IdentityMismatch {
                expected: expected_agent_id.to_string(),
                actual,
            });
        }
        Ok(())
    }

    pub async fn resolve_group(&self, route_group_id: &str) -> Result<String, X0xError> {
        let detail: GroupDetail = self
            .get_json(&format!("/groups/{route_group_id}"), &[])
            .await?;
        if detail.policy.confidentiality != "signed_public" {
            return Err(X0xError::UnsupportedGroup);
        }
        let state: GroupState = self
            .get_json(&format!("/groups/{route_group_id}/state"), &[])
            .await?;
        Ok(state.group_id)
    }

    pub async fn history_after(
        &self,
        stable_group_id: &str,
        watermark: i64,
    ) -> Result<Vec<HistoryRow>, X0xError> {
        let scope = format!("group:{stable_group_id}");
        let mut before_id: Option<i64> = None;
        let mut rows = Vec::new();

        for _ in 0..MAX_HISTORY_PAGES {
            let mut query = vec![
                ("scope".to_string(), scope.clone()),
                ("limit".to_string(), HISTORY_PAGE_SIZE.to_string()),
            ];
            if let Some(cursor) = before_id {
                query.push(("before_id".to_string(), cursor.to_string()));
            }
            let page: HistoryResponse = self.get_json("/history", &query).await?;
            let reached_watermark = page.records.iter().any(|row| row.id <= watermark);
            rows.extend(page.records.into_iter().filter(|row| row.id > watermark));
            if reached_watermark || page.next_before_id.is_none() {
                rows.sort_by_key(|row| row.id);
                return Ok(rows);
            }
            before_id = page.next_before_id;
        }

        Err(X0xError::HistoryPageLimit)
    }

    pub async fn recent_history(&self, stable_group_id: &str) -> Result<Vec<HistoryRow>, X0xError> {
        let query = vec![
            ("scope".to_string(), format!("group:{stable_group_id}")),
            ("limit".to_string(), "100".to_string()),
        ];
        let mut response: HistoryResponse = self.get_json("/history", &query).await?;
        response.records.sort_by_key(|row| row.id);
        Ok(response.records)
    }

    pub async fn send_group_reply(
        &self,
        route_group_id: &str,
        body: &str,
        thread_root: &str,
        thread_parent: &str,
    ) -> Result<Option<String>, X0xError> {
        let request = SendBody {
            body,
            kind: "chat",
            thread_root,
            thread_parent,
        };
        let response: SendResponse = self
            .post_json(&format!("/groups/{route_group_id}/send"), &request)
            .await?;
        Ok(response.msg_id)
    }

    pub async fn run_wake_stream(&self, stable_group_id: &str, wake_tx: mpsc::Sender<()>) {
        let topic = format!("x0x.groups.public.{stable_group_id}");
        loop {
            if let Err(error) = self.run_wake_connection(&topic, &wake_tx).await {
                tracing::warn!("x0xd WebSocket wake stream ended: {error}");
            }
            if wake_tx.is_closed() {
                return;
            }
            tokio::time::sleep(WS_RECONNECT_DELAY).await;
        }
    }

    async fn run_wake_connection(
        &self,
        topic: &str,
        wake_tx: &mpsc::Sender<()>,
    ) -> Result<(), X0xError> {
        let resolved = self.resolve()?;
        let mut request = format!("{}/ws", resolved.ws_base)
            .into_client_request()
            .map_err(|error| X0xError::Transport(format!("WS request: {error}")))?;
        let auth = tungstenite::http::HeaderValue::from_str(&format!("Bearer {}", resolved.token))
            .map_err(|error| X0xError::Transport(format!("WS auth header: {error}")))?;
        request
            .headers_mut()
            .insert(tungstenite::http::header::AUTHORIZATION, auth);
        let (mut stream, _) = tokio_tungstenite::connect_async(request)
            .await
            .map_err(|error| X0xError::Transport(format!("WS connect: {error}")))?;
        let subscribe = serde_json::to_string(&SubscribeFrame {
            kind: "subscribe",
            topics: [topic],
        })
        .map_err(|error| X0xError::Decode(format!("WS subscribe: {error}")))?;
        stream
            .send(Message::Text(subscribe.into()))
            .await
            .map_err(|error| X0xError::Transport(format!("WS subscribe: {error}")))?;

        while let Some(frame) = stream.next().await {
            match frame.map_err(|error| X0xError::Transport(format!("WS read: {error}")))? {
                Message::Text(text) => {
                    let value: serde_json::Value = match serde_json::from_str(&text) {
                        Ok(value) => value,
                        Err(_) => continue,
                    };
                    if value.get("type").and_then(serde_json::Value::as_str) == Some("message")
                        && value.get("topic").and_then(serde_json::Value::as_str) == Some(topic)
                        && wake_tx.send(()).await.is_err()
                    {
                        return Ok(());
                    }
                    if value.get("type").and_then(serde_json::Value::as_str) == Some("error") {
                        return Err(X0xError::Transport("daemon WS error frame".to_string()));
                    }
                }
                Message::Ping(payload) => {
                    stream
                        .send(Message::Pong(payload))
                        .await
                        .map_err(|error| X0xError::Transport(format!("WS pong: {error}")))?;
                }
                Message::Close(_) => return Ok(()),
                _ => {}
            }
        }
        Ok(())
    }

    async fn get_json<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        query: &[(String, String)],
    ) -> Result<T, X0xError> {
        let resolved = self.resolve()?;
        let response = self
            .http
            .get(format!("{}{}", resolved.api_base, path))
            .bearer_auth(&resolved.token)
            .query(query)
            .timeout(REQUEST_TIMEOUT)
            .send()
            .await
            .map_err(|error| X0xError::Transport(format!("GET {path}: {error}")))?;
        decode_response(response, path).await
    }

    async fn post_json<B: Serialize, T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T, X0xError> {
        let resolved = self.resolve()?;
        let response = self
            .http
            .post(format!("{}{}", resolved.api_base, path))
            .bearer_auth(&resolved.token)
            .json(body)
            .timeout(REQUEST_TIMEOUT)
            .send()
            .await
            .map_err(|error| X0xError::Transport(format!("POST {path}: {error}")))?;
        decode_response(response, path).await
    }

    fn resolve(&self) -> Result<Resolved, X0xError> {
        let address = read_api_address(&self.data_dir).ok_or(X0xError::Artifact("api.port"))?;
        let token = read_api_token(&self.data_dir).ok_or(X0xError::Artifact("api-token"))?;
        let host = match address {
            SocketAddr::V4(value) => value.ip().to_string(),
            SocketAddr::V6(value) => format!("[{}]", value.ip()),
        };
        Ok(Resolved {
            api_base: format!("http://{host}:{}", address.port()),
            ws_base: format!("ws://{host}:{}", address.port()),
            token,
        })
    }
}

async fn decode_response<T: serde::de::DeserializeOwned>(
    response: reqwest::Response,
    path: &str,
) -> Result<T, X0xError> {
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(X0xError::Status {
            status: status.as_u16(),
            body: body.chars().take(300).collect(),
        });
    }
    response
        .json()
        .await
        .map_err(|error| X0xError::Decode(format!("{path}: {error}")))
}

fn read_api_address(data_dir: &Path) -> Option<SocketAddr> {
    let raw = std::fs::read_to_string(data_dir.join("api.port")).ok()?;
    let address = raw.trim().parse::<SocketAddr>().ok()?;
    (address.ip().is_loopback() && address.port() != 0).then_some(address)
}

fn read_api_token(data_dir: &Path) -> Option<String> {
    let raw = std::fs::read_to_string(data_dir.join("api-token")).ok()?;
    let token = raw.trim();
    (!token.is_empty()
        && token.len() <= MAX_API_TOKEN_BYTES
        && !token
            .chars()
            .any(|character| character.is_control() || character.is_whitespace()))
    .then(|| token.to_string())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    use axum::extract::{Path as AxumPath, Query, State};
    use axum::http::{HeaderMap, StatusCode};
    use axum::routing::{get, post};
    use axum::{Json, Router};

    use super::*;

    #[derive(Clone)]
    struct FakeDaemon {
        token: String,
        agent_id: String,
        stable_group_id: String,
        posts: Arc<Mutex<Vec<serde_json::Value>>>,
    }

    #[test]
    fn envelope_decode_is_strict_and_preserves_mentions() {
        let body = serde_json::json!({
            "text": "ship it",
            "createdAt": 42,
            "clientId": "client-1",
            "mentions": ["a".repeat(64)]
        });
        let row = test_row(&body.to_string());
        let envelope = row.envelope().expect("valid envelope");
        assert_eq!(envelope.text, "ship it");
        assert_eq!(envelope.mentions, vec!["a".repeat(64)]);
    }

    #[test]
    fn invalid_payload_is_not_a_message() {
        let row = test_row("plain text");
        assert!(row.envelope().is_none());
    }

    #[test]
    fn verified_inbound_requires_every_security_field() {
        let mut row = test_row("{}");
        assert!(row.is_verified_inbound("stable"));
        row.signed = false;
        assert!(!row.is_verified_inbound("stable"));

        row.signed = true;
        row.direction = "inbound".to_string();
        assert!(!row.is_verified_inbound("stable"));
    }

    #[test]
    fn endpoint_control_files_require_numeric_loopback_and_token() {
        let data_dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(data_dir.path().join("api.port"), "localhost:1234\n")
            .expect("write hostname");
        assert!(read_api_address(data_dir.path()).is_none());

        std::fs::write(data_dir.path().join("api.port"), "192.0.2.1:1234\n")
            .expect("write non-loopback");
        assert!(read_api_address(data_dir.path()).is_none());

        std::fs::write(data_dir.path().join("api.port"), "127.0.0.1:1234\n")
            .expect("write loopback");
        assert!(read_api_address(data_dir.path()).is_some());

        std::fs::write(data_dir.path().join("api-token"), "invalid token\n")
            .expect("write invalid token");
        assert!(read_api_token(data_dir.path()).is_none());
    }

    #[tokio::test]
    async fn authenticated_history_and_threaded_send_match_daemon_protocol() {
        let data_dir = tempfile::tempdir().expect("tempdir");
        let state = FakeDaemon {
            token: "test-token".to_string(),
            agent_id: "a".repeat(64),
            stable_group_id: "stable".to_string(),
            posts: Arc::new(Mutex::new(Vec::new())),
        };
        let router = Router::new()
            .route("/agent", get(fake_agent))
            .route("/groups/{id}", get(fake_group))
            .route("/groups/{id}/state", get(fake_group_state))
            .route("/groups/{id}/send", post(fake_group_send))
            .route("/history", get(fake_history))
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind fake daemon");
        let port = listener.local_addr().expect("fake address").port();
        std::fs::write(
            data_dir.path().join("api.port"),
            format!("127.0.0.1:{port}\n"),
        )
        .expect("write port");
        std::fs::write(data_dir.path().join("api-token"), &state.token).expect("write token");
        let server = tokio::spawn(async move {
            axum::serve(listener, router)
                .await
                .expect("serve fake daemon");
        });

        let client = X0xClient::new(data_dir.path().to_path_buf()).expect("HTTP client");
        client
            .verify_identity(&state.agent_id)
            .await
            .expect("identity");
        assert_eq!(
            client.resolve_group("route").await.expect("group"),
            "stable"
        );
        let rows = client.history_after("stable", 1).await.expect("history");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, 2);

        let root = "1".repeat(64);
        let parent = "2".repeat(64);
        client
            .send_group_reply("route", "body", &root, &parent)
            .await
            .expect("send reply");
        let posts = state.posts.lock().expect("posts lock");
        assert_eq!(posts.len(), 1);
        assert_eq!(posts[0]["body"], "body");
        assert_eq!(posts[0]["thread_root"], root);
        assert_eq!(posts[0]["thread_parent"], parent);
        drop(posts);
        server.abort();
    }

    fn test_row(body: &str) -> HistoryRow {
        HistoryRow {
            id: 1,
            msg_id: "1".repeat(64),
            scope: "group:stable".to_string(),
            author_agent: Some("b".repeat(64)),
            direction: "Inbound".to_string(),
            _content_type: "text/plain".to_string(),
            payload: base64::engine::general_purpose::STANDARD.encode(body),
            provenance: "VerifiedEnvelope".to_string(),
            _sent_at_ms: 1,
            _seen_at_ms: 1,
            signed: true,
            thread_root: None,
            thread_parent: None,
        }
    }

    fn is_authorized(headers: &HeaderMap, state: &FakeDaemon) -> bool {
        let expected = format!("Bearer {}", state.token);
        headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            == Some(expected.as_str())
    }

    async fn fake_agent(
        State(state): State<FakeDaemon>,
        headers: HeaderMap,
    ) -> (StatusCode, Json<serde_json::Value>) {
        if !is_authorized(&headers, &state) {
            return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({})));
        }
        (
            StatusCode::OK,
            Json(serde_json::json!({
                "ok": true,
                "agent_id": state.agent_id
            })),
        )
    }

    async fn fake_group(
        State(state): State<FakeDaemon>,
        headers: HeaderMap,
        AxumPath(_id): AxumPath<String>,
    ) -> (StatusCode, Json<serde_json::Value>) {
        if !is_authorized(&headers, &state) {
            return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({})));
        }
        (
            StatusCode::OK,
            Json(serde_json::json!({
                "ok": true,
                "policy": { "confidentiality": "signed_public" }
            })),
        )
    }

    async fn fake_group_state(
        State(state): State<FakeDaemon>,
        headers: HeaderMap,
        AxumPath(_id): AxumPath<String>,
    ) -> (StatusCode, Json<serde_json::Value>) {
        if !is_authorized(&headers, &state) {
            return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({})));
        }
        (
            StatusCode::OK,
            Json(serde_json::json!({
                "ok": true,
                "group_id": state.stable_group_id
            })),
        )
    }

    async fn fake_group_send(
        State(state): State<FakeDaemon>,
        headers: HeaderMap,
        AxumPath(_id): AxumPath<String>,
        Json(body): Json<serde_json::Value>,
    ) -> (StatusCode, Json<serde_json::Value>) {
        if !is_authorized(&headers, &state) {
            return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({})));
        }
        state.posts.lock().expect("posts lock").push(body);
        (
            StatusCode::OK,
            Json(serde_json::json!({ "ok": true, "msg_id": "3".repeat(64) })),
        )
    }

    async fn fake_history(
        State(state): State<FakeDaemon>,
        headers: HeaderMap,
        Query(query): Query<HashMap<String, String>>,
    ) -> (StatusCode, Json<serde_json::Value>) {
        if !is_authorized(&headers, &state) {
            return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({})));
        }
        assert_eq!(query.get("scope").map(String::as_str), Some("group:stable"));
        let envelope = serde_json::json!({
            "text": "hello",
            "createdAt": 1,
            "clientId": "client",
            "mentions": [state.agent_id]
        });
        (
            StatusCode::OK,
            Json(serde_json::json!({
                "ok": true,
                "count": 1,
                "next_before_id": null,
                "records": [{
                    "id": 2,
                    "msg_id": "2".repeat(64),
                    "scope": "group:stable",
                    "author_agent": "b".repeat(64),
                    "direction": "Inbound",
                    "content_type": "text/plain",
                    "payload": base64::engine::general_purpose::STANDARD.encode(envelope.to_string()),
                    "provenance": "VerifiedEnvelope",
                    "sent_at_ms": 1,
                    "seen_at_ms": 1,
                    "signed": true,
                    "thread_root": null,
                    "thread_parent": null
                }]
            })),
        )
    }
}
