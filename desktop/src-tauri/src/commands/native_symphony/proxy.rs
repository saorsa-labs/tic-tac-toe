use std::path::PathBuf;

use futures_util::StreamExt;
use tauri::{Emitter, Manager, State};

use crate::app_state::AppState;
use crate::local_stack::read_api_token;
use crate::symphony_client::{
    parse_sse_frame, SymphonyApprovalEvent, SymphonyClaimResponse, SymphonyClient,
    SymphonyClientError, SymphonyHandoffResponse, SymphonyIssueDraft, SymphonyPendingApproval,
    SymphonyProof, SymphonyProofList, SymphonyRouteInfo, SymphonyStatus, SymphonyTask,
    SymphonyWorkers,
};

/// Resolve the supervised daemon's loopback base URL + data dir from state
/// without holding the lock across an await.
fn symphony_endpoint(state: &AppState) -> Result<(String, PathBuf), String> {
    let guard = state
        .local_symphony
        .lock()
        .map_err(|e| format!("symphony state lock poisoned: {e}"))?;
    let handle = guard
        .as_ref()
        .ok_or_else(|| "symphony daemon is not running".to_string())?;
    Ok((handle.base_url.clone(), handle.data_dir.clone()))
}

/// Build a fail-closed client: loopback base + transiently-read token + shared
/// app HTTP client. The lock is released before this returns.
pub(super) fn build_client(state: &AppState) -> Result<SymphonyClient, String> {
    let (base_url, data_dir) = symphony_endpoint(state)?;
    let token = read_api_token(&data_dir)
        .ok_or_else(|| "symphony api-token missing; daemon not ready".to_string())?;
    SymphonyClient::new(&base_url, token, state.http_client.clone()).map_err(|e| e.to_string())
}

/// Whether the symphony daemon is supervised and (if owned) the base URL it is
/// reachable at. `running=false` when never started or already shut down.
#[tauri::command]
pub fn symphony_supervision_status(
    state: State<'_, AppState>,
) -> Result<SymphonySupervisionStatus, String> {
    // Resolve running/base_url/owned under the symphony lock, then drop it
    // before touching the (separate) error/instance locks.
    let (running, base_url, owned) = {
        let guard = state.local_symphony.lock().map_err(|e| e.to_string())?;
        (
            guard.as_ref().is_some(),
            guard.as_ref().map(|handle| handle.base_url.clone()),
            guard.as_ref().is_some_and(|handle| handle.owns_child()),
        )
    };
    let active_instance_id = state
        .active_company_instance
        .lock()
        .map(|guard| guard.clone())
        .unwrap_or(None);
    // Only surface the captured bring-up error when the daemon is NOT running
    // — a successful bind supersedes a prior failure.
    let error = if running {
        None
    } else {
        state
            .symphony_error
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or(None)
    };
    Ok(SymphonySupervisionStatus {
        running,
        base_url,
        owned,
        active_instance_id,
        error,
    })
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct SymphonySupervisionStatus {
    pub running: bool,
    pub base_url: Option<String>,
    pub owned: bool,
    /// The instance id of the Company bound to this daemon, or `None` when no
    /// company is active (or a non-company symphony is bound).
    pub active_instance_id: Option<String>,
    /// The captured symphony bring-up error, surfaced only when `running` is
    /// false so a failed bring-up is visible rather than just `running:false`.
    pub error: Option<String>,
}

/// Start (or attach to) the supervised symphony daemon against a `WORKFLOW.md`
/// config path. Blocking bring-up (bounded readiness poll) runs on the blocking
/// pool so it never stalls the runtime. Returns `true` once the daemon is up.
#[tauri::command]
pub async fn start_symphony(app: tauri::AppHandle, config_path: String) -> Result<bool, String> {
    let path = PathBuf::from(config_path);
    let app_handle = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        crate::symphony::bring_up_symphony(&app_handle, &path);
    })
    .await
    .map_err(|e| format!("symphony start task failed: {e}"))?;
    let state = app.state::<AppState>();
    let up = state
        .local_symphony
        .lock()
        .map(|g| g.is_some())
        .unwrap_or(false);
    Ok(up)
}

