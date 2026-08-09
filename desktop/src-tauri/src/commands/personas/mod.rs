use tauri::AppHandle;
use uuid::Uuid;

use crate::{
    app_state::AppState,
    managed_agents::{
        apply_persona_behavior, current_instance_id, effective_agent_command, load_managed_agents,
        load_personas, load_teams, managed_agent_avatar_url, save_managed_agents, save_personas,
        stop_managed_agent_process, sync_managed_agent_processes, try_regenerate_nest,
        validate_persona_activation_change, validate_persona_deletion, AgentDefinition,
        CreatePersonaRequest, ManagedAgentRecord, UpdatePersonaRequest,
    },
    util::now_iso,
};

fn trim_required(value: &str, label: &str) -> Result<String, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(format!("{label} is required"));
    }
    Ok(trimmed.to_string())
}

fn trim_optional(value: Option<String>) -> Option<String> {
    value.and_then(|candidate| {
        let trimmed = candidate.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    })
}

#[tauri::command]
pub async fn list_personas(app: AppHandle) -> Result<Vec<AgentDefinition>, String> {
    use tauri::Manager;
    tokio::task::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let _store_guard = state
            .managed_agents_store_lock
            .lock()
            .map_err(|error| error.to_string())?;
        load_personas(&app)
    })
    .await
    .map_err(|e| format!("spawn_blocking failed: {e}"))?
}

#[tauri::command]
pub async fn create_persona(
    input: CreatePersonaRequest,
    app: AppHandle,
) -> Result<AgentDefinition, String> {
    use tauri::Manager;
    tokio::task::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let display_name = trim_required(&input.display_name, "Display name")?;
        // System prompt optional: core memory is auto-injected. Empty is valid.
        let system_prompt = input.system_prompt.trim().to_string();
        let avatar_url = trim_optional(input.avatar_url);
        let runtime = trim_optional(input.runtime);
        let model = trim_optional(input.model);
        let provider = trim_optional(input.provider);
        let now = now_iso();
        let _store_guard = state
            .managed_agents_store_lock
            .lock()
            .map_err(|error| error.to_string())?;
        let mut personas = load_personas(&app)?;
        let name_pool: Vec<String> = input
            .name_pool
            .into_iter()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        crate::managed_agents::validate_user_env_keys(&input.env_vars)?;
        let mut persona = AgentDefinition {
            id: Uuid::new_v4().to_string(),
            display_name,
            avatar_url,
            system_prompt,
            runtime,
            model,
            provider,
            name_pool,
            is_builtin: false,
            is_active: true,
            source_team: None,
            source_team_persona_slug: None,
            env_vars: input.env_vars,
            respond_to: None,
            respond_to_allowlist: Vec::new(),
            parallelism: None,
            created_at: now.clone(),
            updated_at: now,
        };
        apply_persona_behavior(&mut persona, input.behavior)?;
        personas.push(persona.clone());
        save_personas(&app, &personas)?;
        try_regenerate_nest(&app);
        Ok(persona)
    })
    .await
    .map_err(|e| format!("spawn_blocking failed: {e}"))?
}

/// Return value of the `update_persona` command. Uses flatten so all
/// `AgentDefinition` fields appear at the top level of the JSON response —
/// backward-compatible with callers that already destructure a raw persona object.
#[derive(Debug, serde::Serialize)]
pub struct UpdatePersonaResult {
    #[serde(flatten)]
    persona: AgentDefinition,
}

/// Propagate a persona definition's display_name rename to linked agent instances.
/// Only instances whose current `name` equals `old_display_name` are updated;
/// pool-named instances (e.g. "Birch", "Compass") keep their individualised name.
/// Updates both `record.name` (relay display name) and `record.display_name`.
/// Returns the pubkeys of the records that were renamed.
fn propagate_persona_name_rename(
    records: &mut [ManagedAgentRecord],
    persona_id: &str,
    old_display_name: &str,
    new_display_name: &str,
) -> Vec<String> {
    let mut renamed = Vec::new();
    for record in records.iter_mut() {
        if record.persona_id.as_deref() != Some(persona_id) {
            continue;
        }
        if record.name != old_display_name {
            continue; // pool-named instance — keep its individualised name
        }
        record.name = new_display_name.to_string();
        record.display_name = Some(new_display_name.to_string());
        renamed.push(record.pubkey.clone());
    }
    renamed
}

