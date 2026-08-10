use std::sync::atomic::Ordering;

use tauri::{AppHandle, Emitter, Manager};

use super::{
    agent_readiness, append_log_marker, current_instance_id, find_managed_agent_mut,
    load_global_agent_config, load_managed_agents, load_personas, managed_agent_runtime_log_path,
    owner_group_has_existing_active_member, prepare_managed_agent_launch, process_is_running,
    record_agent_command, resolve_effective_agent_env, save_managed_agents, spawn_agent_child,
    spawn_native_lifecycle_monitor, stabilize_started_agent_process, terminate_process,
    terminate_untracked_pair_runtime, write_agent_runtime_receipt, AgentReadiness, BackendKind,
    GroupBindIntent, ManagedAgentCommunityTarget, ManagedAgentPairRuntime, ManagedAgentRecord,
    ManagedAgentRuntimeKey, ManagedAgentRuntimeLifecycle, ManagedAgentRuntimeReceipt,
    ManagedAgentRuntimeStatus,
};
use crate::app_state::AppState;
use crate::managed_agents::agent_identity::managed_agent_child_identity;

const STATUS_EVENT: &str = "managed-agent-runtime-status";

pub(crate) fn status_for(
    app: &AppHandle,
    record: &super::ManagedAgentRecord,
    key: &ManagedAgentRuntimeKey,
    runtime: Option<&ManagedAgentPairRuntime>,
    requested_group_id: Option<String>,
) -> ManagedAgentRuntimeStatus {
    let personas = load_personas(app).unwrap_or_default();
    let global = load_global_agent_config(app).unwrap_or_default();
    status_for_with(
        app,
        record,
        key,
        runtime,
        requested_group_id,
        StatusInputs {
            personas: &personas,
            global: &global,
        },
    )
}

/// Preloaded per-call-site inputs for [`status_for_with`], so multi-row
/// callers (list, reconcile) hit disk once instead of once per row.
struct StatusInputs<'a> {
    personas: &'a [super::AgentDefinition],
    global: &'a super::GlobalAgentConfig,
}

fn status_for_with(
    app: &AppHandle,
    record: &super::ManagedAgentRecord,
    key: &ManagedAgentRuntimeKey,
    runtime: Option<&ManagedAgentPairRuntime>,
    requested_group_id: Option<String>,
    inputs: StatusInputs<'_>,
) -> ManagedAgentRuntimeStatus {
    let StatusInputs { personas, global } = inputs;
    let command = record_agent_command(record, personas);
    let metadata = super::known_acp_runtime(&command);
    let effective = resolve_effective_agent_env(record, personas, metadata, global);
    let local_setup = matches!(agent_readiness(&effective), AgentReadiness::Ready);
    ManagedAgentRuntimeStatus {
        pubkey: key.pubkey.clone(),
        group_id: key.group_id.clone(),
        requested_group_id,
        local_setup,
        lifecycle: runtime
            .map(|runtime| runtime.lifecycle.clone())
            .unwrap_or(ManagedAgentRuntimeLifecycle::Stopped),
        pid: runtime.map(|runtime| runtime.child.id()),
        error: runtime.and_then(|runtime| runtime.error.clone()),
        log_path: managed_agent_runtime_log_path(app, key)
            .ok()
            .map(|path| path.display().to_string()),
    }
}

pub(crate) fn emit_status(app: &AppHandle, status: &ManagedAgentRuntimeStatus) {
    let _ = app.emit(STATUS_EVENT, status);
}

fn observer_lifecycle_key(
    outer_pubkey: &str,
    payload: &super::ManagedAgentRuntimeLifecycleObserverPayload,
) -> Result<ManagedAgentRuntimeKey, String> {
    if !outer_pubkey.eq_ignore_ascii_case(&payload.pubkey) {
        return Err("observer signer does not match lifecycle payload pubkey".into());
    }
    if matches!(
        payload.lifecycle,
        ManagedAgentRuntimeLifecycle::Starting | ManagedAgentRuntimeLifecycle::Stopped
    ) {
        return Err("observer cannot author starting or stopped lifecycle".into());
    }
    if payload.lifecycle == ManagedAgentRuntimeLifecycle::Failed && payload.error.is_none() {
        return Err("failed lifecycle requires an error".into());
    }
    if payload.lifecycle != ManagedAgentRuntimeLifecycle::Failed && payload.error.is_some() {
        return Err("lifecycle error is only valid for failed".into());
    }
    ManagedAgentRuntimeKey::new(payload.pubkey.clone(), &payload.group_id)
}

