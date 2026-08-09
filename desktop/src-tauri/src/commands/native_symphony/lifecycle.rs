use crate::app_state::AppState;
use crate::company_template::instantiate::ManifestStatus;
use crate::symphony_client::SymphonyIssueDraft;

use super::company::{
    CompanyInstantiatedAgent, CompanyInstantiatedGroup, CompanyInstantiationError,
    CompanyInstantiationResult,
};
use super::fs_util::write_private_atomic;
use super::instances::{
    clear_active_company_instance_if_matches, company_instance_dir, owner_x0xd_endpoint,
    set_active_company_instance,
};
use super::proxy::build_client;

/// Assemble a `CompanyInstantiationResult` from its parts.
fn company_result(
    instance_id: &str,
    status: ManifestStatus,
    groups: Vec<CompanyInstantiatedGroup>,
    agents: Vec<CompanyInstantiatedAgent>,
    workflow_md: Option<String>,
    run_id: Option<String>,
    errors: Vec<CompanyInstantiationError>,
) -> CompanyInstantiationResult {
    CompanyInstantiationResult {
        instance_id: instance_id.to_string(),
        run_id,
        groups,
        agents,
        workflow_md,
        errors,
        status,
    }
}

/// Whether the full Company lifecycle is durably complete: provisioning plus
/// identities, public membership, Symphony bind, and a durable run_id. A
/// manifest missing any of these is honestly incomplete and resumable.
pub(super) fn lifecycle_is_complete(
    manifest: &crate::company_template::instantiate::InstanceManifest,
) -> bool {
    use crate::company_template::instantiate::ManifestStatus;
    manifest.status == ManifestStatus::Complete
        && manifest.run_id.is_some()
        && manifest.has_post_phase(&manifest.instance_id, "identities")
        && manifest.has_post_phase(&manifest.instance_id, "membership")
        && manifest.has_post_phase(&manifest.instance_id, "symphony_bind")
}

/// Re-read durable state; true iff a cancel tombstone landed for `instance_id`
/// since the run observed `observed_generation`. The save-time guard in the
/// sink is the hard backstop; this check avoids needless work after a cancel.
pub(super) fn company_run_cancelled(
    sink: &crate::company_template::instantiate::JsonManifestSink,
    instance_id: &str,
    observed_generation: u64,
) -> bool {
    use crate::company_template::instantiate::{ManifestSink, ManifestStatus};
    matches!(
        sink.load(),
        Ok(m) if m.instance_id == instance_id
            && (m.status == ManifestStatus::Cancelled
                || m.cancel_generation > observed_generation)
    )
}

/// Downgrade the on-disk manifest to `Resumable` (cancel-aware: never overwrites
/// a `Cancelled` tombstone — the sink guard refuses that, and we re-check here).
pub(super) fn mark_company_resumable(
    errors: &mut Vec<CompanyInstantiationError>,
    sink: &crate::company_template::instantiate::JsonManifestSink,
) {
    use crate::company_template::instantiate::{ManifestError, ManifestSink, ManifestStatus};
    let Ok(mut manifest) = sink.load() else {
        return;
    };
    if manifest.status == ManifestStatus::Cancelled {
        return;
    }
    manifest.status = ManifestStatus::Resumable;
    if let Err(err) = sink.save(&manifest) {
        // Surface the durable-store failure rather than silently dropping it;
        // a cancel-guard refusal is expected and stays silent.
        if err != ManifestError::Cancelled {
            errors.push(CompanyInstantiationError {
                kind: "provisioning".to_string(),
                r#ref: "manifest".to_string(),
                message: format!("failed to persist resumable state: {err}"),
            });
        }
    }
}

