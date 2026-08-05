//! M3 native `x0xd` Tauri command surface — typed wrappers over the
//! authenticated loopback client owned by [`AppState`].
//!
//! These commands proxy the embedded `x0xd` daemon's REST/WS surface only.
//! They emit **no** relay events and publish **no** Nostr kinds. Authentication
//! and endpoint resolution stay inside [`X0xClient`](crate::x0x_client::X0xClient),
//! which permits loopback endpoints only and never exposes the bearer token.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};
use tauri::ipc::Channel;
use tauri::State;
use tokio::task::AbortHandle;

use crate::app_state::AppState;
use crate::x0x_client::{HistoryPage, X0xBackfillRequest, X0xFrame};

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

/// Frontend backfill options. The canonical scope is supplied separately and
/// injected into the daemon request so backfill and live delivery cannot drift.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveBackfillRequest {
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    before_id: Option<i64>,
    #[serde(default)]
    since_ms: Option<i64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveSubscription {
    stream_id: String,
}

fn topics_for_scope(
    scope: &str,
    override_topics: Option<Vec<String>>,
) -> Result<Vec<String>, String> {
    if let Some(topics) = override_topics {
        if topics.is_empty() || topics.iter().any(|topic| topic.trim().is_empty()) {
            return Err("live subscription topics must be non-empty".to_string());
        }
        return Ok(topics);
    }

    let (kind, id) = scope
        .split_once(':')
        .ok_or_else(|| "live subscription scope must be canonical".to_string())?;
    if id.is_empty() {
        return Err("live subscription scope id must be non-empty".to_string());
    }
    match kind {
        "topic" => Ok(vec![id.to_string()]),
        "group" => Ok(vec![format!("x0x.group.{id}.chat")]),
        "dm" => Err("dm live subscriptions require an explicit daemon topic".to_string()),
        _ => Err("live subscription scope kind is unsupported".to_string()),
    }
}

/// Open one daemon WebSocket that replays the requested history window and
/// then remains attached for live frames. The command returns immediately with
/// a stream id; [`x0x_close_live`] aborts the owning task and drops the socket.
#[tauri::command]
pub async fn x0x_subscribe_live(
    state: State<'_, AppState>,
    scope: String,
    topics: Option<Vec<String>>,
    backfill: Option<LiveBackfillRequest>,
    on_frame: Channel<X0xFrame>,
) -> Result<LiveSubscription, String> {
    let topics = topics_for_scope(&scope, topics)?;
    let backfill = backfill.map(|request| X0xBackfillRequest {
        limit: request.limit.unwrap_or(50),
        scope: Some(scope),
        before_id: request.before_id,
        since_ms: request.since_ms,
    });

    let stream_id = uuid::Uuid::new_v4().to_string();
    let cleanup_id = stream_id.clone();
    let client = state.x0x_client.clone();
    let (start_tx, start_rx) = tokio::sync::oneshot::channel::<()>();
    let task = tauri::async_runtime::spawn(async move {
        // Prevent natural completion from racing insertion into LIVE_STREAMS.
        if start_rx.await.is_err() {
            return;
        }

        let (tx, mut rx) = tokio::sync::mpsc::channel::<X0xFrame>(64);
        let transport = client.run_subscribe(topics, backfill, tx);
        tokio::pin!(transport);

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

        if let Err(error) = transport_result {
            // X0xClient errors contain sanitized stage/status context only;
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

    Ok(LiveSubscription { stream_id })
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

#[cfg(test)]
mod tests {
    use super::topics_for_scope;

    #[test]
    fn live_topics_resolve_topic_and_group_scopes() {
        assert_eq!(
            topics_for_scope("topic:dev", None),
            Ok(vec!["dev".to_string()])
        );
        assert_eq!(
            topics_for_scope("group:engineering", None),
            Ok(vec!["x0x.group.engineering.chat".to_string()])
        );
    }

    #[test]
    fn live_topics_reject_implicit_dm_and_empty_overrides() {
        assert!(topics_for_scope("dm:agent", None).is_err());
        assert!(topics_for_scope("topic:dev", Some(Vec::new())).is_err());
        assert!(topics_for_scope("topic:dev", Some(vec![String::new()])).is_err());
    }
}