#[tauri::command]
pub fn put_managed_agent_runtime_lifecycle(
    outer_pubkey: String,
    payload: super::ManagedAgentRuntimeLifecycleObserverPayload,
    app: AppHandle,
) -> Result<ManagedAgentRuntimeStatus, String> {
    let key = observer_lifecycle_key(&outer_pubkey, &payload)?;
    let state = app.state::<AppState>();
    let records = load_managed_agents(&app)?;
    let record = records
        .iter()
        .find(|record| record.pubkey.eq_ignore_ascii_case(&key.pubkey))
        .ok_or_else(|| format!("agent {} not found", key.pubkey))?;
    let mut runtimes = state
        .managed_agent_processes
        .lock()
        .map_err(|e| e.to_string())?;
    let runtime = runtimes
        .get_mut(&key)
        .ok_or_else(|| "lifecycle frame does not match a tracked runtime pair".to_string())?;
    if runtime.start_nonce != payload.start_nonce {
        return Err("lifecycle frame does not match the current harness generation".into());
    }
    if runtime
        .child
        .try_wait()
        .map_err(|e| e.to_string())?
        .is_some()
    {
        return Err("lifecycle frame arrived after process exit".into());
    }
    runtime.lifecycle = payload.lifecycle;
    runtime.error = payload.error;
    let status = status_for(&app, record, &key, Some(runtime), None);
    emit_status(&app, &status);
    Ok(status)
}

#[tauri::command]
pub fn list_managed_agent_runtimes(
    app: AppHandle,
) -> Result<Vec<ManagedAgentRuntimeStatus>, String> {
    // This command is polled whenever the members sidebar opens and refetched
    // on every status event — load the per-row status inputs once, outside
    // the locks, instead of hitting disk per row while holding them.
    let personas = load_personas(&app).unwrap_or_default();
    let global = load_global_agent_config(&app).unwrap_or_default();
    let state = app.state::<AppState>();
    let _transition = state
        .managed_agent_runtime_transition
        .lock()
        .map_err(|e| e.to_string())?;
    let _store = state
        .managed_agents_store_lock
        .lock()
        .map_err(|e| e.to_string())?;
    let mut records = load_managed_agents(&app)?;
    let mut runtimes = state
        .managed_agent_processes
        .lock()
        .map_err(|e| e.to_string())?;
    let exited_keys: Vec<_> = runtimes
        .iter_mut()
        .filter_map(|(key, runtime)| match runtime.child.try_wait() {
            Ok(Some(_)) | Err(_) => Some(key.clone()),
            Ok(None) => None,
        })
        .collect();
    let records_changed = !exited_keys.is_empty();
    let mut statuses = Vec::new();
    for key in exited_keys {
        runtimes.remove(&key);
        super::remove_agent_runtime_receipt(&app, &key);
        state.clear_agent_session_cache(&key);
        if let Some(record) = records
            .iter_mut()
            .find(|record| record.pubkey.eq_ignore_ascii_case(&key.pubkey))
        {
            record.updated_at = crate::util::now_iso();
            record.last_stopped_at = Some(record.updated_at.clone());
            let status = status_for_with(
                &app,
                record,
                &key,
                None,
                None,
                StatusInputs {
                    personas: &personas,
                    global: &global,
                },
            );
            emit_status(&app, &status);
            statuses.push(status);
        }
    }
    statuses.extend(runtimes.iter().filter_map(|(key, runtime)| {
        let record = records
            .iter()
            .find(|record| record.pubkey.eq_ignore_ascii_case(&key.pubkey))?;
        Some(status_for_with(
            &app,
            record,
            key,
            Some(runtime),
            None,
            StatusInputs {
                personas: &personas,
                global: &global,
            },
        ))
    }));
    drop(runtimes);
    // Records are only mutated above when a runtime exited — skip the store
    // rewrite on the common nothing-changed poll.
    if records_changed {
        save_managed_agents(&app, &records)?;
    }
    Ok(statuses)
}

