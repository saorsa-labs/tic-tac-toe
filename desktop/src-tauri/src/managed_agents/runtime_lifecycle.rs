use std::path::{Path, PathBuf};

use serde::Deserialize;
use tauri::{AppHandle, Manager};

use super::{
    load_managed_agents, terminate_process, ManagedAgentProcess, ManagedAgentRecord,
    ManagedAgentRuntimeKey, ManagedAgentRuntimeLifecycle,
};
use crate::app_state::AppState;

const AGENT_START_STABILITY_WINDOW: std::time::Duration = std::time::Duration::from_millis(250);
const AGENT_START_STABILITY_POLL_INTERVAL: std::time::Duration =
    std::time::Duration::from_millis(10);
const NATIVE_AGENT_READINESS_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

pub(crate) fn validate_native_agent_parallelism(parallelism: u32) -> Result<(), String> {
    if parallelism == 1 {
        Ok(())
    } else {
        Err(format!(
            "native x0x ACP supports exactly one worker; set agent parallelism to 1 (stored value is {parallelism})"
        ))
    }
}

pub(crate) fn prepare_native_harness_lifecycle(
    data_dir: &Path,
    group_id: &str,
) -> Result<PathBuf, String> {
    let path = data_dir.join(format!("buzz-acp-{group_id}.lifecycle.json"));
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(path),
        Err(error) => Err(format!(
            "failed to clear stale native harness lifecycle receipt: {error}"
        )),
    }
}

pub(crate) fn mark_agent_start_failure(
    record: &mut ManagedAgentRecord,
    message: &str,
    exit_code: Option<i32>,
) {
    let now = crate::util::now_iso();
    record.runtime_pid = None;
    record.updated_at = now.clone();
    record.last_stopped_at = Some(now);
    record.last_exit_code = exit_code;
    record.last_error = Some(message.to_string());
    record.last_error_code = None;
}

/// Refuse to register a harness that exits during its initial stabilization
/// window. Native harnesses additionally have to publish a nonce-bound
/// lifecycle receipt after establishing their durable history watermark.
pub(crate) fn stabilize_started_agent_process(
    process: &mut ManagedAgentProcess,
    record: &mut ManagedAgentRecord,
) -> Result<ManagedAgentRuntimeLifecycle, String> {
    let started = std::time::Instant::now();
    let readiness_required = process.lifecycle_path.is_some();
    let timeout = if readiness_required {
        NATIVE_AGENT_READINESS_TIMEOUT
    } else {
        AGENT_START_STABILITY_WINDOW
    };
    loop {
        match process.child.try_wait() {
            Ok(Some(status)) => {
                let message = match status.code() {
                    Some(code) => format!(
                        "agent harness `{}` for {} exited immediately with code {code}",
                        record.acp_command, record.name
                    ),
                    None => format!(
                        "agent harness `{}` for {} exited immediately ({status})",
                        record.acp_command, record.name
                    ),
                };
                mark_agent_start_failure(record, &message, status.code());
                return Err(message);
            }
            Ok(None) => {}
            Err(error) => {
                let message = format!(
                    "failed to inspect newly-started agent harness for {}: {error}",
                    record.name
                );
                mark_agent_start_failure(record, &message, None);
                return Err(message);
            }
        }

        if let Some(path) = process.lifecycle_path.as_deref() {
            match read_native_harness_lifecycle(path, &process.start_nonce) {
                Ok(Some((ManagedAgentRuntimeLifecycle::Failed, error))) => {
                    let message = error.unwrap_or_else(|| {
                        "native agent harness reported startup failure".to_string()
                    });
                    return fail_started_agent_process(process, record, &message);
                }
                Ok(Some((lifecycle, _))) => return Ok(lifecycle),
                Ok(None) => {}
                Err(error) => return fail_started_agent_process(process, record, &error),
            }
        }

        let elapsed = started.elapsed();
        if elapsed >= timeout {
            if readiness_required {
                let message = format!(
                    "native agent harness for {} did not publish a matching readiness receipt within {} seconds",
                    record.name,
                    NATIVE_AGENT_READINESS_TIMEOUT.as_secs()
                );
                return fail_started_agent_process(process, record, &message);
            }
            return Ok(ManagedAgentRuntimeLifecycle::Starting);
        }
        std::thread::sleep(AGENT_START_STABILITY_POLL_INTERVAL.min(timeout - elapsed));
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HarnessLifecycleReceipt {
    start_nonce: String,
    lifecycle: ManagedAgentRuntimeLifecycle,
    error: Option<String>,
}

pub(crate) fn read_native_harness_lifecycle(
    path: &Path,
    expected_nonce: &str,
) -> Result<Option<(ManagedAgentRuntimeLifecycle, Option<String>)>, String> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(format!(
                "failed to read native harness lifecycle receipt: {error}"
            ));
        }
    };
    let receipt: HarnessLifecycleReceipt = serde_json::from_slice(&bytes)
        .map_err(|error| format!("invalid native harness lifecycle receipt: {error}"))?;
    validate_harness_lifecycle_receipt(receipt, expected_nonce)
}

