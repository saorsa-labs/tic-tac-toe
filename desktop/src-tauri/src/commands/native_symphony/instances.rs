use std::path::PathBuf;

use tauri::State;

use crate::app_state::AppState;
use crate::company_template::instantiate::ManifestStatus;

use super::fs_util::write_private_atomic;
use super::lifecycle::{lifecycle_is_complete, run_company_lifecycle};
#[cfg(test)]
use super::test_injection;

/// Cancel the local Company run: write a durable `Cancelled` tombstone (so a
/// restart never resumes it), release the app-owned per-role x0xd children, and
/// shut down the supervised symphony daemon. Durable x0xd task/store state
/// remains on disk. The active binding is cleared iff it points at this
/// instance. Returns `Err` if the tombstone cannot be persisted.
#[tauri::command]
pub fn cancel_company_run(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    instance_id: String,
) -> Result<(), String> {
    use crate::company_template::instantiate::{JsonManifestSink, ManifestSink};

    // Durable Cancelled tombstone FIRST, so a crash mid-cancel still leaves a
    // cancelled instance that boot reconciliation skips.
    let manifest_path = company_instance_dir(&instance_id)?.join("manifest.json");
    let sink = JsonManifestSink::new(&manifest_path);
    let mut manifest = sink
        .load()
        .map_err(|error| format!("failed to load Company manifest for cancel: {error}"))?;
    if manifest.status != ManifestStatus::Cancelled {
        manifest.status = ManifestStatus::Cancelled;
        // Bump the generation so any in-flight lifecycle run observes the
        // cancel (it re-reads the generation before every phase and save) and
        // stops without overwriting this tombstone.
        manifest.cancel_generation = manifest.cancel_generation.saturating_add(1);
        manifest.cancelled_at = Some(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|error| format!("system clock is before Unix epoch: {error}"))?
                .as_millis(),
        );
        sink.save(&manifest)
            .map_err(|error| format!("failed to persist Company cancel tombstone: {error}"))?;
    }

    crate::managed_agents::agent_identity::shutdown_company_agent_identities(&instance_id);
    crate::symphony::shutdown_symphony_owned(&app);

    let mut guard = state
        .active_company_instance
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if guard.as_deref() == Some(instance_id.as_str()) {
        *guard = None;
    }
    Ok(())
}

/// A persisted Company instance summary for runId→instanceId reconstruction.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompanyInstanceSummary {
    pub instance_id: String,
    pub run_id: Option<String>,
    /// Manifest lifecycle status serialized snake_case: "complete" |
    /// "resumable" | "in_progress" | "cancelled".
    pub status: ManifestStatus,
    /// `true` iff this instance is the company currently bound to the
    /// supervised symphony daemon.
    pub active: bool,
}

/// List all persisted Company instances from disk (the durable authority;
/// localStorage is only a cache), each annotated with its manifest status and
/// whether it is the company currently bound to the supervised symphony daemon.
#[tauri::command]
pub fn list_company_instances(state: State<'_, AppState>) -> Vec<CompanyInstanceSummary> {
    let active = state
        .active_company_instance
        .lock()
        .map(|guard| guard.clone())
        .unwrap_or(None);
    scan_company_instances()
        .into_iter()
        .map(|inst| {
            let is_active = active.as_deref() == Some(inst.instance_id.as_str());
            CompanyInstanceSummary {
                instance_id: inst.instance_id,
                run_id: inst.run_id,
                status: inst.status,
                active: is_active,
            }
        })
        .collect()
}

pub(super) fn company_instance_dir(instance_id: &str) -> Result<PathBuf, String> {
    company_instances_root()
        .map(|root| root.join(crate::company_template::plan::instance_key(instance_id)))
        .ok_or_else(|| "could not resolve Company instance data directory".to_string())
}