pub(crate) async fn start_managed_agent_runtime_pair_lazy(
    pubkey: String,
    group_id: String,
    app: AppHandle,
) -> Result<ManagedAgentRuntimeStatus, String> {
    start_pair(
        pubkey,
        group_id,
        true,
        None,
        GroupBindIntent::EnsureAttached,
        app,
    )
    .await
}

#[tauri::command]
pub async fn start_managed_agent_runtime(
    pubkey: String,
    group_id: String,
    app: AppHandle,
) -> Result<ManagedAgentRuntimeStatus, String> {
    start_managed_agent_runtime_pair_lazy(pubkey, group_id, app).await
}

async fn start_pair(
    pubkey: String,
    group_id: String,
    lazy: bool,
    expected_updated_at: Option<&str>,
    bind_intent: GroupBindIntent,
    app: AppHandle,
) -> Result<ManagedAgentRuntimeStatus, String> {
    let state = app.state::<AppState>();
    let record_snapshot = {
        let _store = state
            .managed_agents_store_lock
            .lock()
            .map_err(|e| e.to_string())?;
        let records = load_managed_agents(&app)?;
        records
            .iter()
            .find(|record| record.pubkey.eq_ignore_ascii_case(&pubkey))
            .cloned()
            .ok_or_else(|| format!("agent {pubkey} not found"))?
    };
    if record_snapshot.backend != BackendKind::Local {
        return Err("managed runtime pairs require a local agent".into());
    }
    if expected_updated_at.is_some_and(|expected| record_snapshot.updated_at != expected) {
        return Err("managed agent changed while runtime reconciliation was in flight".into());
    }
    let native_launch =
        prepare_managed_agent_launch(&app, &record_snapshot, &group_id, bind_intent).await?;

    let _transition = state
        .managed_agent_runtime_transition
        .lock()
        .map_err(|e| e.to_string())?;
    if state.shutdown_started.load(Ordering::Acquire) {
        return Err("desktop shutdown has started".into());
    }
    let _store = state
        .managed_agents_store_lock
        .lock()
        .map_err(|e| e.to_string())?;
    let mut records = load_managed_agents(&app)?;
    let record = find_managed_agent_mut(&mut records, &pubkey)?;
    if record.backend != BackendKind::Local {
        return Err("managed runtime pairs require a local agent".into());
    }
    if record.updated_at != record_snapshot.updated_at
        || expected_updated_at.is_some_and(|expected| record.updated_at != expected)
    {
        return Err("managed agent changed while runtime reconciliation was in flight".into());
    }
    let key = ManagedAgentRuntimeKey::new(pubkey, &group_id)?;
    let mut runtimes = state
        .managed_agent_processes
        .lock()
        .map_err(|e| e.to_string())?;
    if runtimes
        .get_mut(&key)
        .is_some_and(|runtime| runtime.child.try_wait().ok().flatten().is_none())
    {
        let status = status_for(&app, record, &key, runtimes.get(&key), None);
        return Ok(status);
    }
    runtimes.remove(&key);
    terminate_untracked_pair_runtime(&app, &key)?;

    let mut process = spawn_agent_child(&app, record, &key.group_id, lazy, &native_launch)?;
    let now = crate::util::now_iso();
    record.runtime_pid = None;
    record.updated_at = now.clone();
    record.last_started_at = Some(now.clone());
    record.last_stopped_at = None;
    record.last_exit_code = None;
    record.last_error = None;
    record.last_error_code = None;

    let initial_lifecycle = match stabilize_started_agent_process(&mut process, record) {
        Ok(lifecycle) => lifecycle,
        Err(error) => {
            runtimes.remove(&key);
            super::remove_agent_runtime_receipt(&app, &key);
            drop(runtimes);
            save_managed_agents(&app, &records)?;
            return Err(error);
        }
    };

    let receipt = ManagedAgentRuntimeReceipt {
        key: key.clone(),
        pid: process.child.id(),
        desktop_instance_id: current_instance_id(&app),
        started_at: now.clone(),
    };
    if let Err(error) = write_agent_runtime_receipt(&app, &receipt) {
        let _ = terminate_process(process.child.id());
        let _ = process.child.wait();
        return Err(error);
    }
    let lifecycle_path = process.lifecycle_path.clone();
    let start_nonce = process.start_nonce.clone();
    runtimes.insert(
        key.clone(),
        ManagedAgentPairRuntime::with_lifecycle(process, initial_lifecycle),
    );
    let status = status_for(&app, record, &key, runtimes.get(&key), None);
    drop(runtimes);
    save_managed_agents(&app, &records)?;
    emit_status(&app, &status);
    if let Some(path) = lifecycle_path {
        spawn_native_lifecycle_monitor(app.clone(), key, path, start_nonce);
    }
    Ok(status)
}