fn validate_harness_lifecycle_receipt(
    receipt: HarnessLifecycleReceipt,
    expected_nonce: &str,
) -> Result<Option<(ManagedAgentRuntimeLifecycle, Option<String>)>, String> {
    if receipt.start_nonce != expected_nonce {
        return Ok(None);
    }
    if matches!(
        receipt.lifecycle,
        ManagedAgentRuntimeLifecycle::Starting | ManagedAgentRuntimeLifecycle::Stopped
    ) {
        return Err("native harness authored an invalid lifecycle state".into());
    }
    let has_error = receipt
        .error
        .as_ref()
        .is_some_and(|error| !error.is_empty());
    if receipt.lifecycle == ManagedAgentRuntimeLifecycle::Failed && !has_error {
        return Err("failed native harness lifecycle requires an error".into());
    }
    if receipt.lifecycle != ManagedAgentRuntimeLifecycle::Failed && receipt.error.is_some() {
        return Err("native harness lifecycle error is only valid for failed".into());
    }
    Ok(Some((receipt.lifecycle, receipt.error)))
}

fn fail_started_agent_process<T>(
    process: &mut ManagedAgentProcess,
    record: &mut ManagedAgentRecord,
    message: &str,
) -> Result<T, String> {
    let _ = terminate_process(process.child.id());
    let exit_code = process.child.wait().ok().and_then(|status| status.code());
    mark_agent_start_failure(record, message, exit_code);
    Err(message.to_string())
}

pub(crate) fn spawn_native_lifecycle_monitor(
    app: AppHandle,
    key: ManagedAgentRuntimeKey,
    path: PathBuf,
    start_nonce: String,
) {
    let _ = std::thread::Builder::new()
        .name(format!("buzz-lifecycle-{}", &key.pubkey[..8]))
        .spawn(move || monitor_native_lifecycle(app, key, path, start_nonce));
}