pub(super) fn owner_x0xd_endpoint() -> Result<String, String> {
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

/// The persisted Company instances root: `<data_dir>/x0x-company-ttt`.
fn company_instances_root() -> Option<PathBuf> {
    #[cfg(test)]
    if let Some(injected) = test_injection::current_root() {
        return Some(injected);
    }
    dirs::data_dir().map(|d| d.join("x0x-company-ttt"))
}

/// Epoch-millis suffix parsed from an instance id (`{slug}-{epoch_ms}`), used
/// to order instances newest-first. `None` if the trailing segment is not a
/// number.
fn instance_epoch(instance_id: &str) -> Option<u128> {
    instance_id
        .rsplit_once('-')
        .and_then(|(_, tail)| tail.parse::<u128>().ok())
}

/// One persisted Company instance discovered on disk.
pub(super) struct ScannedInstance {
    pub(super) instance_id: String,
    pub(super) template_id: String,
    pub(super) status: ManifestStatus,
    pub(super) run_id: Option<String>,
    pub(super) manifest_path: PathBuf,
    pub(super) epoch: Option<u128>,
}

/// Scan every persisted instance manifest. Unreadable manifests are skipped
/// (never panic); order is unspecified — callers sort as needed.
pub(super) fn scan_company_instances() -> Vec<ScannedInstance> {
    use crate::company_template::instantiate::{JsonManifestSink, ManifestSink};
    let Some(root) = company_instances_root() else {
        return Vec::new();
    };
    let entries = match std::fs::read_dir(&root) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let manifest_path = entry.path().join("manifest.json");
        if !manifest_path.is_file() {
            continue;
        }
        let sink = JsonManifestSink::new(&manifest_path);
        let Ok(manifest) = sink.load() else {
            continue;
        };
        let epoch = instance_epoch(&manifest.instance_id);
        out.push(ScannedInstance {
            instance_id: manifest.instance_id,
            template_id: manifest.template_id,
            status: manifest.status,
            run_id: manifest.run_id,
            manifest_path,
            epoch,
        });
    }
    out
}

/// The newest non-cancelled instance on disk, if any. Used by the
/// single-active-company gate: a second company is refused while one exists.
pub(super) fn existing_active_instance() -> Option<String> {
    let mut candidates: Vec<ScannedInstance> = scan_company_instances()
        .into_iter()
        .filter(|s| s.status != ManifestStatus::Cancelled)
        .collect();
    candidates.sort_by(|a, b| {
        b.epoch
            .cmp(&a.epoch)
            .then_with(|| b.instance_id.cmp(&a.instance_id))
    });
    candidates.into_iter().next().map(|s| s.instance_id)
}

/// Record `instance_id` as the single bound active company (poison-safe).
pub(super) fn set_active_company_instance(state: &AppState, instance_id: &str) {
    match state.active_company_instance.lock() {
        Ok(mut guard) => *guard = Some(instance_id.to_string()),
        Err(poisoned) => *poisoned.into_inner() = Some(instance_id.to_string()),
    }
}

/// Clear the active-company binding iff it currently points at `instance_id`
/// (poison-safe). Used when a cancel wins the post-bind race: undo only our own
/// binding, never clobber a different instance the reconciler may have just set.
pub(super) fn clear_active_company_instance_if_matches(state: &AppState, instance_id: &str) {
    let mut guard = state
        .active_company_instance
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if guard.as_deref() == Some(instance_id) {
        *guard = None;
    }
}