#[tauri::command]
pub fn stop_managed_agent_runtime(
    pubkey: String,
    group_id: String,
    app: AppHandle,
) -> Result<ManagedAgentRuntimeStatus, String> {
    let state = app.state::<AppState>();
    let _transition = state
        .managed_agent_runtime_transition
        .lock()
        .map_err(|e| e.to_string())?;
    let _store = state
        .managed_agents_store_lock
        .lock()
        .map_err(|e| e.to_string())?;
    let mut records = load_managed_agents(&app)?;
    let record = find_managed_agent_mut(&mut records, &pubkey)?;
    let key = ManagedAgentRuntimeKey::new(pubkey, &group_id)?;
    let mut runtimes = state
        .managed_agent_processes
        .lock()
        .map_err(|e| e.to_string())?;
    if let Some(mut runtime) = runtimes.remove(&key) {
        let stop_result = if process_is_running(runtime.child.id()) {
            terminate_process(runtime.child.id())
        } else {
            Ok(())
        }
        .and_then(|()| runtime.child.wait().map_err(|e| e.to_string()));
        match stop_result {
            Ok(status) => {
                record.last_exit_code = status.code();
                let _ = append_log_marker(&runtime.log_path, "=== stopped pair runtime ===");
            }
            Err(error) => {
                // Keep failed teardown visible/manageable instead of
                // orphaning it: the child stays tracked and the receipt
                // stays on disk until a stop actually succeeds.
                runtimes.insert(key, runtime);
                return Err(error);
            }
        }
    } else {
        // No runtime is tracked at this key, but a valid prior-session
        // receipt may still point at a live child (e.g. the crash-recovery
        // window for a non-auto-start agent). Terminate that orphan before
        // erasing its receipt — otherwise this "stop" leaves the harness
        // running yet deletes the one artifact sweeps and
        // terminate_untracked_pair_runtime use to find it, and a follow-up
        // start would spawn a duplicate harness for the same pair. On
        // failure the receipt stays on disk (terminate_untracked_pair_runtime
        // only removes it after the child exits), mirroring the tracked
        // path's keep-until-success invariant.
        terminate_untracked_pair_runtime(&app, &key)?;
    }
    super::remove_agent_runtime_receipt(&app, &key);
    state.clear_agent_session_cache(&key);
    record.runtime_pid = None;
    record.updated_at = crate::util::now_iso();
    record.last_stopped_at = Some(record.updated_at.clone());
    let status = status_for(&app, record, &key, None, None);
    drop(runtimes);
    save_managed_agents(&app, &records)?;
    emit_status(&app, &status);
    Ok(status)
}

#[tauri::command]
pub async fn restart_managed_agent_runtime(
    pubkey: String,
    group_id: String,
    app: AppHandle,
) -> Result<ManagedAgentRuntimeStatus, String> {
    stop_managed_agent_runtime(pubkey.clone(), group_id.clone(), app.clone())?;
    start_pair(
        pubkey,
        group_id,
        true,
        None,
        GroupBindIntent::EnsureAttached,
        app,
    )
    .await
}

fn auto_restore_allowed(state: &AppState) -> bool {
    state
        .managed_agent_auto_restore_allowed
        .load(Ordering::Acquire)
}