#[tauri::command]
pub async fn update_persona(
    input: UpdatePersonaRequest,
    app: AppHandle,
) -> Result<UpdatePersonaResult, String> {
    use tauri::Manager;

    // Phase 1: synchronous save (persona record + linked agent record updates).
    let result = tokio::task::spawn_blocking({
        let app = app.clone();
        move || -> Result<AgentDefinition, String> {
            let state = app.state::<AppState>();
            let display_name = trim_required(&input.display_name, "Display name")?;
            let system_prompt = input.system_prompt.clone();
            let avatar_url = trim_optional(input.avatar_url);
            let runtime = trim_optional(input.runtime);
            let model = trim_optional(input.model);
            let provider = trim_optional(input.provider);

            let _store_guard = state
                .managed_agents_store_lock
                .lock()
                .map_err(|error| error.to_string())?;
            let mut personas = load_personas(&app)?;
            let persona = personas
                .iter_mut()
                .find(|record| record.id == input.id)
                .ok_or_else(|| format!("agent {} not found", input.id))?;

            // Track what changed so we can propagate to linked agent records.
            let avatar_changed = persona.avatar_url != avatar_url;
            let name_changed = persona.display_name != display_name;
            let old_display_name = persona.display_name.clone();

            persona.display_name = display_name;
            persona.avatar_url = avatar_url;
            persona.system_prompt = system_prompt;
            persona.runtime = runtime;
            persona.model = model;
            persona.provider = provider;
            persona.name_pool = input
                .name_pool
                .into_iter()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            if let Some(env_vars) = input.env_vars {
                crate::managed_agents::validate_user_env_keys(&env_vars)?;
                persona.env_vars = env_vars;
            }
            apply_persona_behavior(persona, input.behavior)?;
            persona.updated_at = now_iso();

            let result = persona.clone();
            save_personas(&app, &personas)?;
            try_regenerate_nest(&app);

            // Propagate avatar/display_name changes to linked agent records so
            // instances stay in sync with their definition.
            if avatar_changed || name_changed {
                let mut records = load_managed_agents(&app)?;
                let renamed: Vec<String> = if name_changed {
                    propagate_persona_name_rename(
                        &mut records,
                        &result.id,
                        &old_display_name,
                        &result.display_name,
                    )
                } else {
                    Vec::new()
                };

                let mut agents_modified = false;
                for record in records.iter_mut() {
                    if record.persona_id.as_deref() != Some(&result.id) {
                        continue;
                    }
                    let mut record_changed = renamed.contains(&record.pubkey);
                    if avatar_changed {
                        let effective_cmd = effective_agent_command(
                            record.persona_id.as_deref(),
                            std::slice::from_ref(&result),
                            record.agent_command_override.as_deref(),
                        );
                        record.avatar_url = result
                            .avatar_url
                            .clone()
                            .or_else(|| managed_agent_avatar_url(&effective_cmd));
                        record_changed = true;
                    }
                    if record_changed {
                        agents_modified = true;
                    }
                }

                if agents_modified {
                    save_managed_agents(&app, &records)?;
                }
            }

            Ok(result)
        }
    })
    .await
    .map_err(|e| format!("spawn_blocking failed: {e}"))??;

    Ok(UpdatePersonaResult { persona: result })
}

#[cfg(test)]
mod delete_cascade_tests;
#[cfg(test)]
mod name_propagation_tests;

/// Return pubkeys of every managed agent whose definition is the given persona.
///
/// Pure helper used by `delete_persona` to determine which agent records to
/// cascade-delete. Extracted so the filtering logic can be unit-tested without
/// a full Tauri `AppHandle`.
fn collect_cascade_pubkeys(agents: &[ManagedAgentRecord], persona_id: &str) -> Vec<String> {
    agents
        .iter()
        .filter(|a| a.persona_id.as_deref() == Some(persona_id))
        .map(|a| a.pubkey.clone())
        .collect()
}