/// Boot reconciliation of persisted Company instances.
///
/// Deterministic and best-effort (errors are recorded, never fatal to launch):
/// 1. Advance every non-cancelled instance whose lifecycle is NOT yet durably
///    complete (provisioning + identities + membership + Symphony bind + run_id)
///    through [`run_company_lifecycle`] — completed phases skip/adopt
///    idempotently. A `Complete` manifest missing post-phases/run_id is resumed
///    too; cancelled instances are skipped.
/// 2. Rebind the supervised Symphony daemon to the single newest lifecycle-
///    complete instance (regenerating its deterministic `WORKFLOW.md`
///    idempotently first) and mark it the active company — covers the case where
///    an instance was already complete on disk and step 1 skipped it.
///
/// At most one Company is bound, preserving the single-active-company invariant
/// across restarts. Serialized under `company_run_lock` like instantiate/resume.
pub async fn reconcile_companies(app: &tauri::AppHandle) {
    use crate::company_template::instantiate::{InstanceManifest, JsonManifestSink, ManifestSink};
    use tauri::Manager;

    let state = app.state::<AppState>();

    // Serialize against any concurrent instantiate/resume.
    let _run_guard = state.company_run_lock.lock().await;

    // 1. Advance every non-cancelled, not-yet-complete instance.
    for inst in scan_company_instances() {
        if inst.status == ManifestStatus::Cancelled {
            continue;
        }
        // Skip instances whose lifecycle is already durably complete — step 2
        // rebinds the single newest of these.
        if let Ok(manifest) = JsonManifestSink::new(&inst.manifest_path).load() {
            if lifecycle_is_complete(&manifest) {
                continue;
            }
        }
        let Some(template) =
            crate::company_template::registry::get_company_template(&inst.template_id)
        else {
            eprintln!(
                "company-reconcile: instance `{}` references unknown template `{}`; skipping",
                inst.instance_id, inst.template_id
            );
            continue;
        };
        let display_name = match JsonManifestSink::new(&inst.manifest_path).load() {
            Ok(InstanceManifest { display_name, .. }) => {
                display_name.unwrap_or_else(|| template.name.clone())
            }
            Err(error) => {
                eprintln!(
                    "company-reconcile: cannot load manifest for `{}`: {error}",
                    inst.instance_id
                );
                continue;
            }
        };
        match run_company_lifecycle(app, &state, &template, &inst.instance_id, &display_name).await
        {
            Ok(result) => {
                if result.status != ManifestStatus::Complete {
                    eprintln!(
                        "company-reconcile: instance `{}` still incomplete after resume: {:?}",
                        inst.instance_id, result.status
                    );
                }
            }
            Err(error) => {
                eprintln!(
                    "company-reconcile: instance `{}` resume failed: {error}",
                    inst.instance_id
                );
            }
        }
    }

    // 2. Rebind the single newest lifecycle-complete instance (covers instances
    // that were already complete on disk and skipped by step 1).
    let mut complete: Vec<ScannedInstance> = scan_company_instances()
        .into_iter()
        .filter(|s| {
            JsonManifestSink::new(&s.manifest_path)
                .load()
                .is_ok_and(|m| lifecycle_is_complete(&m))
        })
        .collect();
    complete.sort_by(|a, b| {
        b.epoch
            .cmp(&a.epoch)
            .then_with(|| b.instance_id.cmp(&a.instance_id))
    });
    let Some(target) = complete.into_iter().next() else {
        return;
    };

    // Regenerate the deterministic WORKFLOW.md idempotently and (re)bind.
    let workflow = match owner_x0xd_endpoint() {
        Ok(url) => {
            match crate::company_template::registry::get_company_template(&target.template_id) {
                Some(tpl) => {
                    crate::company_template::symphony_config::generate_symphony_config_for_x0xd(
                        &tpl,
                        &target.instance_id,
                        &url,
                    )
                }
                None => {
                    eprintln!(
                    "company-reconcile: cannot regenerate WORKFLOW.md for `{}`; unknown template `{}`",
                    target.instance_id, target.template_id
                );
                    return;
                }
            }
        }
        Err(error) => {
            eprintln!(
                "company-reconcile: owner x0xd not ready, cannot regenerate WORKFLOW.md for `{}`: {error}",
                target.instance_id
            );
            return;
        }
    };

    let instance_dir = match company_instance_dir(&target.instance_id) {
        Ok(dir) => dir,
        Err(error) => {
            eprintln!("company-reconcile: cannot resolve instance dir: {error}");
            return;
        }
    };
    let workflow_path = instance_dir.join("WORKFLOW.md");
    if let Err(error) = write_private_atomic(&workflow_path, workflow.as_bytes()) {
        eprintln!("company-reconcile: failed to write WORKFLOW.md: {error}");
        return;
    }

    let app_handle = app.clone();
    let start_path = workflow_path.clone();
    let _ = tauri::async_runtime::spawn_blocking(move || {
        crate::symphony::bring_up_symphony(&app_handle, &start_path);
    })
    .await;

    let bound = state
        .local_symphony
        .lock()
        .map(|guard| guard.is_some())
        .unwrap_or(false);
    if bound {
        set_active_company_instance(&state, &target.instance_id);
        // Post-bind reload: cancel is a sync command that does NOT hold the run
        // lock, so it can land during this rebind. The target was lifecycle-
        // complete when selected, but re-read its manifest and undo the binding
        // if a tombstone landed — never leave a cancelled instance bound active.
        let cancelled = JsonManifestSink::new(&target.manifest_path)
            .load()
            .map(|m| m.status == crate::company_template::instantiate::ManifestStatus::Cancelled)
            .unwrap_or(false);
        if cancelled {
            clear_active_company_instance_if_matches(&state, &target.instance_id);
            eprintln!(
                "company-reconcile: cancelled during rebind; unbound `{}`",
                target.instance_id
            );
        } else {
            eprintln!(
                "company-reconcile: rebound active company `{}`",
                target.instance_id
            );
        }
    } else {
        eprintln!(
            "company-reconcile: symphony bind failed for `{}` (see symphony_supervision_status.error)",
            target.instance_id
        );
    }
}