fn reconcile_jobs<F>(
    records: &[ManagedAgentRecord],
    group_ids: &[String],
    mut child_identity: F,
) -> Vec<(ManagedAgentRecord, String, String)>
where
    F: FnMut(&str) -> Option<String>,
{
    let mut jobs = Vec::new();
    for group_id in group_ids {
        for record in records
            .iter()
            .filter(|record| record.start_on_app_launch && record.backend == BackendKind::Local)
        {
            if let Some(child_agent_id) = child_identity(&record.pubkey) {
                jobs.push((record.clone(), group_id.clone(), child_agent_id));
            }
        }
    }
    jobs
}

fn normalize_reconcile_group_ids(
    communities: &[ManagedAgentCommunityTarget],
) -> Result<Vec<String>, String> {
    let mut seen = std::collections::HashSet::new();
    let mut group_ids = Vec::with_capacity(communities.len());
    for community in communities {
        let group_id = community.group_id.trim();
        if group_id.is_empty() {
            return Err("managed-agent reconciliation requires non-empty group ids".to_string());
        }
        if seen.insert(group_id.to_string()) {
            group_ids.push(group_id.to_string());
        }
    }
    Ok(group_ids)
}

async fn probe_existing_pair_membership(
    app: &AppHandle,
    group_id: &str,
    child_agent_id: &str,
) -> Result<bool, String> {
    let encoded_group = crate::path_segment(group_id)?;
    let group_path = format!("/groups/{encoded_group}");
    let state = app.state::<AppState>();
    let owner_group: serde_json::Value = state
        .x0x_client
        .get_json(&group_path, &[])
        .await
        .map_err(|error| format!("community {group_id} lookup failed: {error}"))?;
    owner_group_has_existing_active_member(&owner_group, group_id, child_agent_id)
}

fn failed_reconcile_status(
    app: &AppHandle,
    record: &ManagedAgentRecord,
    requested_group_id: String,
    error: String,
    personas: &[super::AgentDefinition],
    global: &super::GlobalAgentConfig,
) -> ManagedAgentRuntimeStatus {
    let status = ManagedAgentRuntimeKey::new(record.pubkey.clone(), &requested_group_id)
        .ok()
        .map(|key| {
            status_for_with(
                app,
                record,
                &key,
                None,
                Some(requested_group_id.clone()),
                StatusInputs { personas, global },
            )
        });
    let mut status = status.unwrap_or_else(|| {
        let command = record_agent_command(record, personas);
        let metadata = super::known_acp_runtime(&command);
        let effective = resolve_effective_agent_env(record, personas, metadata, global);
        ManagedAgentRuntimeStatus {
            pubkey: record.pubkey.clone(),
            group_id: requested_group_id.clone(),
            requested_group_id: Some(requested_group_id),
            local_setup: matches!(agent_readiness(&effective), AgentReadiness::Ready),
            lifecycle: ManagedAgentRuntimeLifecycle::Failed,
            pid: None,
            error: None,
            log_path: None,
        }
    });
    status.lifecycle = ManagedAgentRuntimeLifecycle::Failed;
    status.error = Some(error);
    status
}

