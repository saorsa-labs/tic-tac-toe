//! M3 native `x0xd` Tauri command surface — typed wrappers over the
//! authenticated loopback client owned by [`AppState`].
//!
//! These commands proxy the embedded `x0xd` daemon's REST/WS surface only.
//! They emit **no** relay events and publish **no** Nostr kinds. Authentication
//! and endpoint resolution stay inside [`X0xClient`](crate::x0x_client::X0xClient),
//! which permits loopback endpoints only and never exposes the bearer token.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};
use tauri::ipc::Channel;
use tauri::State;
use tokio::task::AbortHandle;

use crate::app_state::AppState;
use crate::x0x_client::{
    DirectSendReceipt, GroupConfidentiality, HistoryPage, HistoryRow, X0xBackfillRequest,
    X0xClientError, X0xFrame,
};

/// Abort handles for live streams opened by the frontend. A stream is removed
/// when it ends naturally or when [`x0x_close_live`] cancels it.
static LIVE_STREAMS: OnceLock<Mutex<HashMap<String, AbortHandle>>> = OnceLock::new();

fn live_streams() -> &'static Mutex<HashMap<String, AbortHandle>> {
    LIVE_STREAMS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// `GET /history` — scoped durable-history listing, newest-first.
#[tauri::command]
pub async fn x0x_history_list(
    state: State<'_, AppState>,
    scope: String,
    since_ms: Option<i64>,
    until_ms: Option<i64>,
    limit: Option<usize>,
    before_id: Option<i64>,
) -> Result<HistoryPage, String> {
    state
        .x0x_client
        .history_list(&crate::x0x_client::HistoryListRequest {
            scope,
            since_ms,
            until_ms,
            limit,
            before_id,
        })
        .await
        .map_err(Into::into)
}

/// `GET /history/search` — FTS5 search over text payloads within a scope.
#[tauri::command]
pub async fn x0x_history_search(
    state: State<'_, AppState>,
    scope: String,
    q: String,
    since_ms: Option<i64>,
    until_ms: Option<i64>,
    limit: Option<usize>,
    before_id: Option<i64>,
) -> Result<HistoryPage, String> {
    state
        .x0x_client
        .history_search(&crate::x0x_client::HistorySearchRequest {
            scope,
            q,
            since_ms,
            until_ms,
            limit,
            before_id,
        })
        .await
        .map_err(Into::into)
}

/// `GET /history/message/:msg_id` — single durable-history row by canonical
/// BLAKE3 `msg_id` (lowercase 64-hex). Returns `None` when the id is absent
/// from the local store (404), distinct from a transport/decode error.
///
/// No scope hint: `msg_id` is globally unique within one daemon's store, so a
/// canonical id unambiguously identifies one local row. The lookup never
/// reaches the network (ADR-0023 non-goal).
#[tauri::command]
pub async fn x0x_history_get(
    state: State<'_, AppState>,
    msg_id: String,
) -> Result<Option<HistoryRow>, String> {
    state
        .x0x_client
        .history_get(&msg_id)
        .await
        .map_err(Into::into)
}