/// Checkpoint a completed post-provisioning phase (`identities` / `membership` /
/// `symphony_bind`) into the durable manifest. Returns `Ok(true)` if persisted,
/// `Ok(false)` if a cancel landed (the tombstone must not be overwritten), or
/// `Err` with a ready-to-surface provisioning error on a durable-store failure.
fn persist_post_phase(
    sink: &crate::company_template::instantiate::JsonManifestSink,
    instance_id: &str,
    kind: &str,
    resource_id: &str,
) -> Result<bool, CompanyInstantiationError> {
    use crate::company_template::instantiate::{
        post_phase_step, ManifestError, ManifestSink, ManifestStatus,
    };
    let mut manifest = sink.load().map_err(|error| CompanyInstantiationError {
        kind: "provisioning".to_string(),
        r#ref: "manifest".to_string(),
        message: format!("failed to load manifest: {error}"),
    })?;
    if manifest.status == ManifestStatus::Cancelled {
        return Ok(false);
    }
    let step = post_phase_step(instance_id, kind, resource_id);
    if manifest.completed_key(&step.key).is_none() {
        manifest.completed.push(step);
    }
    match sink.save(&manifest) {
        Ok(()) => Ok(true),
        Err(ManifestError::Cancelled) => Ok(false),
        Err(error) => Err(CompanyInstantiationError {
            kind: "provisioning".to_string(),
            r#ref: "manifest".to_string(),
            message: format!("manifest checkpoint failed: {error}"),
        }),
    }
}

/// Release the per-role agent children and the supervised Symphony daemon for an
/// instance whose run was cancelled or hit a terminal checkpoint failure. Best
/// effort; attached warm daemons are left running by the shutdown paths.
fn cleanup_company_runtime(app: &tauri::AppHandle, instance_id: &str) {
    crate::managed_agents::agent_identity::shutdown_company_agent_identities(instance_id);
    crate::symphony::shutdown_symphony_owned(app);
}