/// Warm one lazy native harness for every eligible `(agent, group)` pair.
///
/// The caller supplies all group IDs explicitly. Only local auto-start records
/// with an already-persisted child identity and ACTIVE owner membership are
/// considered. The membership check is repeated by `ExistingOnly` launch
/// preparation, which may catch up child-local history but never changes the
/// owner's roster.
#[tauri::command]
pub async fn reconcile_managed_agent_runtimes(
    communities: Vec<ManagedAgentCommunityTarget>,
    app: AppHandle,
) -> Result<Vec<ManagedAgentRuntimeStatus>, String> {
    use futures_util::{stream, StreamExt as _};

    let state = app.state::<AppState>();
    if !auto_restore_allowed(&state) {
        return Ok(Vec::new());
    }

    let group_ids = normalize_reconcile_group_ids(&communities)?;
    let records = load_managed_agents(&app)?;
    let jobs = reconcile_jobs(&records, &group_ids, |pubkey| {
        managed_agent_child_identity(pubkey).map(|child| child.agent_id)
    });
    let probes: Vec<_> = stream::iter(jobs)
        .map(|(record, requested_group_id, child_agent_id)| {
            let probe_app = app.clone();
            async move {
                match probe_existing_pair_membership(
                    &probe_app,
                    &requested_group_id,
                    &child_agent_id,
                )
                .await
                {
                    Ok(true) => Ok(Some((record, requested_group_id, child_agent_id))),
                    Ok(false) => Ok(None),
                    Err(error) => Err((record, requested_group_id, error)),
                }
            }
        })
        .buffer_unordered(6)
        .collect()
        .await;

    let personas = load_personas(&app).unwrap_or_default();
    let global = load_global_agent_config(&app).unwrap_or_default();
    let mut rows = Vec::with_capacity(probes.len());
    for probe in probes {
        match probe {
            Ok(Some((record, requested_group_id, child_agent_id))) => {
                match start_pair(
                    record.pubkey.clone(),
                    requested_group_id.clone(),
                    true,
                    Some(&record.updated_at),
                    GroupBindIntent::ExistingOnly {
                        expected_child_agent_id: child_agent_id,
                    },
                    app.clone(),
                )
                .await
                {
                    Ok(mut status) => {
                        status.requested_group_id = Some(requested_group_id);
                        rows.push(status);
                    }
                    Err(error) => rows.push(failed_reconcile_status(
                        &app,
                        &record,
                        requested_group_id,
                        error,
                        &personas,
                        &global,
                    )),
                }
            }
            Ok(None) => {}
            Err((record, requested_group_id, error)) => rows.push(failed_reconcile_status(
                &app,
                &record,
                requested_group_id,
                error,
                &personas,
                &global,
            )),
        }
    }
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(
        pubkey_byte: char,
        start_on_app_launch: bool,
        backend: BackendKind,
    ) -> ManagedAgentRecord {
        ManagedAgentRecord {
            pubkey: pubkey_byte.to_string().repeat(64),
            name: format!("agent-{pubkey_byte}"),
            persona_id: None,
            team_id: None,
            avatar_url: None,
            acp_command: "buzz-agent".into(),
            agent_command: "goose".into(),
            agent_command_override: None,
            agent_args: Vec::new(),
            mcp_command: String::new(),
            turn_timeout_seconds: 320,
            idle_timeout_seconds: None,
            max_turn_duration_seconds: None,
            parallelism: 1,
            system_prompt: None,
            model: None,
            provider: None,
            persona_source_version: None,
            env_vars: std::collections::BTreeMap::new(),
            start_on_app_launch,
            auto_restart_on_config_change: true,
            runtime_pid: None,
            backend,
            backend_agent_id: None,
            provider_binary_path: None,
            persona_team_dir: None,
            persona_name_in_team: None,
            created_at: "2026-01-01T00:00:00Z".into(),
            updated_at: "2026-01-01T00:00:00Z".into(),
            last_started_at: None,
            last_stopped_at: None,
            last_exit_code: None,
            last_error: None,
            last_error_code: None,
            respond_to: super::super::RespondTo::OwnerOnly,
            respond_to_allowlist: Vec::new(),
            display_name: None,
            slug: None,
            runtime: None,
            name_pool: Vec::new(),
            is_builtin: false,
            is_active: true,
            source_team: None,
            source_team_persona_slug: None,
            definition_respond_to: None,
            definition_respond_to_allowlist: Vec::new(),
            definition_parallelism: None,
        }
    }

    fn payload(
        group_id: &str,
        lifecycle: ManagedAgentRuntimeLifecycle,
        error: Option<&str>,
    ) -> super::super::ManagedAgentRuntimeLifecycleObserverPayload {
        super::super::ManagedAgentRuntimeLifecycleObserverPayload {
            pubkey: "aa".repeat(32),
            group_id: group_id.into(),
            start_nonce: "test-generation".into(),
            lifecycle,
            error: error.map(str::to_owned),
        }
    }

    #[test]
    fn group_targets_trim_reject_empty_and_exact_dedupe() {
        let targets = vec![
            ManagedAgentCommunityTarget {
                group_id: " group-a ".into(),
            },
            ManagedAgentCommunityTarget {
                group_id: "group-a".into(),
            },
            ManagedAgentCommunityTarget {
                group_id: "Group-A".into(),
            },
            ManagedAgentCommunityTarget {
                group_id: "group-b".into(),
            },
        ];
        assert_eq!(
            normalize_reconcile_group_ids(&targets).unwrap(),
            vec!["group-a", "Group-A", "group-b"]
        );
        assert!(
            normalize_reconcile_group_ids(&[ManagedAgentCommunityTarget {
                group_id: "  ".into(),
            }])
            .is_err()
        );
    }

    #[test]
    fn planner_only_fans_out_local_auto_records_with_durable_identities() {
        let eligible = record('a', true, BackendKind::Local);
        let manual = record('b', false, BackendKind::Local);
        let provider = record(
            'c',
            true,
            BackendKind::Provider {
                id: "provider".into(),
                config: serde_json::json!({}),
            },
        );
        let missing_identity = record('d', true, BackendKind::Local);
        let records = vec![eligible.clone(), manual, provider, missing_identity];
        let submitted = vec!["group-one".to_string(), "Group-Two".to_string()];

        let jobs = reconcile_jobs(&records, &submitted, |pubkey| {
            (pubkey != "d".repeat(64)).then(|| "e".repeat(64))
        });

        assert_eq!(jobs.len(), 2);
        assert!(
            jobs.iter()
                .all(|(record, _, child)| record.pubkey == eligible.pubkey
                    && child == &"e".repeat(64))
        );
        assert_eq!(
            jobs.into_iter()
                .map(|(_, group, _)| group)
                .collect::<Vec<_>>(),
            submitted
        );
    }

    #[test]
    fn auto_restore_gate_is_fail_closed_and_session_long() {
        let state = crate::app_state::build_app_state();
        assert!(!auto_restore_allowed(&state));
        state
            .managed_agent_auto_restore_allowed
            .store(true, Ordering::Release);
        assert!(auto_restore_allowed(&state));
        assert!(auto_restore_allowed(&state));
    }

    #[test]
    fn runtime_key_rejects_non_hex_pubkeys() {
        assert!(ManagedAgentRuntimeKey::new("../not-a-key", "group-a").is_err());
        assert!(ManagedAgentRuntimeKey::new("gg".repeat(32), "group-a").is_err());
    }

    #[test]
    fn runtime_key_canonicalizes_hex_pubkeys() {
        let key = ManagedAgentRuntimeKey::new("AA".repeat(32), "group-a").unwrap();
        assert_eq!(key.pubkey, "aa".repeat(32));
        assert_eq!(key.group_id, "group-a");
    }

    #[test]
    fn observer_lifecycle_key_preserves_exact_group_pair() {
        let first = payload("group-a", ManagedAgentRuntimeLifecycle::Ready, None);
        let key = observer_lifecycle_key(&first.pubkey, &first).unwrap();
        assert_eq!(key.pubkey, first.pubkey);
        assert_eq!(key.group_id, "group-a");

        let other = payload("group-b", ManagedAgentRuntimeLifecycle::Ready, None);
        assert_ne!(key, observer_lifecycle_key(&other.pubkey, &other).unwrap());
    }

    #[test]
    fn observer_lifecycle_rejects_cross_agent_and_desktop_states() {
        let ready = payload("group-a", ManagedAgentRuntimeLifecycle::Ready, None);
        assert!(observer_lifecycle_key(&"bb".repeat(32), &ready).is_err());

        let stopped = payload("group-a", ManagedAgentRuntimeLifecycle::Stopped, None);
        assert!(observer_lifecycle_key(&stopped.pubkey, &stopped).is_err());
    }

    #[test]
    fn observer_lifecycle_enforces_failed_error_contract() {
        let failed = payload("group-a", ManagedAgentRuntimeLifecycle::Failed, None);
        assert!(observer_lifecycle_key(&failed.pubkey, &failed).is_err());

        let ready_with_error = payload(
            "group-a",
            ManagedAgentRuntimeLifecycle::Ready,
            Some("unexpected"),
        );
        assert!(observer_lifecycle_key(&ready_with_error.pubkey, &ready_with_error).is_err());
    }
}