/// Names of cascade agents that are provider-deployed: non-local backend with
/// a live `backend_agent_id`.
///
/// Pure helper used by `delete_persona`'s pre-flight: the cascade is refused
/// while any exist, because deleting the local record would orphan the remote
/// deployment. Mirrors `delete_managed_agent`'s `force_remote_delete` guard.
fn collect_remote_deployed(
    agents: &[ManagedAgentRecord],
    cascade: &std::collections::HashSet<String>,
) -> Vec<String> {
    agents
        .iter()
        .filter(|a| {
            cascade.contains(&a.pubkey)
                && a.backend != crate::managed_agents::BackendKind::Local
                && a.backend_agent_id.is_some()
        })
        .map(|a| a.name.clone())
        .collect()
}

/// Remove cascade agents from `agents` and persist via the injectable `save`.
///
/// Extracted from `delete_persona` so unit tests can inject a failing save and
/// verify retry-safety without a full `AppHandle` mock: if `save` returns `Err`,
/// this function propagates it before the keyring deletions and tombstones that
/// appear after the `?` in the call site — nothing is destroyed and the command
/// is safe to retry.
fn commit_cascade_agents(
    agents: &mut Vec<ManagedAgentRecord>,
    cascade: &std::collections::HashSet<String>,
    save: impl FnOnce(&[ManagedAgentRecord]) -> Result<(), String>,
) -> Result<(), String> {
    agents.retain(|a| !cascade.contains(&a.pubkey));
    save(agents)
}

#[tauri::command]
pub async fn delete_persona(id: String, app: AppHandle) -> Result<(), String> {
    use tauri::Manager;
    tokio::task::spawn_blocking(move || {
        let state = app.state::<AppState>();

        {
            // Store lock held across all three phases.
            // Lock ordering: store lock (acquired here) → process lock (per-agent in Phase 2).
            let _store_guard = state
                .managed_agents_store_lock
                .lock()
                .map_err(|error| error.to_string())?;

            // Load and validate the persona before any destructive work.
            let mut personas = load_personas(&app)?;
            let persona = personas
                .iter()
                .find(|record| record.id == id)
                .ok_or_else(|| format!("persona {id} not found"))?;
            let referenced_by_team = load_teams(&app)?.iter().any(|team| {
                team.persona_ids
                    .iter()
                    .any(|persona_id| persona_id == id.as_str())
            });
            validate_persona_deletion(persona, referenced_by_team)?;
            // ── Phase 1: Stage ─────────────────────────────────────────────
            //
            // Load agents, sync process state, and build the cascade set. Lock
            // ordering: store lock (held) → process lock (acquired for sync,
            // then released before Phase 2 stops). Every fallible read/lock is
            // here; an error leaves all state intact and the command is retryable.
            let mut agents = load_managed_agents(&app)?;
            {
                let mut runtimes = state
                    .managed_agent_processes
                    .lock()
                    .map_err(|error| error.to_string())?;
                let (sync_changed, exited_pubkeys) = sync_managed_agent_processes(
                    &mut agents,
                    &mut runtimes,
                    &current_instance_id(&app),
                );
                if sync_changed {
                    save_managed_agents(&app, &agents)?;
                }
                for pk in &exited_pubkeys {
                    state.clear_agent_session_caches(pk);
                }
                // runtimes drops here (process lock released before Phase 2).
            }

            // Build the cascade set. HashSet for O(1) membership in Phase 3.
            let cascade: std::collections::HashSet<String> =
                collect_cascade_pubkeys(&agents, &id).into_iter().collect();

            // Remote-agent pre-flight: refuse the cascade before any destructive
            // work while any target is provider-deployed. Nothing in
            // create_managed_agent forbids a persona-linked provider agent, so
            // this must be a runtime guard, not an assumed invariant.
            let remote_deployed = collect_remote_deployed(&agents, &cascade);
            if !remote_deployed.is_empty() {
                return Err(format!(
                    "persona {id} has provider-deployed agent instances ({}); delete those agent instances first",
                    remote_deployed.join(", ")
                ));
            }

            // ── Phase 2: Stop ───────────────────────────────────────────────
            //
            // Best-effort stop each running cascade instance. Lock ordering:
            // store lock (held) → process lock acquired per-agent and released
            // between stops so the process lock is not held across the full poll
            // cycle (stop_managed_agent_process polls 100ms×10 before SIGKILL).
            //
            // Per-agent stop errors are swallowed — these records are deleted in
            // Phase 3 regardless. Intentional difference from delete_managed_agent
            // (single-agent, fatal on stop failure); here the cascade is multi-agent
            // and deletion must proceed even if one instance cannot be stopped.
            for pk in &cascade {
                if let Some(rec) = agents.iter_mut().find(|a| a.pubkey == *pk) {
                    let mut runtimes = state
                        .managed_agent_processes
                        .lock()
                        .map_err(|error| error.to_string())?;
                    if let Err(e) = stop_managed_agent_process(&app, rec, &mut runtimes) {
                        eprintln!("buzz-desktop: delete_persona: failed to stop agent {pk}: {e}");
                    }
                    // runtimes drops here (per-agent, process lock not held across stops).
                }
            }

            // ── Phase 3: Commit ─────────────────────────────────────────────
            //
            // Disk-authoritative writes first, side effects strictly after.
            // commit_cascade_agents is an injectable seam so unit tests can
            // verify retry-safety: a failing save propagates before any keyring
            // deletion or tombstone occurs.
            //
            // Failure semantics:
            //   agent save fails   → nothing destroyed; full cascade retries cleanly
            //   persona save fails → cascade agents gone, persona survives; a retry
            //                        finds an empty cascade and proceeds cleanly
            // Keys and tombstones are enqueued only after their records leave disk.
            if !cascade.is_empty() {
                commit_cascade_agents(&mut agents, &cascade, |recs| {
                    save_managed_agents(&app, recs)
                })?;
            }

            let original_len = personas.len();
            personas.retain(|record| record.id != id);
            if personas.len() == original_len {
                return Err(format!("persona {id} not found"));
            }
            save_personas(&app, &personas)?;

            // Side effects — strictly after records leave disk.
            for pk in &cascade {
                state.clear_agent_session_caches(pk);
            }

            // _store_guard drops here, before try_regenerate_nest.
        }

        try_regenerate_nest(&app);

        Ok(())
    })
    .await
    .map_err(|e| format!("spawn_blocking failed: {e}"))?
}