/// Stop the supervised symphony daemon (reap the app-owned child only; an
/// attached daemon is left running). Idempotent.
#[tauri::command]
pub fn stop_symphony(app: tauri::AppHandle) -> Result<bool, String> {
    crate::symphony::shutdown_symphony_owned(&app);
    Ok(true)
}

/// `GET /symphony/tasks` — list tasks, optionally filtered by state.
#[tauri::command]
pub async fn symphony_tasks(
    state: State<'_, AppState>,
    state_filter: Option<String>,
) -> Result<Vec<SymphonyTask>, String> {
    let client = build_client(&state)?;
    client.tasks(state_filter.as_deref()).await.map_err(map_err)
}

/// `GET /symphony/tasks/{id}` — full task detail (opaque JSON).
#[tauri::command]
pub async fn symphony_task(
    state: State<'_, AppState>,
    id: String,
) -> Result<serde_json::Value, String> {
    let client = build_client(&state)?;
    client.task(&id).await.map_err(map_err)
}

/// `GET /symphony/status` — daemon + orchestrator status with active claims.
#[tauri::command]
pub async fn symphony_status(state: State<'_, AppState>) -> Result<SymphonyStatus, String> {
    let client = build_client(&state)?;
    client.status().await.map_err(map_err)
}

/// `GET /symphony/workers` — live worker-discovery cards.
#[tauri::command]
pub async fn symphony_workers(state: State<'_, AppState>) -> Result<SymphonyWorkers, String> {
    let client = build_client(&state)?;
    client.workers().await.map_err(map_err)
}

/// `GET /symphony/approvals/pending` — network-sourced issues awaiting consent.
#[tauri::command]
pub async fn symphony_approvals_pending(
    state: State<'_, AppState>,
) -> Result<Vec<SymphonyPendingApproval>, String> {
    let client = build_client(&state)?;
    client.approvals_pending().await.map_err(map_err)
}

/// `POST /symphony/approvals/{id}` — approve a network-sourced issue. Optional
/// `expected_content_hash`/`expected_signer_agent_id` guard against a race
/// where the payload changed between view and approve.
#[tauri::command]
pub async fn symphony_approve(
    state: State<'_, AppState>,
    id: String,
    expected_content_hash: Option<String>,
    expected_signer_agent_id: Option<String>,
) -> Result<SymphonyApprovalEvent, String> {
    let client = build_client(&state)?;
    client
        .approve(
            &id,
            expected_content_hash.as_deref(),
            expected_signer_agent_id.as_deref(),
        )
        .await
        .map_err(map_err)
}

/// `POST /symphony/approvals/{id}` — deny a network-sourced issue.
#[tauri::command]
pub async fn symphony_deny(
    state: State<'_, AppState>,
    id: String,
    expected_content_hash: Option<String>,
    expected_signer_agent_id: Option<String>,
) -> Result<SymphonyApprovalEvent, String> {
    let client = build_client(&state)?;
    client
        .deny(
            &id,
            expected_content_hash.as_deref(),
            expected_signer_agent_id.as_deref(),
        )
        .await
        .map_err(map_err)
}

/// `POST /symphony/issues` — create a symphony-owned issue.
#[tauri::command]
pub async fn symphony_create_issue(
    state: State<'_, AppState>,
    title: String,
    description: Option<String>,
    priority: Option<i32>,
    labels: Vec<String>,
) -> Result<serde_json::Value, String> {
    let client = build_client(&state)?;
    let draft = SymphonyIssueDraft {
        title,
        description,
        priority,
        labels,
    };
    client.create_issue(&draft).await.map_err(map_err)
}