/// The shared, idempotent, cooperative Company lifecycle — used by instantiate,
/// resume, and boot reconcile. Walks the deterministic plan (provisioning skips
/// every completed step), then advances the post-phases (identities → public
/// membership → Symphony bind → run issue), checkpointing each into the durable
/// manifest. Cancellation is re-checked before every phase and every save; a
/// cancel during any phase is terminal and owns no processes. The instance
/// becomes the active company only once the lifecycle is genuinely complete.
pub(super) async fn run_company_lifecycle(
    app: &tauri::AppHandle,
    state: &AppState,
    template: &crate::company_template::spec::CompanyTemplate,
    instance_id: &str,
    display_name: &str,
) -> Result<CompanyInstantiationResult, String> {
    use crate::company_template::instantiate::{
        instantiate_company, post_phase_key, post_phase_step, InstanceManifest, JsonManifestSink,
        ManifestError, ManifestSink, ManifestStatus,
    };
    use crate::company_template::parse::validate_supported_contract;
    use crate::company_template::plan::plan_instantiation;
    use crate::company_template::provisioner::{
        CompanyProvisioner, EnsureMemberRequest, X0xHttpProvisioner,
    };
    use crate::company_template::spec::GroupVisibility;
    use crate::company_template::symphony_config::generate_symphony_config_for_x0xd;

    // Re-check the supported-runtime contract on every advance — a template
    // that no longer satisfies it cannot be provisioned or resumed.
    validate_supported_contract(template).map_err(|e| e.to_string())?;

    let instance_dir = company_instance_dir(instance_id)?;
    let sink = JsonManifestSink::new(instance_dir.join("manifest.json"));
    let plan = plan_instantiation(template, instance_id);
    let provisioner = X0xHttpProvisioner::new(&state.x0x_client);

    // Cancellation generation observed at the start of the run.
    let observed_generation = sink
        .load()
        .ok()
        .filter(|m: &InstanceManifest| m.instance_id == instance_id)
        .map(|m| m.cancel_generation)
        .unwrap_or(0);

    let mut errors: Vec<CompanyInstantiationError> = Vec::new();

    // ── Phase 1: provisioning (groups / stores / task lists / config / md) ──
    let outcome = instantiate_company(&plan, &provisioner, &sink).await;
    for (kind, target, message) in outcome.errors() {
        errors.push(CompanyInstantiationError {
            kind: kind.to_string(),
            r#ref: target.unwrap_or_default().to_string(),
            message: message.to_string(),
        });
    }
    if let Some(checkpoint_error) = outcome.manifest_error.clone() {
        errors.push(CompanyInstantiationError {
            kind: "provisioning".to_string(),
            r#ref: "manifest".to_string(),
            message: checkpoint_error,
        });
    }
    let groups = realized_groups(template, &outcome.steps);

    if outcome.status == ManifestStatus::Cancelled {
        cleanup_company_runtime(app, instance_id);
        let run_id = sink.load().ok().and_then(|m| m.run_id);
        return Ok(company_result(
            instance_id,
            ManifestStatus::Cancelled,
            groups,
            Vec::new(),
            None,
            run_id,
            errors,
        ));
    }
    if outcome.status != ManifestStatus::Complete {
        // Resumable provisioning — partial state is durable; no processes to own.
        return Ok(company_result(
            instance_id,
            outcome.status,
            groups,
            Vec::new(),
            None,
            None,
            errors,
        ));
    }

    // ── Phase 2: managed-agent identities ──
    if company_run_cancelled(&sink, instance_id, observed_generation) {
        cleanup_company_runtime(app, instance_id);
        return Ok(company_result(
            instance_id,
            ManifestStatus::Cancelled,
            groups,
            Vec::new(),
            None,
            None,
            errors,
        ));
    }
    let identities = match crate::managed_agents::agent_identity::provision_company_agent_identities(
        instance_id,
        &template.roles,
    ) {
        Ok(ids) => ids,
        Err(error) => {
            // Partial children are already reaped inside the provisioner.
            errors.push(CompanyInstantiationError {
                kind: "agent".to_string(),
                r#ref: "managed-agent-identities".to_string(),
                message: error.to_string(),
            });
            mark_company_resumable(&mut errors, &sink);
            let status = if company_run_cancelled(&sink, instance_id, observed_generation) {
                cleanup_company_runtime(app, instance_id);
                ManifestStatus::Cancelled
            } else {
                ManifestStatus::Resumable
            };
            return Ok(company_result(
                instance_id,
                status,
                groups,
                Vec::new(),
                None,
                None,
                errors,
            ));
        }
    };
    match persist_post_phase(&sink, instance_id, "identities", "identities") {
        Ok(true) => {}
        Ok(false) => {
            cleanup_company_runtime(app, instance_id);
            return Ok(company_result(
                instance_id,
                ManifestStatus::Cancelled,
                groups,
                Vec::new(),
                None,
                None,
                errors,
            ));
        }
        Err(error) => {
            errors.push(error);
            mark_company_resumable(&mut errors, &sink);
            return Ok(company_result(
                instance_id,
                ManifestStatus::Resumable,
                groups,
                Vec::new(),
                None,
                None,
                errors,
            ));
        }
    }

    // Realize the agent roster for the result + membership.
    let group_id_map = group_id_map_from(&outcome.steps);
    let agents: Vec<CompanyInstantiatedAgent> = identities
        .iter()
        .map(|identity| {
            let realized_group_id = template
                .role(&identity.role)
                .and_then(|role| role.groups.first())
                .and_then(|local| group_id_map.get(local))
                .cloned()
                .unwrap_or_default();
            CompanyInstantiatedAgent {
                agent_id: identity.agent_id.clone(),
                role: identity.role.clone(),
                group_id: realized_group_id,
            }
        })
        .collect();

    // ── Phase 3: public group membership ──
    let manifest = sink
        .load()
        .map_err(|e| format!("failed to load manifest: {e}"))?;
    if !manifest.has_post_phase(instance_id, "membership") {
        if company_run_cancelled(&sink, instance_id, observed_generation) {
            cleanup_company_runtime(app, instance_id);
            return Ok(company_result(
                instance_id,
                ManifestStatus::Cancelled,
                groups,
                agents.clone(),
                None,
                None,
                errors,
            ));
        }
        let mut membership_failed = false;
        for identity in &identities {
            let Some(role_spec) = template.role(&identity.role) else {
                continue;
            };
            for local_group in &role_spec.groups {
                let Some(group_id) = group_id_map.get(local_group) else {
                    continue;
                };
                // Public groups only: private-secure member-add would require a
                // TreeKEM key package (crypto, out of scope). Fail-closed skip.
                let is_public = template.group(local_group).map(|g| g.visibility)
                    == Some(GroupVisibility::PublicOpen);
                if !is_public {
                    continue;
                }
                if let Err(error) = provisioner
                    .ensure_group_member(EnsureMemberRequest {
                        group_id: group_id.clone(),
                        agent_id: identity.agent_id.clone(),
                        display_name: Some(role_spec.name.clone()),
                    })
                    .await
                {
                    errors.push(CompanyInstantiationError {
                        kind: "membership".to_string(),
                        r#ref: format!("{}→{}", identity.role, local_group),
                        message: error.to_string(),
                    });
                    membership_failed = true;
                }
            }
        }
        if membership_failed {
            mark_company_resumable(&mut errors, &sink);
            let status = if company_run_cancelled(&sink, instance_id, observed_generation) {
                cleanup_company_runtime(app, instance_id);
                ManifestStatus::Cancelled
            } else {
                ManifestStatus::Resumable
            };
            return Ok(company_result(
                instance_id,
                status,
                groups,
                agents.clone(),
                None,
                None,
                errors,
            ));
        }
        match persist_post_phase(&sink, instance_id, "membership", "membership") {
            Ok(true) => {}
            Ok(false) => {
                cleanup_company_runtime(app, instance_id);
                return Ok(company_result(
                    instance_id,
                    ManifestStatus::Cancelled,
                    groups,
                    agents.clone(),
                    None,
                    None,
                    errors,
                ));
            }
            Err(error) => {
                errors.push(error);
                mark_company_resumable(&mut errors, &sink);
                return Ok(company_result(
                    instance_id,
                    ManifestStatus::Resumable,
                    groups,
                    agents.clone(),
                    None,
                    None,
                    errors,
                ));
            }
        }
    }

    // ── Phase 4: deterministic WORKFLOW.md → supervised Symphony bind ──
    // Always (re)write the deterministic WORKFLOW and (re)bind the daemon: the
    // bind is idempotent (no-op against the same config, rebind otherwise), and
    // a restart that finds symphony_bind already checkpointed still needs the
    // daemon up for the run-issue phase. The checkpoint records that the bind
    // succeeded at least once.
    let workflow_md = {
        if company_run_cancelled(&sink, instance_id, observed_generation) {
            cleanup_company_runtime(app, instance_id);
            return Ok(company_result(
                instance_id,
                ManifestStatus::Cancelled,
                groups,
                agents.clone(),
                None,
                None,
                errors,
            ));
        }
        let x0xd_url = owner_x0xd_endpoint()?;
        let workflow = generate_symphony_config_for_x0xd(template, instance_id, &x0xd_url);
        let workflow_path = instance_dir.join("WORKFLOW.md");
        write_private_atomic(&workflow_path, workflow.as_bytes())?;
        let app_handle = app.clone();
        let start_path = workflow_path.clone();
        tauri::async_runtime::spawn_blocking(move || {
            crate::symphony::bring_up_symphony(&app_handle, &start_path);
        })
        .await
        .map_err(|error| format!("Company symphony start task failed: {error}"))?;
        let symphony_ready = state
            .local_symphony
            .lock()
            .map(|guard| guard.is_some())
            .unwrap_or(false);
        if !symphony_ready {
            errors.push(CompanyInstantiationError {
                kind: "daemon".to_string(),
                r#ref: "start".to_string(),
                message: "symphony daemon did not become ready".to_string(),
            });
            mark_company_resumable(&mut errors, &sink);
            let status = if company_run_cancelled(&sink, instance_id, observed_generation) {
                cleanup_company_runtime(app, instance_id);
                ManifestStatus::Cancelled
            } else {
                ManifestStatus::Resumable
            };
            return Ok(company_result(
                instance_id,
                status,
                groups,
                agents.clone(),
                Some(workflow),
                None,
                errors,
            ));
        }
        match persist_post_phase(&sink, instance_id, "symphony_bind", "symphony_bind") {
            Ok(true) => {}
            Ok(false) => {
                cleanup_company_runtime(app, instance_id);
                return Ok(company_result(
                    instance_id,
                    ManifestStatus::Cancelled,
                    groups,
                    agents.clone(),
                    None,
                    None,
                    errors,
                ));
            }
            Err(error) => {
                errors.push(error);
                mark_company_resumable(&mut errors, &sink);
                return Ok(company_result(
                    instance_id,
                    ManifestStatus::Resumable,
                    groups,
                    agents.clone(),
                    None,
                    None,
                    errors,
                ));
            }
        }
        workflow
    };

    // ── Phase 5: Symphony run issue (durable run_id) ──
    let mut manifest = sink
        .load()
        .map_err(|e| format!("failed to load manifest: {e}"))?;
    if manifest.run_id.is_none() {
        if company_run_cancelled(&sink, instance_id, observed_generation) {
            cleanup_company_runtime(app, instance_id);
            return Ok(company_result(
                instance_id,
                ManifestStatus::Cancelled,
                groups,
                agents.clone(),
                Some(workflow_md.clone()),
                None,
                errors,
            ));
        }
        let run_id = match build_client(state) {
            Ok(client) => {
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
            }
            Err(error) => {
                errors.push(CompanyInstantiationError {
                    kind: "daemon".to_string(),
                    r#ref: "client".to_string(),
                    message: error,
                });
                None
            }
        };
        let Some(run_id) = run_id else {
            mark_company_resumable(&mut errors, &sink);
            let status = if company_run_cancelled(&sink, instance_id, observed_generation) {
                cleanup_company_runtime(app, instance_id);
                ManifestStatus::Cancelled
            } else {
                ManifestStatus::Resumable
            };
            return Ok(company_result(
                instance_id,
                status,
                groups,
                agents.clone(),
                Some(workflow_md.clone()),
                None,
                errors,
            ));
        };
        // Persist run_id + the run_issue checkpoint atomically — Complete is
        // never claimed before the run_id is durable.
        manifest.run_id = Some(run_id);
        let run_issue_key = post_phase_key(instance_id, "run_issue");
        if manifest.completed_key(&run_issue_key).is_none() {
            manifest.completed.push(post_phase_step(
                instance_id,
                "run_issue",
                manifest.run_id.as_deref().unwrap_or("run_issue"),
            ));
        }
        match sink.save(&manifest) {
            Ok(()) => {}
            Err(ManifestError::Cancelled) => {
                cleanup_company_runtime(app, instance_id);
                return Ok(company_result(
                    instance_id,
                    ManifestStatus::Cancelled,
                    groups,
                    agents.clone(),
                    Some(workflow_md.clone()),
                    None,
                    errors,
                ));
            }
            Err(error) => {
                errors.push(CompanyInstantiationError {
                    kind: "provisioning".to_string(),
                    r#ref: "manifest".to_string(),
                    message: format!("failed to persist run id: {error}"),
                });
                mark_company_resumable(&mut errors, &sink);
                return Ok(company_result(
                    instance_id,
                    ManifestStatus::Resumable,
                    groups,
                    agents.clone(),
                    Some(workflow_md.clone()),
                    None,
                    errors,
                ));
            }
        }
    }

    // ── Complete: all required phases succeeded and run_id is durable ──
    // The provisioning executor persists InProgress (never Complete); this is
    // the SINGLE point that marks the lifecycle Complete — atomically and
    // cancel-aware — only after every post-phase + a durable run_id.
    if company_run_cancelled(&sink, instance_id, observed_generation) {
        cleanup_company_runtime(app, instance_id);
        return Ok(company_result(
            instance_id,
            ManifestStatus::Cancelled,
            groups,
            agents.clone(),
            Some(workflow_md.clone()),
            None,
            errors,
        ));
    }
    let mut manifest = sink
        .load()
        .map_err(|e| format!("failed to load manifest for completion: {e}"))?;
    if manifest.status == ManifestStatus::Cancelled {
        cleanup_company_runtime(app, instance_id);
        return Ok(company_result(
            instance_id,
            ManifestStatus::Cancelled,
            groups,
            agents.clone(),
            Some(workflow_md.clone()),
            None,
            errors,
        ));
    }
    manifest.status = ManifestStatus::Complete;
    match sink.save(&manifest) {
        Ok(()) => {}
        Err(ManifestError::Cancelled) => {
            cleanup_company_runtime(app, instance_id);
            return Ok(company_result(
                instance_id,
                ManifestStatus::Cancelled,
                groups,
                agents.clone(),
                Some(workflow_md.clone()),
                None,
                errors,
            ));
        }
        Err(err) => {
            errors.push(CompanyInstantiationError {
                kind: "provisioning".to_string(),
                r#ref: "manifest".to_string(),
                message: format!("failed to persist completion: {err}"),
            });
            mark_company_resumable(&mut errors, &sink);
            return Ok(company_result(
                instance_id,
                ManifestStatus::Resumable,
                groups,
                agents.clone(),
                Some(workflow_md.clone()),
                None,
                errors,
            ));
        }
    }

    // The active binding is recorded only once the lifecycle is genuinely
    // complete: provisioning + identities + membership + Symphony bind + a
    // durable run_id, all now durably marked Complete.
    set_active_company_instance(state, instance_id);

    // Post-bind reload: cancel is a sync command that does NOT hold the run
    // lock, so it can write a tombstone (and clear the active binding itself)
    // concurrently with this bind. Re-read durable state AFTER the bind; if a
    // tombstone landed, undo the binding we just set and return Cancelled.
    // Complete is never returned unless the post-bind reload is non-cancelled
    // lifecycle-complete.
    if company_run_cancelled(&sink, instance_id, observed_generation) {
        clear_active_company_instance_if_matches(state, instance_id);
        cleanup_company_runtime(app, instance_id);
        let run_id = manifest.run_id.clone();
        return Ok(company_result(
            instance_id,
            ManifestStatus::Cancelled,
            groups,
            agents.clone(),
            Some(workflow_md.clone()),
            run_id,
            errors,
        ));
    }

    let run_id = manifest.run_id.clone();
    Ok(company_result(
        instance_id,
        ManifestStatus::Complete,
        groups,
        agents,
        Some(workflow_md),
        run_id,
        errors,
    ))
}

/// `local_id → realized x0xd id` map for group steps (a role staffs its first
/// realized group).
fn group_id_map_from(
    steps: &[crate::company_template::instantiate::StepOutcome],
) -> std::collections::HashMap<String, String> {
    steps
        .iter()
        .filter_map(|step| {
            if step.kind == "group" {
                step.local_id
                    .as_ref()
                    .zip(step.resource_id.as_ref())
                    .map(|(local, realized)| (local.clone(), realized.clone()))
            } else {
                None
            }
        })
        .collect()
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
                kind: super::company::company_group_kind(&group.id),
            })
        })
        .collect()
}
