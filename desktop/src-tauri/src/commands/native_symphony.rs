//! native_symphony — typed Tauri commands over the supervised x0x-symphonyd.
//!
//! Each command resolves the supervised loopback endpoint from [`AppState`],
//! reads the transient bearer token from the daemon data dir, builds a
//! [`SymphonyClient`], and proxies one daemon call. The token is never stored
//! on the command/state; errors propagate as structured [`String`]s that carry
//! the daemon's HTTP status + body (so `conflict`/`not_found` distinctions
//! surface to the operator).
//!
//! No Nostr relay event is emitted by any path here.

use std::path::{Path, PathBuf};

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
fn build_client(state: &AppState) -> Result<SymphonyClient, String> {
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
    let guard = state.local_symphony.lock().map_err(|e| e.to_string())?;
    Ok(match guard.as_ref() {
        Some(handle) => SymphonySupervisionStatus {
            running: true,
            base_url: Some(handle.base_url.clone()),
            owned: handle.owns_child(),
        },
        None => SymphonySupervisionStatus {
            running: false,
            base_url: None,
            owned: false,
        },
    })
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct SymphonySupervisionStatus {
    pub running: bool,
    pub base_url: Option<String>,
    pub owned: bool,
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

// ── Company product slice ─────────────────────────────────────────────────

#[derive(Debug, Clone, serde::Serialize)]
pub struct CompanyTemplateGroupView {
    id: String,
    name: String,
    kind: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct CompanyTemplateAgentView {
    role: String,
    group_id: String,
    runtime: Option<String>,
    model: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct CompanyTemplateView {
    id: String,
    name: String,
    description: Option<String>,
    groups: Vec<CompanyTemplateGroupView>,
    agents: Vec<CompanyTemplateAgentView>,
    is_builtin: bool,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstantiateCompanyTemplateInput {
    display_name: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct CompanyInstantiatedGroup {
    id: String,
    name: String,
    kind: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct CompanyInstantiatedAgent {
    agent_id: String,
    role: String,
    group_id: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct CompanyInstantiationError {
    kind: String,
    r#ref: String,
    message: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct CompanyInstantiationResult {
    instance_id: String,
    run_id: Option<String>,
    groups: Vec<CompanyInstantiatedGroup>,
    agents: Vec<CompanyInstantiatedAgent>,
    workflow_md: Option<String>,
    errors: Vec<CompanyInstantiationError>,
}

/// Built-in template picker data. Static and available even while symphonyd is
/// stopped, so Company remains discoverable and can explain what it creates.
#[tauri::command]
pub fn list_company_templates() -> Vec<CompanyTemplateView> {
    crate::company_template::registry::REGISTRY
        .iter()
        .map(|template| CompanyTemplateView {
            id: template.id.clone(),
            name: template.name.clone(),
            description: template.description.clone(),
            groups: template
                .groups
                .iter()
                .map(|group| CompanyTemplateGroupView {
                    id: group.id.clone(),
                    name: group.name.clone(),
                    kind: company_group_kind(&group.id),
                })
                .collect(),
            agents: template
                .roles
                .iter()
                .map(|role| CompanyTemplateAgentView {
                    role: role.id.clone(),
                    group_id: role.groups.first().cloned().unwrap_or_default(),
                    runtime: Some(role.harness.clone()),
                    model: role.model.clone(),
                })
                .collect(),
            is_builtin: true,
        })
        .collect()
}

/// Instantiate native x0xd resources, dedicated managed-agent identities, a
/// durable manifest, and the supervised Symphony run for the selected Company
/// template. Partial failures are returned per item; no relay fallback exists.
#[tauri::command]
pub async fn instantiate_company_template(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    template_id: String,
    input: InstantiateCompanyTemplateInput,
) -> Result<CompanyInstantiationResult, String> {
    use crate::company_template::instantiate::{instantiate_company, JsonManifestSink};
    use crate::company_template::plan::{instance_slug, plan_instantiation};
    use crate::company_template::provisioner::X0xHttpProvisioner;
    use crate::company_template::symphony_config::generate_symphony_config_for_x0xd;

    let template = crate::company_template::registry::get_company_template(&template_id)
        .ok_or_else(|| format!("unknown Company template `{template_id}`"))?;
    let display_name = input
        .display_name
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| template.name.clone());
    let created_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| format!("system clock is before Unix epoch: {error}"))?
        .as_millis();
    let instance_id = format!("{}-{created_at}", instance_slug(&display_name));
    let plan = plan_instantiation(&template, &instance_id);
    let instance_dir = company_instance_dir(&instance_id)?;
    let manifest = JsonManifestSink::new(instance_dir.join("manifest.json"));
    let provisioner = X0xHttpProvisioner::new(&state.x0x_client);
    let outcome = instantiate_company(&plan, &provisioner, &manifest).await;

    let mut errors: Vec<CompanyInstantiationError> = outcome
        .errors()
        .into_iter()
        .map(|(kind, target, message)| CompanyInstantiationError {
            kind: kind.to_string(),
            r#ref: target.unwrap_or_default().to_string(),
            message: message.to_string(),
        })
        .collect();
    let groups = realized_groups(&template, &outcome.steps);

    let identities = match crate::managed_agents::agent_identity::provision_company_agent_identities(
        &instance_id,
        &template.roles,
    ) {
        Ok(identities) => identities,
        Err(error) => {
            errors.push(CompanyInstantiationError {
                kind: "agent".to_string(),
                r#ref: "managed-agent-identities".to_string(),
                message: error.to_string(),
            });
            Vec::new()
        }
    };
    let agents = identities
        .into_iter()
        .map(|identity| {
            let group_id = template
                .role(&identity.role)
                .and_then(|role| role.groups.first())
                .cloned()
                .unwrap_or_default();
            CompanyInstantiatedAgent {
                agent_id: identity.agent_id,
                role: identity.role,
                group_id,
            }
        })
        .collect();

    let x0xd_url = owner_x0xd_endpoint()?;
    let workflow = generate_symphony_config_for_x0xd(&template, &instance_id, &x0xd_url);
    let workflow_path = instance_dir.join("WORKFLOW.md");
    write_private_atomic(&workflow_path, workflow.as_bytes())?;

    let app_handle = app.clone();
    let start_path = workflow_path.clone();
    tauri::async_runtime::spawn_blocking(move || {
        crate::symphony::bring_up_symphony(&app_handle, &start_path);
    })
    .await
    .map_err(|error| format!("Company symphony start task failed: {error}"))?;

    let run_id = if state
        .local_symphony
        .lock()
        .map(|guard| guard.is_some())
        .unwrap_or(false)
    {
        let client = build_client(&state)?;
        let draft = SymphonyIssueDraft {
            title: format!("Start {display_name}"),
            description: Some(format!(
                "Instantiate and operate Company template `{}`.",
                template.id
            )),
            priority: Some(1),
            labels: vec!["company".to_string(), template.id.clone()],
        };
        match client.create_issue(&draft).await {
            Ok(value) => value
                .get("id")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string),
            Err(error) => {
                errors.push(CompanyInstantiationError {
                    kind: "daemon".to_string(),
                    r#ref: "run".to_string(),
                    message: error.to_string(),
                });
                None
            }
        }
    } else {
        errors.push(CompanyInstantiationError {
            kind: "daemon".to_string(),
            r#ref: "start".to_string(),
            message: "symphony daemon did not become ready".to_string(),
        });
        None
    };

    Ok(CompanyInstantiationResult {
        instance_id,
        run_id,
        groups,
        agents,
        workflow_md: Some(workflow),
        errors,
    })
}

/// Cancel the local Company run and release its app-owned per-role x0xd
/// children. The durable x0xd task/store state remains available on restart.
#[tauri::command]
pub fn cancel_company_run(app: tauri::AppHandle, instance_id: String) -> Result<(), String> {
    crate::managed_agents::agent_identity::shutdown_company_agent_identities(&instance_id);
    crate::symphony::shutdown_symphony_owned(&app);
    Ok(())
}

fn company_instance_dir(instance_id: &str) -> Result<PathBuf, String> {
    dirs::data_dir()
        .map(|root| {
            root.join("x0x-company-ttt")
                .join(crate::company_template::plan::instance_slug(instance_id))
        })
        .ok_or_else(|| "could not resolve Company instance data directory".to_string())
}

fn owner_x0xd_endpoint() -> Result<String, String> {
    let data_dir = crate::local_stack::named_data_dir()
        .ok_or_else(|| "could not resolve owner x0xd data directory".to_string())?;
    let port = crate::local_stack::read_api_port(&data_dir)
        .ok_or_else(|| "owner x0xd api.port missing".to_string())?;
    // Verify the transient token exists before writing a workflow that the
    // child could never authenticate. The supervisor re-reads and injects it;
    // this copy is dropped immediately and never stored in product state.
    let _token = crate::local_stack::read_api_token(&data_dir)
        .ok_or_else(|| "owner x0xd api-token missing".to_string())?;
    Ok(crate::local_stack::loopback_api_base(port))
}

fn write_private_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "Company workflow path has no parent".to_string())?;
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("failed to create Company instance directory: {error}"))?;
    let temporary = path.with_extension("md.tmp");
    std::fs::write(&temporary, bytes)
        .map_err(|error| format!("failed to write Company workflow: {error}"))?;
    std::fs::rename(&temporary, path)
        .map_err(|error| format!("failed to commit Company workflow: {error}"))
}

fn realized_groups(
    template: &crate::company_template::spec::CompanyTemplate,
    steps: &[crate::company_template::instantiate::StepOutcome],
) -> Vec<CompanyInstantiatedGroup> {
    template
        .groups
        .iter()
        .filter_map(|group| {
            let realized = steps.iter().find(|step| {
                step.kind == "group" && step.local_id.as_deref() == Some(group.id.as_str())
            })?;
            Some(CompanyInstantiatedGroup {
                id: realized.resource_id.clone()?,
                name: group.name.clone(),
                kind: company_group_kind(&group.id),
            })
        })
        .collect()
}

fn company_group_kind(group_id: &str) -> String {
    match group_id {
        "engineering" => "engineering",
        "sales" => "sales",
        "all-hands" => "all_hands",
        _ => "custom",
    }
    .to_string()
}