#[tauri::command]
pub async fn set_persona_active(
    id: String,
    active: bool,
    app: AppHandle,
) -> Result<AgentDefinition, String> {
    use tauri::Manager;
    tokio::task::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let _store_guard = state
            .managed_agents_store_lock
            .lock()
            .map_err(|error| error.to_string())?;
        let mut personas = load_personas(&app)?;
        let persona = personas
            .iter_mut()
            .find(|record| record.id == id)
            .ok_or_else(|| format!("agent {id} not found"))?;

        let referenced_by_managed_agent = !active
            && load_managed_agents(&app)?
                .iter()
                .any(|agent| agent.persona_id.as_deref() == Some(id.as_str()));
        let referenced_by_team = !active
            && load_teams(&app)?.iter().any(|team| {
                team.persona_ids
                    .iter()
                    .any(|persona_id| persona_id == id.as_str())
            });

        validate_persona_activation_change(
            persona,
            active,
            referenced_by_managed_agent,
            referenced_by_team,
        )?;

        if persona.is_active == active {
            return Ok(persona.clone());
        }

        persona.is_active = active;
        persona.updated_at = now_iso();

        let updated = persona.clone();
        save_personas(&app, &personas)?;
        try_regenerate_nest(&app);
        Ok(updated)
    })
    .await
    .map_err(|e| format!("spawn_blocking failed: {e}"))?
}

pub(crate) const PNG_MAGIC: [u8; 4] = [0x89, 0x50, 0x4E, 0x47];
mod snapshot;
pub use snapshot::encode_agent_snapshot_for_send;
pub use snapshot::export_agent_snapshot;
pub(crate) use snapshot::import::resolve_snapshot_import_behavior;
pub use snapshot::{confirm_agent_snapshot_import, preview_agent_snapshot_import};