/// `POST /symphony/claim/{id}` — claim an issue for this daemon's agent.
#[tauri::command]
pub async fn symphony_claim(
    state: State<'_, AppState>,
    id: String,
) -> Result<SymphonyClaimResponse, String> {
    let client = build_client(&state)?;
    client.claim(&id).await.map_err(map_err)
}

/// `POST /symphony/handoff/{id}` — record a handoff for a claimed issue.
#[tauri::command]
pub async fn symphony_handoff(
    state: State<'_, AppState>,
    id: String,
    message: String,
    file: Option<String>,
) -> Result<SymphonyHandoffResponse, String> {
    let client = build_client(&state)?;
    client.handoff(&id, message, file).await.map_err(map_err)
}

/// `GET /symphony/proofs` — proof artefact names.
#[tauri::command]
pub async fn symphony_proofs(state: State<'_, AppState>) -> Result<SymphonyProofList, String> {
    let client = build_client(&state)?;
    client.proofs().await.map_err(map_err)
}

/// `GET /symphony/proofs/{name}` — one proof artefact's UTF-8 content.
#[tauri::command]
pub async fn symphony_proof(
    state: State<'_, AppState>,
    name: String,
) -> Result<SymphonyProof, String> {
    let client = build_client(&state)?;
    client.proof(&name).await.map_err(map_err)
}

/// `GET /symphony/routes` — the daemon's own route table (for discovery/debug).
#[tauri::command]
pub async fn symphony_routes(state: State<'_, AppState>) -> Result<Vec<SymphonyRouteInfo>, String> {
    let client = build_client(&state)?;
    client.routes().await.map(|r| r.routes).map_err(map_err)
}

/// Subscribe to the daemon's SSE event bus (`GET /symphony/events`) and re-emit
/// each frame as a Tauri `symphony-event` event `{ event, data }`. The token
/// rides as `?token=` (EventSource cannot set headers) over a no-redirect client
/// so a 3xx can never forward it off-origin. Runs until the stream closes or
/// errors, then emits `symphony-event-stream-ended`. Returns immediately.
#[tauri::command]
pub async fn symphony_subscribe_events(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let client = build_client(&state)?;
    let url = client.event_stream_url();
    let app_handle = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(err) = run_symphony_event_stream(&app_handle, &url).await {
            eprintln!("symphony: event stream ended with error: {err}");
            let _ = app_handle.emit(
                "symphony-event-stream-ended",
                serde_json::json!({ "error": err }),
            );
            return;
        }
        let _ = app_handle.emit(
            "symphony-event-stream-ended",
            serde_json::json!({ "error": serde_json::Value::Null }),
        );
    });
    Ok(())
}

/// Forward the SSE stream, emitting one `symphony-event` per parsed frame.
async fn run_symphony_event_stream(app: &tauri::AppHandle, url: &str) -> Result<(), String> {
    use tauri::Emitter;
    let http = crate::symphony_client::build_sse_client()
        .map_err(|e| format!("sse client build failed: {e}"))?;
    let response = http
        .get(url)
        .send()
        .await
        .map_err(|_| "sse connect failed".to_string())?;
    if !response.status().is_success() {
        return Err(format!(
            "sse connect returned HTTP {}",
            response.status().as_u16()
        ));
    }
    let mut stream = response.bytes_stream();
    let mut buffer = String::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| "sse read failed".to_string())?;
        buffer.push_str(&String::from_utf8_lossy(&chunk));
        while let Some(frame) = parse_sse_frame(&mut buffer) {
            let _ = app.emit(
                "symphony-event",
                serde_json::json!({ "event": frame.event, "data": frame.data }),
            );
        }
    }
    Ok(())
}

/// Surface the daemon's HTTP status + body (e.g. `conflict`/`not_found`) so the
/// operator sees the real failure rather than a generic transport string.
fn map_err(e: SymphonyClientError) -> String {
    e.to_string()
}