/// `POST /publish` — publish base64 application bytes to a native topic.
#[tauri::command]
pub async fn x0x_publish(
    state: State<'_, AppState>,
    topic: String,
    payload_b64: String,
) -> Result<(), String> {
    state
        .x0x_client
        .publish(&topic, &payload_b64)
        .await
        .map_err(Into::into)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SendGroupMessageInput {
    group_id: String,
    body: String,
    #[serde(default)]
    kind: Option<String>,
    /// ADR-0029 native threading — optional 64-hex `msg_id` of the thread root.
    /// Forwarded to the daemon's `POST /groups/:id/send`; the secure
    /// (MlsEncrypted) route is closed entirely.
    #[serde(default)]
    thread_root: Option<String>,
    /// ADR-0029 native threading — optional 64-hex `msg_id` of the direct parent.
    #[serde(default)]
    thread_parent: Option<String>,
}

/// Secure (MlsEncrypted / GSS / TreeKEM) group sends are not available: the
/// secure-group messaging plane is NOT approved. Every MlsEncrypted send is
/// rejected at this boundary before any `/secure/send` route, so the renderer
/// can never trigger a secure-group encrypt/deliver through Tauri.
const NATIVE_SECURE_SEND_BLOCKER: &str = "Secure (MlsEncrypted/GSS/TreeKEM) group sends are not available: the secure-group messaging plane is pending approval.";

/// `POST /groups/:id/send` (SignedPublic only) — durable group send.
///
/// Resolves the group's confidentiality first ([`X0xClient::resolve_group_transport`]):
/// - **SignedPublic** → routes through the daemon's authority-signed send
///   (publishes on `x0x.groups.public.{stable_id}`, records under
///   `Scope::Group`). Optional `thread_root`/`thread_parent` (ADR-0029) are
///   forwarded to the daemon; the response carries the canonical `msg_id`
///   (`BLAKE3(signable_bytes)`) used for optimistic reconciliation.
/// - **MlsEncrypted** → REJECTED: secure-group (MLS/GSS/TreeKEM) sends are not
///   approved, so the `/groups/:id/secure/send` route is never reached.
///
/// Missing/unknown confidentiality fails closed inside
/// [`X0xClient::resolve_group_transport`] (never defaulted to MLS), so this
/// command only ever observes an explicitly-tagged SignedPublic or
/// MlsEncrypted group.
#[tauri::command]
pub async fn x0x_send_group_message(
    input: SendGroupMessageInput,
    state: State<'_, AppState>,
) -> Result<Option<String>, String> {
    let client = state.x0x_client.clone();
    let transport = client.resolve_group_transport(&input.group_id).await?;
    match transport.confidentiality {
        GroupConfidentiality::SignedPublic => {
            let kind = input.kind.as_deref().unwrap_or("chat");
            let msg_id = client
                .send_group_message(
                    &input.group_id,
                    &input.body,
                    kind,
                    input.thread_root.as_deref(),
                    input.thread_parent.as_deref(),
                )
                .await?;
            Ok(msg_id)
        }
        GroupConfidentiality::MlsEncrypted => {
            // Secure-group sends are closed: the MLS/GSS/TreeKEM plane is not
            // approved, so reject before any `/secure/send` route is reached.
            // (Missing/unknown confidentiality already failed closed inside
            // `resolve_group_transport`.)
            Err(NATIVE_SECURE_SEND_BLOCKER.to_string())
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SendDirectMessageInput {
    /// Recipient x0x AgentId (64-hex).
    agent_id: String,
    /// Base64-encoded application payload bytes.
    payload_b64: String,
    /// Optional 64-hex canonical msg_id of the thread root.
    #[serde(default)]
    thread_root: Option<String>,
    /// Optional 64-hex canonical msg_id of the direct parent.
    #[serde(default)]
    thread_parent: Option<String>,
}

/// `POST /direct/send` — native one-to-one direct message send.
///
/// Sends base64 application bytes to a connected agent over the daemon's
/// authenticated DM path. The recipient is a 64-hex `AgentId`; optional
/// `thread_root`/`thread_parent` carry native threading metadata (validated to
/// 32 bytes daemon-side). The daemon records the outbound row under
/// `dm:<recipient>`; live inbound frames arrive over `/ws/direct` and are
/// peer-filtered by the consumer.
#[tauri::command]
pub async fn x0x_send_direct_message(
    input: SendDirectMessageInput,
    state: State<'_, AppState>,
) -> Result<DirectSendReceipt, String> {
    state
        .x0x_client
        .send_direct_message(
            &input.agent_id,
            &input.payload_b64,
            input.thread_root.as_deref(),
            input.thread_parent.as_deref(),
        )
        .await
        .map_err(Into::into)
}

/// Frontend backfill options. Only `limit` is honoured by the daemon's WS
/// `WsBackfill`; scope/cursors are NOT supported on the live path (group/DM
/// durable history is cold-loaded via `x0x_history_list`). DM backfill is
/// passed as the `/ws/direct?backfill=N` query param instead.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveBackfillRequest {
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveSubscription {
    stream_id: String,
    /// The canonical durable-history scope the frontend should cold-load via
    /// `x0x_history_list` before/alongside this live stream. Only set for
    /// groups, where it is the **stable** id (`group:<stable_id>`, which can
    /// differ from the mls id used for REST routing). `None` for topic/dm —
    /// the caller already holds the scope.
    #[serde(skip_serializing_if = "Option::is_none")]
    history_scope: Option<String>,
}

/// Resolve the live topic name(s) for a `topic:` scope. A caller-supplied
/// `topics` override (rare) must be non-empty. Group and DM scopes resolve
/// their transport via the daemon inside [`x0x_subscribe_live`] and never
/// reach this helper.
fn resolve_topic_topics(
    id: &str,
    override_topics: Option<Vec<String>>,
) -> Result<Vec<String>, String> {
    if let Some(topics) = override_topics {
        if topics.is_empty() || topics.iter().any(|topic| topic.trim().is_empty()) {
            return Err("live subscription topics must be non-empty".to_string());
        }
        return Ok(topics);
    }
    Ok(vec![id.to_string()])
}

/// Open one daemon WebSocket and remain attached for live frames. Returns
/// immediately with a stream id; [`x0x_close_live`] aborts the owning task.
///
/// Per scope kind:
/// - **topic** → subscribe to the gossip topic(s); optional `limit` backfill
///   replays stored `Scope::Topic` rows (the only window the daemon honours).
/// - **group** → resolve the authoritative live topic from the daemon
///   ([`X0xClient::resolve_group_transport`]) and open **live-only**: the
///   daemon's WS backfill reads `Scope::Topic`, not `Scope::Group`, so group
///   durable history cannot be replayed here — cold-load it via
///   `x0x_history_list`.
/// - **dm** → open `/ws/direct` (auto-subscribes to all direct delivery) with
///   optional `backfill` stored-DM replay; the daemon delivers every DM to the
///   session, so peer filtering is the consumer's job.
#[tauri::command]
pub async fn x0x_subscribe_live(
    state: State<'_, AppState>,
    scope: String,
    topics: Option<Vec<String>>,
    backfill: Option<LiveBackfillRequest>,
    on_frame: Channel<X0xFrame>,
) -> Result<LiveSubscription, String> {
    let (kind, id) = scope
        .split_once(':')
        .ok_or_else(|| "live subscription scope must be canonical".to_string())?;
    if id.is_empty() {
        return Err("live subscription scope id must be non-empty".to_string());
    }

    let client = state.x0x_client.clone();

    /// What the spawned task drives — lets the topic/group (`/ws`) and dm
    /// (`/ws/direct`) futures share one frame→Channel pump.
    enum LivePlan {
        Ws {
            topics: Vec<String>,
            backfill: Option<X0xBackfillRequest>,
        },
        Direct {
            backfill: Option<usize>,
        },
    }

    let (plan, history_scope) = match kind {
        "topic" => {
            let topics = resolve_topic_topics(id, topics)?;
            let backfill = backfill.map(|b| X0xBackfillRequest {
                limit: b.limit.unwrap_or(50),
            });
            (LivePlan::Ws { topics, backfill }, None)
        }
        "group" => {
            // Authoritative topic from the daemon; live-only because the WS
            // backfill cannot read group-scoped durable history.
            let transport = client.resolve_group_transport(id).await?;
            // The durable history scope is the **stable** id (may differ from
            // the mls id used for REST routing) — surface it so cold-load via
            // x0x_history_list queries the right scope end-to-end.
            (
                LivePlan::Ws {
                    topics: vec![transport.live_topic],
                    backfill: None,
                },
                Some(format!("group:{}", transport.stable_group_id)),
            )
        }
        "dm" => (
            LivePlan::Direct {
                backfill: backfill.and_then(|b| b.limit),
            },
            None,
        ),
        _ => return Err("live subscription scope kind is unsupported".to_string()),
    };

    let stream_id = uuid::Uuid::new_v4().to_string();
    let cleanup_id = stream_id.clone();
    let (start_tx, start_rx) = tokio::sync::oneshot::channel::<()>();
    let task = tauri::async_runtime::spawn(async move {
        // Prevent natural completion from racing insertion into LIVE_STREAMS.
        if start_rx.await.is_err() {
            return;
        }

        let (tx, mut rx) = tokio::sync::mpsc::channel::<X0xFrame>(64);
        // Both run_subscribe and run_subscribe_direct drive the same
        // frame→Channel pump, so erase the future to one pinned type.
        let mut transport: Pin<Box<dyn Future<Output = Result<(), X0xClientError>> + Send>> =
            match plan {
                LivePlan::Ws { topics, backfill } => {
                    Box::pin(client.run_subscribe(topics, backfill, tx))
                }
                LivePlan::Direct { backfill } => {
                    Box::pin(client.run_subscribe_direct(backfill, tx))
                }
            };

        let transport_result = loop {
            tokio::select! {
                result = &mut transport => break result,
                frame = rx.recv() => {
                    let Some(frame) = frame else {
                        break Ok(());
                    };
                    if on_frame.send(frame).is_err() {
                        break Ok(());
                    }
                }
            }
        };

        // The transport may finish in the same scheduler turn that its last
        // frames enter the bounded bridge — drain them before reporting done.
        while let Ok(frame) = rx.try_recv() {
            if on_frame.send(frame).is_err() {
                break;
            }
        }

        if let Err(error) = transport_result {
            // X0xClient errors carry sanitized stage/status context only;
            // bearer tokens never enter their variants or Display output.
            let _ = on_frame.send(X0xFrame::Error {
                message: error.to_string(),
            });
        }

        if let Ok(mut streams) = live_streams().lock() {
            streams.remove(&cleanup_id);
        }
    });

    let abort_handle = task.inner().abort_handle();
    live_streams()
        .lock()
        .map_err(|error| format!("live stream registry unavailable: {error}"))?
        .insert(stream_id.clone(), abort_handle);
    let _ = start_tx.send(());

    Ok(LiveSubscription {
        stream_id,
        history_scope,
    })
}

/// Cancel a live stream. Idempotent so React effect cleanup can safely run
/// after natural socket closure or during strict-mode remounts.
#[tauri::command]
pub fn x0x_close_live(stream_id: String) -> Result<(), String> {
    let handle = live_streams()
        .lock()
        .map_err(|error| format!("live stream registry unavailable: {error}"))?
        .remove(&stream_id);
    if let Some(handle) = handle {
        handle.abort();
    }
    Ok(())
}

/// Cancel every live stream owned by the current desktop process. This is used
/// before a webview reload so the Rust tasks cannot outlive their listeners.
#[tauri::command]
pub fn x0x_close_all_live() -> Result<(), String> {
    let handles = live_streams()
        .lock()
        .map_err(|error| format!("live stream registry unavailable: {error}"))?
        .drain()
        .map(|(_, handle)| handle)
        .collect::<Vec<_>>();
    for handle in handles {
        handle.abort();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::resolve_topic_topics;

    #[test]
    fn topic_scope_resolves_id_or_override() {
        // No override ⇒ the topic id is the gossip topic.
        assert_eq!(
            resolve_topic_topics("dev", None),
            Ok(vec!["dev".to_string()])
        );
        // Override ⇒ honored verbatim (rare; for multi-topic or aliased topics).
        assert_eq!(
            resolve_topic_topics("dev", Some(vec!["a".into(), "b".into()])),
            Ok(vec!["a".to_string(), "b".to_string()])
        );
    }

    #[test]
    fn topic_scope_rejects_empty_overrides() {
        assert!(resolve_topic_topics("dev", Some(Vec::new())).is_err());
        assert!(resolve_topic_topics("dev", Some(vec![String::new()])).is_err());
        assert!(resolve_topic_topics("dev", Some(vec!["  ".to_string()])).is_err());
    }

    // NOTE: group live-topic resolution and dm `/ws/direct` routing live in the
    // async `x0x_subscribe_live` body (they require the daemon). The group
    // public-topic format is unit-tested in `x0x_client::tests`
    // (`group_public_topic_matches_daemon_public_topic_for`); the MlsEncrypted
    // chat_topic is an authoritative daemon value (no synthesis to assert).
}