fn monitor_native_lifecycle(
    app: AppHandle,
    key: ManagedAgentRuntimeKey,
    path: PathBuf,
    start_nonce: String,
) {
    loop {
        std::thread::sleep(std::time::Duration::from_millis(50));
        let (update, invalid_receipt) = match read_native_harness_lifecycle(&path, &start_nonce) {
            Ok(Some(update)) => (update, false),
            Ok(None) => continue,
            Err(error) => ((ManagedAgentRuntimeLifecycle::Failed, Some(error)), true),
        };
        let state = app.state::<AppState>();
        let mut runtimes = match state.managed_agent_processes.lock() {
            Ok(runtimes) => runtimes,
            Err(_) => return,
        };
        let Some(runtime) = runtimes.get_mut(&key) else {
            return;
        };
        if runtime.start_nonce != start_nonce {
            return;
        }
        match runtime.child.try_wait() {
            Ok(None) => {}
            Ok(Some(_)) | Err(_) => return,
        }
        if runtime.lifecycle == update.0 && runtime.error == update.1 {
            if runtime.lifecycle == ManagedAgentRuntimeLifecycle::Failed {
                return;
            }
            continue;
        }
        if invalid_receipt {
            let _ = terminate_process(runtime.child.id());
        }
        runtime.lifecycle = update.0;
        runtime.error = update.1;
        let failed = runtime.lifecycle == ManagedAgentRuntimeLifecycle::Failed;
        drop(runtimes);
        let records = match load_managed_agents(&app) {
            Ok(records) => records,
            Err(_) => return,
        };
        let Some(record) = records
            .iter()
            .find(|record| record.pubkey.eq_ignore_ascii_case(&key.pubkey))
        else {
            return;
        };
        let runtimes = match state.managed_agent_processes.lock() {
            Ok(runtimes) => runtimes,
            Err(_) => return,
        };
        let Some(runtime) = runtimes
            .get(&key)
            .filter(|runtime| runtime.start_nonce == start_nonce)
        else {
            return;
        };
        let status = super::runtime_commands::status_for(&app, record, &key, Some(runtime), None);
        drop(runtimes);
        super::runtime_commands::emit_status(&app, &status);
        if failed {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::managed_agents::{BackendKind, RespondTo};

    #[test]
    fn stale_generation_receipt_cannot_change_current_lifecycle() {
        let receipt = HarnessLifecycleReceipt {
            start_nonce: "old-generation".to_string(),
            lifecycle: ManagedAgentRuntimeLifecycle::Ready,
            error: None,
        };
        assert_eq!(
            validate_harness_lifecycle_receipt(receipt, "current-generation")
                .expect("stale receipt is ignored"),
            None
        );
    }

    #[test]
    fn matching_forged_starting_receipt_fails_closed() {
        let receipt = HarnessLifecycleReceipt {
            start_nonce: "current-generation".to_string(),
            lifecycle: ManagedAgentRuntimeLifecycle::Starting,
            error: None,
        };
        assert!(
            validate_harness_lifecycle_receipt(receipt, "current-generation")
                .expect_err("harness cannot self-author starting")
                .contains("invalid lifecycle")
        );
    }

    #[test]
    fn native_agent_parallelism_fails_closed_above_one() {
        assert!(validate_native_agent_parallelism(1).is_ok());
        let error = validate_native_agent_parallelism(24)
            .expect_err("native adapter must not pretend to provide 24 workers");
        assert!(error.contains("exactly one worker"));
        assert!(error.contains("stored value is 24"));
    }

    #[cfg(unix)]
    #[test]
    fn native_start_waits_until_watermark_readiness_receipt_exists() {
        let directory = tempfile::tempdir().expect("tempdir");
        let lifecycle_path = directory.path().join("lifecycle.json");
        let nonce = "0123456789abcdef0123456789abcdef".to_string();
        let child = std::process::Command::new("sh")
            .args(["-c", "sleep 5"])
            .spawn()
            .expect("spawn persistent harness");
        let mut process = ManagedAgentProcess {
            child,
            log_path: directory.path().join("agent.log"),
            spawn_config_hash: 0,
            setup_mode: false,
            adapter_availability: None,
            start_nonce: nonce.clone(),
            lifecycle_path: Some(lifecycle_path.clone()),
        };
        let writer = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(75));
            std::fs::write(
                lifecycle_path,
                serde_json::json!({
                    "startNonce": nonce,
                    "lifecycle": "listening",
                    "error": null
                })
                .to_string(),
            )
            .expect("write readiness receipt");
        });
        let mut record = record_fixture();
        let started = std::time::Instant::now();

        let lifecycle = stabilize_started_agent_process(&mut process, &mut record)
            .expect("matching listening receipt permits registration");

        assert_eq!(lifecycle, ManagedAgentRuntimeLifecycle::Listening);
        assert!(started.elapsed() >= std::time::Duration::from_millis(50));
        writer.join().expect("receipt writer");
        let _ = terminate_process(process.child.id());
        let _ = process.child.wait();
    }

    fn record_fixture() -> ManagedAgentRecord {
        ManagedAgentRecord {
            pubkey: "p".into(),
            name: "Guide".into(),
            persona_id: None,
            avatar_url: None,
            acp_command: "buzz-acp".into(),
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
            start_on_app_launch: false,
            auto_restart_on_config_change: true,
            runtime_pid: None,
            backend: BackendKind::Local,
            backend_agent_id: None,
            provider_binary_path: None,
            team_id: None,
            persona_team_dir: None,
            persona_name_in_team: None,
            created_at: "now".into(),
            updated_at: "now".into(),
            last_started_at: None,
            last_stopped_at: None,
            last_exit_code: None,
            last_error: None,
            last_error_code: None,
            respond_to: RespondTo::Anyone,
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
}
