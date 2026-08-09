use std::sync::atomic::Ordering;

use tauri::{AppHandle, Emitter, Manager};

use super::{
    agent_readiness, append_log_marker, current_instance_id, find_managed_agent_mut,
    load_global_agent_config, load_managed_agents, load_personas, managed_agent_runtime_log_path,
    process_is_running, record_agent_command, resolve_effective_agent_env, save_managed_agents,
    spawn_agent_child, terminate_process, terminate_untracked_pair_runtime,
    write_agent_runtime_receipt, AgentReadiness, BackendKind, ManagedAgentPairRuntime,
    ManagedAgentRuntimeKey, ManagedAgentRuntimeLifecycle, ManagedAgentRuntimeReceipt,
    ManagedAgentRuntimeStatus,
};
use crate::app_state::AppState;

const STATUS_EVENT: &str = "managed-agent-runtime-status";

fn status_for(
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

fn emit_status(app: &AppHandle, status: &ManagedAgentRuntimeStatus) {
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

pub(crate) fn start_managed_agent_runtime_pair_lazy(
    pubkey: String,
    group_id: String,
    app: AppHandle,
) -> Result<ManagedAgentRuntimeStatus, String> {
    start_pair(pubkey, group_id, true, None, app)
}

#[tauri::command]
pub fn start_managed_agent_runtime(
    pubkey: String,
    group_id: String,
    app: AppHandle,
) -> Result<ManagedAgentRuntimeStatus, String> {
    start_managed_agent_runtime_pair_lazy(pubkey, group_id, app)
}

fn start_pair(
    pubkey: String,
    group_id: String,
    lazy: bool,
    expected_updated_at: Option<&str>,
    app: AppHandle,
) -> Result<ManagedAgentRuntimeStatus, String> {
    let state = app.state::<AppState>();
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
    if expected_updated_at.is_some_and(|expected| record.updated_at != expected) {
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

    let mut process = spawn_agent_child(&app, record, &key.group_id, lazy)?;
    let now = crate::util::now_iso();
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
    record.runtime_pid = None;
    record.updated_at = now.clone();
    record.last_started_at = Some(now);
    record.last_stopped_at = None;
    record.last_error = None;
    runtimes.insert(key.clone(), ManagedAgentPairRuntime::starting(process));
    let status = status_for(&app, record, &key, runtimes.get(&key), None);
    drop(runtimes);
    save_managed_agents(&app, &records)?;
    emit_status(&app, &status);
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
pub fn restart_managed_agent_runtime(
    pubkey: String,
    group_id: String,
    app: AppHandle,
) -> Result<ManagedAgentRuntimeStatus, String> {
    stop_managed_agent_runtime(pubkey.clone(), group_id.clone(), app.clone())?;
    start_pair(pubkey, group_id, true, None, app)
}

#[cfg(test)]
mod tests {
    use super::*;

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
