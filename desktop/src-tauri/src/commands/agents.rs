use base64::Engine as _;
use serde::Deserialize;
use std::collections::HashMap;
use tauri::{AppHandle, State};

use crate::{
    app_state::AppState,
    managed_agents::{
        build_managed_agent_summary, current_instance_id, discover_provider_candidates,
        ensure_persona_is_active, find_managed_agent_mut, load_managed_agents, load_personas,
        load_teams, managed_agent_avatar_url, normalize_agent_args, provider_deploy,
        resolve_provider_binary, save_managed_agents, start_managed_agent_process,
        stop_managed_agent_process, stop_managed_agent_workspace_pair,
        sync_managed_agent_processes, try_regenerate_nest, validate_provider_config, BackendKind,
        CreateManagedAgentRequest, CreateManagedAgentResponse, ManagedAgentMentionWakeRequest,
        ManagedAgentRuntimeStatus, ManagedAgentSummary, RespondTo, DEFAULT_ACP_COMMAND,
        DEFAULT_AGENT_PARALLELISM, DEFAULT_AGENT_TURN_TIMEOUT_SECONDS,
    },
    util::now_iso,
};

const NATIVE_ID_HEX_LEN: usize = 64;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManagedMentionEnvelope {
    #[serde(default)]
    mentions: Vec<String>,
    #[serde(default)]
    agent_generated: bool,
    #[serde(default)]
    agent_generation: Option<u8>,
    #[serde(default)]
    delegation_root: Option<String>,
}

fn canonical_hex_id(value: &str, label: &str) -> Result<String, String> {
    let normalized = value.trim().to_ascii_lowercase();
    if normalized.len() == NATIVE_ID_HEX_LEN
        && normalized.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        Ok(normalized)
    } else {
        Err(format!("{label} must be 64 hexadecimal characters"))
    }
}

fn decode_managed_mention_envelope(
    row: &crate::x0x_client::HistoryRow,
) -> Result<ManagedMentionEnvelope, String> {
    let payload = base64::engine::general_purpose::STANDARD
        .decode(row.payload.as_bytes())
        .map_err(|_| "managed mention owner row payload is not canonical base64".to_string())?;
    serde_json::from_slice(&payload)
        .map_err(|_| "managed mention owner row is not a native channel envelope".to_string())
}

fn require_verified_owner_mention_row(
    row: &crate::x0x_client::HistoryRow,
    stable_group_id: &str,
    expected_msg_id: &str,
    target_child_agent_id: &str,
    owned_child_by_agent_id: &HashMap<String, String>,
) -> Result<(String, ManagedMentionEnvelope), String> {
    if row.msg_id != expected_msg_id
        || row.scope != format!("group:{stable_group_id}")
        || row.direction != "Inbound"
        || !row.signed
        || row.provenance != "VerifiedEnvelope"
    {
        return Err(
            "managed mention must be the exact signed VerifiedEnvelope owner history row"
                .to_string(),
        );
    }
    let author = row
        .author_agent
        .as_deref()
        .ok_or_else(|| "managed mention owner row has no authenticated author".to_string())?
        .to_ascii_lowercase();
    if author == target_child_agent_id || !owned_child_by_agent_id.contains_key(&author) {
        return Err("managed mention author is not another owned managed child".to_string());
    }
    let envelope = decode_managed_mention_envelope(row)?;
    if !envelope
        .mentions
        .iter()
        .any(|mention| mention.eq_ignore_ascii_case(target_child_agent_id))
    {
        return Err("managed mention does not explicitly target the child AgentId".to_string());
    }
    Ok((author, envelope))
}

fn author_allowed_for_launch(
    respond_to: RespondTo,
    author_agent_id: &str,
    owner_agent_id: &str,
    effective_allowlist: &[String],
) -> bool {
    match respond_to {
        RespondTo::OwnerOnly => author_agent_id.eq_ignore_ascii_case(owner_agent_id),
        RespondTo::Allowlist => {
            author_agent_id.eq_ignore_ascii_case(owner_agent_id)
                || effective_allowlist
                    .iter()
                    .any(|allowed| allowed.eq_ignore_ascii_case(author_agent_id))
        }
        RespondTo::Anyone => true,
    }
}

fn require_exact_redelivery_receipt(
    receipt: &crate::x0x_client::GroupMessageRedeliveryReceipt,
    stable_group_id: &str,
    msg_id: &str,
    target_agent_id: &str,
) -> Result<(), String> {
    let durable_outcome = matches!(receipt.outcome.as_str(), "committed" | "duplicate");
    if receipt.ok
        && receipt.group_id == stable_group_id
        && receipt.msg_id == msg_id
        && receipt.agent_id == target_agent_id
        && durable_outcome
    {
        Ok(())
    } else {
        Err("x0xd did not return an exact durable managed mention redelivery receipt".into())
    }
}

fn require_bounded_agent_delegation(
    mention_row: &crate::x0x_client::HistoryRow,
    mention: &ManagedMentionEnvelope,
    root_row: &crate::x0x_client::HistoryRow,
    stable_group_id: &str,
    owner_agent_id: &str,
    delegate_agent_id: &str,
) -> Result<(), String> {
    let root_id = mention
        .delegation_root
        .as_deref()
        .ok_or_else(|| "managed child mention has no owner delegation root".to_string())?;
    if !mention.agent_generated
        || mention.agent_generation != Some(1)
        || mention_row.thread_root.as_deref() != Some(root_id)
        || mention_row.thread_parent.as_deref() != Some(root_id)
    {
        return Err("managed child mention is not a bounded first-generation delegation".into());
    }
    if root_row.msg_id != root_id
        || root_row.scope != format!("group:{stable_group_id}")
        || root_row.direction != "Outbound"
        || !root_row.signed
        || root_row.provenance != "LocalSend"
        || root_row.author_agent.as_deref() != Some(owner_agent_id)
        || root_row.thread_root.is_some()
        || root_row.thread_parent.is_some()
    {
        return Err(
            "managed child mention delegation root is not the exact signed owner row".into(),
        );
    }
    let root = decode_managed_mention_envelope(root_row)?;
    if root.agent_generated
        || !root
            .mentions
            .iter()
            .any(|mention| mention.eq_ignore_ascii_case(delegate_agent_id))
    {
        return Err("managed child mention was not authorized by its owner delegation root".into());
    }
    Ok(())
}

fn trim_to_optional_string(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn resolve_created_avatar_url(
    requested_avatar_url: Option<&str>,
    persona_avatar_url: Option<String>,
    agent_command: &str,
) -> Option<String> {
    requested_avatar_url
        .and_then(trim_to_optional_string)
        .or_else(|| {
            persona_avatar_url
                .as_deref()
                .and_then(trim_to_optional_string)
        })
        .or_else(|| managed_agent_avatar_url(agent_command))
}

/// Apply the linked persona fields that are effective for the next native
/// launch. Behavioral defaults are normally copied at mint time, but native
/// x0x ACP has an exact-one-worker contract. When a persona supplies a current
/// parallelism value, it is the recoverable UI intent for a stale materialized
/// value that native launch cannot accept.
fn apply_persona_launch_snapshot(
    record: &mut crate::managed_agents::ManagedAgentRecord,
    personas: &[crate::managed_agents::AgentDefinition],
) -> bool {
    let Some(persona_id) = record.persona_id.as_deref() else {
        return false;
    };
    let Some(persona) = personas.iter().find(|persona| persona.id == persona_id) else {
        return false;
    };

    crate::managed_agents::persona_events::apply_persona_snapshot(record, persona);
    if let Some(parallelism) = persona.parallelism {
        record.parallelism = parallelism;
    }
    record.updated_at = crate::util::now_iso();
    true
}

pub(super) async fn start_local_agent_pairs_with_preflight(
    app: &AppHandle,
    state: &AppState,
    pubkey: &str,
    group_ids: &[String],
) -> Result<ManagedAgentSummary, String> {
    let record_snapshot = {
        let _store_guard = state
            .managed_agents_store_lock
            .lock()
            .map_err(|e| e.to_string())?;
        load_managed_agents(app)?
            .into_iter()
            .find(|record| record.pubkey == pubkey)
            .ok_or_else(|| format!("agent {pubkey} not found"))?
    };
    if record_snapshot.backend != BackendKind::Local {
        return Err(format!("agent {pubkey} is not a local agent"));
    }

    {
        let _store_guard = state
            .managed_agents_store_lock
            .lock()
            .map_err(|e| e.to_string())?;
        let mut records = load_managed_agents(app)?;
        let record = find_managed_agent_mut(&mut records, pubkey)?;
        let personas = load_personas(app).unwrap_or_default();
        if apply_persona_launch_snapshot(record, &personas) {
            save_managed_agents(app, &records)?;
        }
    }

    let mut errors = Vec::new();
    for group_id in group_ids {
        if let Err(error) = crate::managed_agents::start_managed_agent_runtime_pair_lazy(
            pubkey.to_string(),
            group_id.clone(),
            app.clone(),
        )
        .await
        {
            errors.push(format!("{group_id}: {error}"));
        }
    }
    if !errors.is_empty() {
        return Err(format!(
            "failed to restart one or more managed-agent runtime pairs: {}",
            errors.join("; ")
        ));
    }

    let _store_guard = state
        .managed_agents_store_lock
        .lock()
        .map_err(|e| e.to_string())?;
    let records = load_managed_agents(app)?;
    let runtimes = state
        .managed_agent_processes
        .lock()
        .map_err(|e| e.to_string())?;
    let personas = load_personas(app).unwrap_or_default();
    let record = records
        .iter()
        .find(|record| record.pubkey == pubkey)
        .ok_or_else(|| format!("agent {pubkey} not found"))?;
    build_managed_agent_summary(app, record, &runtimes, &personas)
}

pub(super) async fn start_local_agent_with_preflight(
    app: &AppHandle,
    state: &AppState,
    pubkey: &str,
) -> Result<ManagedAgentSummary, String> {
    let (record_snapshot, personas) = {
        let _store_guard = state
            .managed_agents_store_lock
            .lock()
            .map_err(|e| e.to_string())?;
        let mut records = load_managed_agents(app)?;
        let personas = load_personas(app).unwrap_or_default();
        let record = find_managed_agent_mut(&mut records, pubkey)?;
        if record.backend != BackendKind::Local {
            return Err(format!("agent {pubkey} is not a local agent"));
        }
        if apply_persona_launch_snapshot(record, &personas) {
            save_managed_agents(app, &records)?;
        }
        let record_snapshot = records
            .iter()
            .find(|record| record.pubkey == pubkey)
            .cloned()
            .ok_or_else(|| format!("agent {pubkey} not found"))?;
        (record_snapshot, personas)
    };
    crate::managed_agents::validate_native_agent_parallelism(record_snapshot.parallelism)?;

    let group_id = state
        .active_group_id
        .lock()
        .map_err(|e| e.to_string())?
        .clone()
        .ok_or_else(|| "no native community is active".to_string())?;
    let native_launch = crate::managed_agents::prepare_managed_agent_launch(
        app,
        &record_snapshot,
        &group_id,
        crate::managed_agents::GroupBindIntent::EnsureAttached,
    )
    .await?;

    let _store_guard = state
        .managed_agents_store_lock
        .lock()
        .map_err(|e| e.to_string())?;
    let mut records = load_managed_agents(app)?;
    let mut runtimes = state
        .managed_agent_processes
        .lock()
        .map_err(|e| e.to_string())?;
    let record = find_managed_agent_mut(&mut records, pubkey)?;
    if record.backend != BackendKind::Local {
        return Err(format!("agent {pubkey} is no longer a local agent"));
    }
    if record.updated_at != record_snapshot.updated_at {
        return Err(
            "managed agent changed while native launch preflight was in flight".to_string(),
        );
    }
    if let Err(start_error) =
        start_managed_agent_process(app, record, &mut runtimes, &native_launch)
    {
        if let Err(save_error) = save_managed_agents(app, &records) {
            return Err(format!(
                "{start_error}; additionally failed to persist the stopped agent state: {save_error}"
            ));
        }
        return Err(start_error);
    }
    save_managed_agents(app, &records)?;
    let record = records
        .iter()
        .find(|record| record.pubkey == pubkey)
        .ok_or_else(|| format!("agent {pubkey} not found"))?;
    build_managed_agent_summary(app, record, &runtimes, &personas)
}

/// Deploy an agent to a provider backend. Resolves the binary, calls deploy via
/// spawn_blocking, and persists the result (backend_agent_id or last_error).
///
/// Idempotency: calling deploy on an already-deployed agent sends the same payload
/// again. Providers are expected to handle this as an update-in-place or no-op —
/// the protocol does not include an explicit `undeploy` operation (deferred to v2).
///
/// Returns Ok(()) on success, Err(message) on failure. Either way the record is
/// updated and saved before returning.
async fn deploy_to_provider(
    app: &AppHandle,
    state: &AppState,
    pubkey: &str,
    provider_id: &str,
    config: &serde_json::Value,
    agent_json: serde_json::Value,
    cached_binary_path: Option<&str>,
) -> Result<(), String> {
    // Resolve via discovered candidates only. Cached path must match BOTH
    // "is a discovered candidate" AND "belongs to this provider_id". A tampered
    // record cannot redirect deploys to a different provider's binary.
    let bin_path = cached_binary_path
        .map(std::path::PathBuf::from)
        .filter(|p| p.exists())
        .map(|p| p.canonicalize().unwrap_or(p))
        .filter(|canonical| {
            discover_provider_candidates().iter().any(|(id, cp)| {
                id == provider_id && cp.canonicalize().ok().as_ref() == Some(canonical)
            })
        })
        .map_or_else(|| resolve_provider_binary(provider_id), Ok)?;

    let config_clone = config.clone();
    let deploy_result =
        tokio::task::spawn_blocking(move || provider_deploy(&bin_path, &agent_json, &config_clone))
            .await
            .map_err(|e| format!("spawn_blocking failed: {e}"))?;

    // Persist result under lock.
    let _store_guard = state
        .managed_agents_store_lock
        .lock()
        .map_err(|e| e.to_string())?;
    let mut records = load_managed_agents(app)?;
    let rec = records
        .iter_mut()
        .find(|r| r.pubkey == pubkey)
        .ok_or_else(|| format!("agent {pubkey} not found"))?;

    match deploy_result {
        Ok(backend_agent_id) => {
            rec.backend_agent_id = Some(backend_agent_id);
            rec.last_started_at = Some(now_iso());
            rec.updated_at = now_iso();
            rec.last_error = None;
        }
        Err(ref e) => {
            rec.last_error = Some(e.clone());
            rec.updated_at = now_iso();
            save_managed_agents(app, &records)?;
            return Err(e.clone());
        }
    }
    save_managed_agents(app, &records)?;
    Ok(())
}

// Async so the blocking body (disk reads of agent/persona records, per-agent
// process-liveness syscalls, and a possible save) runs on Tauri's worker pool
// via spawn_blocking instead of the main UI thread — it was a beachball on the
// agents menu mount and after every start/stop/edit refetch. State is re-derived
// from the owned AppHandle inside the closure because `State<'_, _>` is borrowed
// and `std::sync::MutexGuard` is not `Send`.
#[tauri::command]
pub async fn list_managed_agents(app: AppHandle) -> Result<Vec<ManagedAgentSummary>, String> {
    use tauri::Manager;
    tokio::task::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let _store_guard = state
            .managed_agents_store_lock
            .lock()
            .map_err(|error| error.to_string())?;
        let mut records = load_managed_agents(&app)?;
        let mut runtimes = state
            .managed_agent_processes
            .lock()
            .map_err(|error| error.to_string())?;

        let (sync_changed, exited_pubkeys) =
            sync_managed_agent_processes(&mut records, &mut runtimes, &current_instance_id(&app));
        if sync_changed {
            save_managed_agents(&app, &records)?;
        }
        for pubkey in &exited_pubkeys {
            state.clear_agent_session_caches(pubkey);
        }

        let personas = load_personas(&app).unwrap_or_default();
        records
            .iter()
            .map(|record| build_managed_agent_summary(&app, record, &runtimes, &personas))
            .collect()
    })
    .await
    .map_err(|e| format!("spawn_blocking failed: {e}"))?
}

#[tauri::command]
pub async fn create_managed_agent(
    input: CreateManagedAgentRequest,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<CreateManagedAgentResponse, String> {
    let name = input.name.trim().to_string();
    if name.is_empty() {
        return Err("agent name is required".to_string());
    }
    let requested_persona_id = input
        .persona_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    if let Some(parallelism) = input.parallelism {
        if !(1..=32).contains(&parallelism) {
            return Err("parallelism must be between 1 and 32".to_string());
        }
    }
    crate::managed_agents::validate_user_env_keys(&input.env_vars)?;

    // Validate & normalize the respond-to allowlist BEFORE any side effects.
    // The harness has its own validator (buzz-acp/src/config.rs) but we want
    // to catch malformed input at the boundary so the agent never tries to
    // start with a list that will crash it on launch. The mode/allowlist
    // pairing (and the definition-default fallback) is resolved later at the
    // mint site via `resolve_mint_behavioral_defaults`, where the linked
    // definition is in hand.
    let respond_to_allowlist =
        crate::managed_agents::validate_respond_to_allowlist(&input.respond_to_allowlist)?;
    if input.respond_to == Some(crate::managed_agents::RespondTo::Allowlist)
        && respond_to_allowlist.is_empty()
    {
        return Err(
            "respond-to mode 'allowlist' requires at least one pubkey in the allowlist".to_string(),
        );
    }

    // ── Phase 1: allocate an opaque storage id (sync lock) ───────────────────
    let (pubkey, input) = {
        let _store_guard = state
            .managed_agents_store_lock
            .lock()
            .map_err(|error| error.to_string())?;
        let mut records = load_managed_agents(&app)?;
        let mut runtimes = state
            .managed_agent_processes
            .lock()
            .map_err(|error| error.to_string())?;

        let (sync_changed, exited_pubkeys) =
            sync_managed_agent_processes(&mut records, &mut runtimes, &current_instance_id(&app));
        if sync_changed {
            save_managed_agents(&app, &records)?;
        }
        for pubkey in &exited_pubkeys {
            state.clear_agent_session_caches(pubkey);
        }
        if let Some(persona_id) = requested_persona_id.as_deref() {
            let personas = load_personas(&app)?;
            ensure_persona_is_active(&personas, persona_id)?;
        }
        let pubkey = format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
        if records.iter().any(|record| record.pubkey == pubkey) {
            return Err(format!("agent {pubkey} already exists"));
        }

        (pubkey, input)
    };

    // ── Pre-Phase 2: validate provider config BEFORE any side effects ────────
    if let BackendKind::Provider { ref config, ref id } = input.backend {
        validate_provider_config(config)?;
        // Validate via discovered candidates — not raw resolve_command.
        resolve_provider_binary(id)?;
    }

    // ── Phase 3: save record (sync lock) ───────────────────────────────────────
    let agent = {
        let _store_guard = state
            .managed_agents_store_lock
            .lock()
            .map_err(|error| error.to_string())?;
        let mut records = load_managed_agents(&app)?;
        let mut runtimes = state
            .managed_agent_processes
            .lock()
            .map_err(|error| error.to_string())?;

        let (sync_changed, exited_pubkeys) =
            sync_managed_agent_processes(&mut records, &mut runtimes, &current_instance_id(&app));
        if sync_changed {
            save_managed_agents(&app, &records)?;
        }
        for pubkey in &exited_pubkeys {
            state.clear_agent_session_caches(pubkey);
        }

        // Guard against a duplicate pubkey appearing between phase 1 and phase 3
        // (extremely unlikely but safe to check).
        if records.iter().any(|record| record.pubkey == pubkey) {
            return Err(format!("agent {pubkey} already exists"));
        }
        // Provider config was already validated in Pre-Phase 2; cache the discovered binary path for deploy_to_provider.
        let provider_binary_path = if let BackendKind::Provider { ref id, .. } = input.backend {
            // Use resolve_provider_binary (discovered candidates only).
            resolve_provider_binary(id)
                .ok()
                .map(|p| p.display().to_string())
        } else {
            None
        };

        // Load personas once for harness/pack/avatar resolution below.
        let personas = load_personas(&app).unwrap_or_default();
        let global_agent_config = crate::managed_agents::load_global_agent_config(&app)?;

        // Harness resolution: the persona's runtime is authoritative. A
        // persona-backed create stores an `agent_command_override` ONLY when the
        // user deliberately picked a divergent runtime (`harness_override`) —
        // e.g. AddChannelBotDialog's runtime selector. A divergence WITHOUT that
        // flag is a missing-runtime fallback from `resolvePersonaRuntime`, not a
        // pin, and must inherit so it doesn't freeze on the fallback harness once
        // the persona's runtime is installed. A persona-less create always
        // preserves the picked command as a real pin.
        let agent_command_override = crate::managed_agents::create_time_agent_command_override(
            requested_persona_id.as_deref(),
            &personas,
            input.agent_command.as_deref(),
            input.harness_override,
        );
        // The create-time snapshot used for arg/mcp/avatar derivations and
        // legacy reconcile. A global preferred runtime is materialized on the
        // new record; explicit record/persona choices remain authoritative.
        let (agent_command, materialized_runtime) =
            crate::managed_agents::resolve_create_agent_runtime(
                requested_persona_id.as_deref(),
                &personas,
                agent_command_override.as_deref(),
                global_agent_config.preferred_runtime.as_deref(),
            )?;
        let agent_args = normalize_agent_args(
            &agent_command,
            input
                .agent_args
                .iter()
                .map(|arg| arg.trim().to_string())
                .filter(|arg| !arg.is_empty())
                .collect::<Vec<_>>(),
        );

        // Derive MCP command exclusively from the runtime catalog — the
        // per-record field is never read at spawn time so user-supplied input
        // is silently discarded. Always sourcing from the catalog ensures
        // new agents pick up the correct value without any stored override.
        let mcp_command = match crate::managed_agents::known_acp_runtime(&agent_command) {
            Some(p) => p.mcp_command.unwrap_or("").to_string(),
            None => String::new(),
        };

        let team_id = input
            .team_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        if let Some(team_id) = &team_id {
            if !load_teams(&app)?.iter().any(|team| &team.id == team_id) {
                return Err(format!("team {team_id} not found"));
            }
        }

        // Resolve the avatar URL once at creation and persist it on the record.
        // Explicit input wins, then the persona's own avatar, then the runtime
        // fallback. Storing it lets reconciliation compare against what was
        // actually published instead of re-deriving it.
        let persona_avatar_url = requested_persona_id.as_ref().and_then(|persona_id| {
            personas
                .iter()
                .find(|persona| persona.id == *persona_id)?
                .avatar_url
                .clone()
        });
        let resolved_avatar_url = resolve_created_avatar_url(
            input.avatar_url.as_deref(),
            persona_avatar_url,
            &agent_command,
        );

        // Pin the persona config onto the record at create. After this, spawn
        // and deploy read these snapshotted fields, never the live persona, so
        // the agent stays on the config it was created with across restarts;
        // delete+respawn re-runs create and rewrites the snapshot. env_vars are
        // NOT pinned: `record.env_vars` holds agent-level overrides only
        // (input.env_vars), and the live persona env is merged underneath at
        // read time (spawn / readiness / deploy) so persona credential edits
        // refresh on the next spawn like prompt/model/provider already do.
        let linked_persona = requested_persona_id.as_deref().and_then(|pid| {
            load_personas(&app)
                .ok()?
                .into_iter()
                .find(|persona| persona.id == pid)
        });
        let persona_snapshot = linked_persona
            .as_ref()
            .map(crate::managed_agents::persona_events::persona_snapshot);
        let snapshot_prompt = persona_snapshot
            .as_ref()
            .and_then(|s| s.system_prompt.clone());
        let snapshot_model = persona_snapshot.as_ref().and_then(|s| s.model.clone());
        let snapshot_provider = persona_snapshot.as_ref().and_then(|s| s.provider.clone());
        let snapshot_source_version = persona_snapshot.as_ref().map(|s| s.source_version.clone());
        let effective_provider = snapshot_provider
            .or_else(|| input.provider.as_deref().and_then(trim_to_optional_string));
        let mut effective_model =
            snapshot_model.or_else(|| input.model.as_deref().and_then(trim_to_optional_string));
        if effective_provider.as_deref() == Some(crate::managed_agents::SHARED_COMPUTE_PROVIDER_ID)
            && effective_model.is_none()
        {
            effective_model = Some(crate::managed_agents::SHARED_COMPUTE_AUTO_MODEL_ID.to_string());
        }

        // Mint-time behavioral quad: explicit input wins, then the linked
        // definition's NIP-AP defaults, then client defaults. The ONLY parse
        // point for definition behavioral strings — fails loudly on a bad
        // mode/range instead of minting an agent the author didn't describe.
        let minted = crate::managed_agents::resolve_mint_behavioral_defaults(
            input.respond_to,
            respond_to_allowlist.clone(),
            input.parallelism,
            linked_persona.as_ref(),
        )?;

        let record = crate::managed_agents::ManagedAgentRecord {
            pubkey: pubkey.clone(),
            name: name.clone(),
            persona_id: requested_persona_id.clone(),
            team_id,
            avatar_url: resolved_avatar_url.clone(),
            acp_command: input
                .acp_command
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or(DEFAULT_ACP_COMMAND)
                .to_string(),
            agent_command,
            agent_command_override,
            agent_args,
            mcp_command,
            // BUZZ_ACP_TURN_TIMEOUT is deprecated and ignored by the harness;
            // store the schema default only. Use idle_timeout_seconds or
            // max_turn_duration_seconds for actual turn-length control.
            turn_timeout_seconds: DEFAULT_AGENT_TURN_TIMEOUT_SECONDS,
            // 0 or None → harness uses its own default (320s idle, 3600s max), and the CLI also clamps 0 → minimum.
            idle_timeout_seconds: input.idle_timeout_seconds.filter(|s| *s > 0),
            max_turn_duration_seconds: input.max_turn_duration_seconds.filter(|s| *s > 0),
            parallelism: minted.parallelism.unwrap_or(DEFAULT_AGENT_PARALLELISM),
            system_prompt: snapshot_prompt.or_else(|| {
                input
                    .system_prompt
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string)
            }),
            model: effective_model.clone(),
            provider: effective_provider.clone(),
            persona_source_version: snapshot_source_version,
            // Provider agents are managed externally — force false.
            start_on_app_launch: if input.backend != BackendKind::Local {
                false
            } else {
                input.start_on_app_launch
            },
            auto_restart_on_config_change: true,
            runtime_pid: None,
            backend: input.backend.clone(),
            backend_agent_id: None,
            provider_binary_path,
            persona_team_dir: None,
            persona_name_in_team: None,
            env_vars: input.env_vars.clone(),
            created_at: now_iso(),
            updated_at: now_iso(),
            last_started_at: None,
            last_stopped_at: None,
            last_exit_code: None,
            last_error: None,
            last_error_code: None,
            respond_to: minted.respond_to,
            respond_to_allowlist: minted.respond_to_allowlist.clone(),
            display_name: None,
            slug: None,
            runtime: crate::managed_agents::persisted_create_runtime(materialized_runtime),
            name_pool: Vec::new(),
            is_builtin: false,
            is_active: true,
            source_team: None,
            source_team_persona_slug: None,
            definition_respond_to: None,
            definition_respond_to_allowlist: Vec::new(),
            definition_parallelism: None,
        };

        records.push(record);

        save_managed_agents(&app, &records)?;

        let record = records
            .iter()
            .find(|record| record.pubkey == pubkey)
            .ok_or_else(|| "created agent disappeared unexpectedly".to_string())?;
        let personas = load_personas(&app).unwrap_or_default();
        build_managed_agent_summary(&app, record, &runtimes, &personas)?
    };

    // ── Phase 3b: local spawn (async preflight outside store lock) ───────────
    let mut spawn_error = None;
    let agent = if input.spawn_after_create && input.backend == BackendKind::Local {
        match start_local_agent_with_preflight(&app, &state, &pubkey).await {
            Ok(agent) => agent,
            Err(error) => {
                let _store_guard = state
                    .managed_agents_store_lock
                    .lock()
                    .map_err(|e| e.to_string())?;
                let mut records = load_managed_agents(&app)?;
                let runtimes = state
                    .managed_agent_processes
                    .lock()
                    .map_err(|e| e.to_string())?;
                let record = find_managed_agent_mut(&mut records, &pubkey)?;
                record.updated_at = now_iso();
                record.last_error = Some(error.clone());
                save_managed_agents(&app, &records)?;
                spawn_error = Some(error);
                let record = records
                    .iter()
                    .find(|record| record.pubkey == pubkey)
                    .ok_or_else(|| "created agent disappeared unexpectedly".to_string())?;
                let personas = load_personas(&app).unwrap_or_default();
                build_managed_agent_summary(&app, record, &runtimes, &personas)?
            }
        }
    } else {
        agent
    };

    try_regenerate_nest(&app);

    let profile_sync_error: Option<String> = None;

    // ── Phase 5: provider deploy (async, outside lock) ───────────────────────
    let spawn_error = if input.spawn_after_create && input.backend != BackendKind::Local {
        if let BackendKind::Provider { ref id, ref config } = input.backend {
            // Read the saved record to build the deploy payload (record has the
            // canonical field values after Phase 3 normalization).
            let agent_json = {
                let _g = state
                    .managed_agents_store_lock
                    .lock()
                    .map_err(|e| e.to_string())?;
                let records = load_managed_agents(&app)?;
                let rec = records
                    .iter()
                    .find(|r| r.pubkey == pubkey)
                    .ok_or_else(|| "agent disappeared".to_string())?;
                build_deploy_payload(&app, rec)?
            };
            match deploy_to_provider(&app, &state, &pubkey, id, config, agent_json, None).await {
                Ok(()) => spawn_error,
                Err(e) => Some(e),
            }
        } else {
            spawn_error
        }
    } else {
        spawn_error
    };

    // Rebuild summary if provider deploy may have updated backend_agent_id.
    let final_agent = if input.backend != BackendKind::Local && spawn_error.is_none() {
        let _store_guard = state
            .managed_agents_store_lock
            .lock()
            .map_err(|e| e.to_string())?;
        let records = load_managed_agents(&app)?;
        let runtimes = state
            .managed_agent_processes
            .lock()
            .map_err(|e| e.to_string())?;
        let record = records
            .iter()
            .find(|r| r.pubkey == pubkey)
            .ok_or_else(|| "agent disappeared".to_string())?;
        let personas = load_personas(&app).unwrap_or_default();
        build_managed_agent_summary(&app, record, &runtimes, &personas)?
    } else {
        agent
    };

    Ok(CreateManagedAgentResponse {
        agent: final_agent,
        profile_sync_error,
        spawn_error,
    })
}

/// Cold-start one exact managed `(record, group)` runtime from one exact
/// signed child-authored causal row. Every authority-bearing fact is re-read
/// from owner x0xd history; the renderer supplies identifiers only.
#[tauri::command]
pub async fn wake_managed_agent_from_mention(
    input: ManagedAgentMentionWakeRequest,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<ManagedAgentRuntimeStatus, String> {
    let target_record_pubkey = canonical_hex_id(
        &input.target_record_pubkey,
        "managed-agent target record pubkey",
    )?;
    let msg_id = canonical_hex_id(&input.msg_id, "managed mention msg id")?;
    let group_id = input.group_id.trim().to_string();
    if group_id.is_empty() {
        return Err("managed mention group id is required".into());
    }

    let (target_record, records) = {
        let _store_guard = state
            .managed_agents_store_lock
            .lock()
            .map_err(|error| error.to_string())?;
        let records = load_managed_agents(&app)?;
        let target = records
            .iter()
            .find(|record| record.pubkey.eq_ignore_ascii_case(&target_record_pubkey))
            .cloned()
            .ok_or_else(|| format!("agent {target_record_pubkey} not found"))?;
        if target.backend != BackendKind::Local {
            return Err("managed mention target must be a local agent".into());
        }
        (target, records)
    };

    let target_child =
        crate::managed_agents::agent_identity::managed_agent_child_identity(&target_record_pubkey)
            .ok_or_else(|| {
                "managed mention target has no durable native child identity".to_string()
            })?;
    let mut owned_child_by_agent_id = HashMap::new();
    for record in &records {
        if record.backend != BackendKind::Local {
            continue;
        }
        if let Some(child) =
            crate::managed_agents::agent_identity::managed_agent_child_identity(&record.pubkey)
        {
            owned_child_by_agent_id.insert(child.agent_id, record.pubkey.clone());
        }
    }

    let transport = state.x0x_client.resolve_group_transport(&group_id).await?;
    if transport.confidentiality != crate::x0x_client::GroupConfidentiality::SignedPublic {
        return Err("managed mention group must be signed_public".into());
    }
    let row = state
        .x0x_client
        .history_get(&msg_id)
        .await?
        .ok_or_else(|| "managed mention owner history row was not found".to_string())?;
    let (author_agent_id, mention) = require_verified_owner_mention_row(
        &row,
        &transport.stable_group_id,
        &msg_id,
        &target_child.agent_id,
        &owned_child_by_agent_id,
    )?;

    let owner: serde_json::Value = state.x0x_client.get_json("/agent", &[]).await?;
    let owner_agent_id = owner
        .get("agent_id")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "owner /agent response has no AgentId".to_string())?
        .to_ascii_lowercase();
    let root_id = mention
        .delegation_root
        .as_deref()
        .ok_or_else(|| "managed child mention has no owner delegation root".to_string())?;
    let root_row = state
        .x0x_client
        .history_get(root_id)
        .await?
        .ok_or_else(|| "managed child mention owner delegation root was not found".to_string())?;
    require_bounded_agent_delegation(
        &row,
        &mention,
        &root_row,
        &transport.stable_group_id,
        &owner_agent_id,
        &author_agent_id,
    )?;

    // Launch preparation revalidates/repairs exact target membership and
    // translates record-key policy to native child AgentIds. It does not start
    // the harness; redelivery and pending-causal persistence still happen first.
    let native_launch = crate::managed_agents::prepare_managed_agent_launch(
        &app,
        &target_record,
        &group_id,
        crate::managed_agents::GroupBindIntent::EnsureAttached,
    )
    .await?;
    if native_launch.child_agent_id != target_child.agent_id {
        return Err("managed mention target child identity changed during validation".into());
    }
    if !author_allowed_for_launch(
        target_record.respond_to,
        &author_agent_id,
        &native_launch.owner_agent_id,
        &native_launch.effective_respond_to_allowlist,
    ) {
        return Err(
            "managed mention author is not authorized by the target response policy".into(),
        );
    }

    // The child independently revalidates the same delegation chain. Establish
    // its exact owner root first, then the exact causal mention; never inject
    // decoded content or a renderer-supplied substitute into child history.
    let root_receipt = state
        .x0x_client
        .redeliver_group_message(&transport.stable_group_id, root_id, &target_child.agent_id)
        .await?;
    require_exact_redelivery_receipt(
        &root_receipt,
        &transport.stable_group_id,
        root_id,
        &target_child.agent_id,
    )?;
    let mention_receipt = state
        .x0x_client
        .redeliver_group_message(&transport.stable_group_id, &msg_id, &target_child.agent_id)
        .await?;
    require_exact_redelivery_receipt(
        &mention_receipt,
        &transport.stable_group_id,
        &msg_id,
        &target_child.agent_id,
    )?;

    crate::managed_agents::start_managed_agent_runtime_pair_for_causal_message(
        target_record_pubkey,
        transport.stable_group_id,
        msg_id,
        app,
    )
    .await
}

/// Data needed for background profile reconciliation after agent start.
#[tauri::command]
pub async fn start_managed_agent(
    pubkey: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<ManagedAgentSummary, String> {
    enum StartTarget {
        Local,
        Provider {
            backend: BackendKind,
            cached_binary_path: Option<String>,
            agent_json: serde_json::Value,
        },
    }

    // Collect backend info under lock; async preflight/spawn happens below.
    // Also snapshot profile reconciliation data for the background task.
    let target = {
        let _store_guard = state
            .managed_agents_store_lock
            .lock()
            .map_err(|error| error.to_string())?;
        let mut records = load_managed_agents(&app)?;
        let mut runtimes = state
            .managed_agent_processes
            .lock()
            .map_err(|error| error.to_string())?;

        let (sync_changed, exited_pubkeys) =
            sync_managed_agent_processes(&mut records, &mut runtimes, &current_instance_id(&app));
        if sync_changed {
            save_managed_agents(&app, &records)?;
        }
        for pubkey in &exited_pubkeys {
            state.clear_agent_session_caches(pubkey);
        }

        let record = find_managed_agent_mut(&mut records, &pubkey)?;

        if record.backend == BackendKind::Local {
            StartTarget::Local
        } else {
            StartTarget::Provider {
                backend: record.backend.clone(),
                cached_binary_path: record.provider_binary_path.clone(),
                agent_json: build_deploy_payload(&app, record)?,
            }
        }
    };

    let result = match target {
        StartTarget::Local => start_local_agent_with_preflight(&app, &state, &pubkey).await,
        StartTarget::Provider {
            backend: BackendKind::Provider { id, config },
            cached_binary_path,
            agent_json,
        } => {
            deploy_to_provider(
                &app,
                &state,
                &pubkey,
                &id,
                &config,
                agent_json,
                cached_binary_path.as_deref(),
            )
            .await?;

            // Return updated summary.
            let _store_guard = state
                .managed_agents_store_lock
                .lock()
                .map_err(|e| e.to_string())?;
            let records = load_managed_agents(&app)?;
            let runtimes = state
                .managed_agent_processes
                .lock()
                .map_err(|e| e.to_string())?;
            let record = records
                .iter()
                .find(|r| r.pubkey == pubkey)
                .ok_or_else(|| format!("agent {pubkey} not found"))?;
            let personas = load_personas(&app).unwrap_or_default();
            build_managed_agent_summary(&app, record, &runtimes, &personas)
        }
        StartTarget::Provider { backend, .. } => Err(format!(
            "agent {pubkey} has unsupported backend kind: {backend:?}"
        )),
    };

    result
}

#[tauri::command]
pub async fn stop_managed_agent(
    pubkey: String,
    app: AppHandle,
) -> Result<ManagedAgentSummary, String> {
    use tauri::Manager;
    tokio::task::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let _store_guard = state
            .managed_agents_store_lock
            .lock()
            .map_err(|error| error.to_string())?;
        let mut records = load_managed_agents(&app)?;
        let mut runtimes = state
            .managed_agent_processes
            .lock()
            .map_err(|error| error.to_string())?;

        let (sync_changed, exited_pubkeys) =
            sync_managed_agent_processes(&mut records, &mut runtimes, &current_instance_id(&app));
        if sync_changed {
            save_managed_agents(&app, &records)?;
        }
        for pubkey in &exited_pubkeys {
            state.clear_agent_session_caches(pubkey);
        }

        {
            let record = find_managed_agent_mut(&mut records, &pubkey)?;
            // Remote agents are stopped via !shutdown @mention from the frontend,
            // not via this backend command. Reject the call.
            if record.backend != BackendKind::Local {
                return Err(
                    "remote agents are stopped via !shutdown message, not this command".to_string(),
                );
            }
            // Pair-scoped: stops only the active workspace's pair; delete and
            // the config-restart flows still drain every pair.
            stop_managed_agent_workspace_pair(&app, record, &mut runtimes)?;
        }
        save_managed_agents(&app, &records)?;
        let record = records
            .iter()
            .find(|record| record.pubkey == pubkey)
            .ok_or_else(|| format!("agent {pubkey} not found"))?;
        let personas = load_personas(&app).unwrap_or_default();
        build_managed_agent_summary(&app, record, &runtimes, &personas)
    })
    .await
    .map_err(|e| format!("spawn_blocking failed: {e}"))?
}

// Async so the blocking body (disk reads/writes, process termination, keyring
// delete, nest regeneration) runs off the main UI thread via spawn_blocking.
#[tauri::command]
pub async fn delete_managed_agent(
    pubkey: String,
    force_remote_delete: Option<bool>,
    app: AppHandle,
) -> Result<(), String> {
    use tauri::Manager;
    tokio::task::spawn_blocking(move || {
        let state = app.state::<AppState>();
        {
            let _store_guard = state
                .managed_agents_store_lock
                .lock()
                .map_err(|error| error.to_string())?;
            let mut records = load_managed_agents(&app)?;
            let mut runtimes = state
                .managed_agent_processes
                .lock()
                .map_err(|error| error.to_string())?;

            let (sync_changed, exited_pubkeys) = sync_managed_agent_processes(
                &mut records,
                &mut runtimes,
                &current_instance_id(&app),
            );
            if sync_changed {
                save_managed_agents(&app, &records)?;
            }
            for pubkey in &exited_pubkeys {
                state.clear_agent_session_caches(pubkey);
            }

            // Guard: reject deletion of deployed remote agents unless explicitly forced.
            // This turns "don't orphan remote infra" from a UI convention into a backend
            // invariant — a buggy or compromised IPC caller cannot silently orphan a live
            // remote deployment. The frontend sends force_remote_delete: true only after
            // the user confirms the orphan warning.
            if let Some(record) = records.iter().find(|r| r.pubkey == pubkey) {
                if record.backend != BackendKind::Local
                    && record.backend_agent_id.is_some()
                    && !force_remote_delete.unwrap_or(false)
                {
                    return Err(
                        "cannot delete a deployed remote agent without force_remote_delete: true"
                            .to_string(),
                    );
                }
            }

            if let Some(record) = records.iter_mut().find(|record| record.pubkey == pubkey) {
                stop_managed_agent_process(&app, record, &mut runtimes)?;
            }
            state.clear_agent_session_caches(&pubkey);
            let initial_len = records.len();
            records.retain(|record| record.pubkey != pubkey);
            if records.len() == initial_len {
                return Err(format!("agent {pubkey} not found"));
            }
            save_managed_agents(&app, &records)?;
        }
        try_regenerate_nest(&app);
        Ok(())
    })
    .await
    .map_err(|e| format!("spawn_blocking failed: {e}"))?
}

// Remote agent shutdown is handled entirely by the frontend:
// 1. Frontend sends "!shutdown" @mention via WebSocket (signed by user's key)
// 2. Harness sees it, exits gracefully, sets presence to "offline"
// 3. Desktop's existing presence polling sees "offline" — UI updates automatically
// No backend Tauri command needed. Presence IS the status.

#[path = "agents_deploy.rs"]
mod deploy;
use deploy::build_deploy_payload;
#[cfg(test)]
use deploy::deploy_payload_json;
#[cfg(test)]
pub(crate) use deploy::resolve_deploy_model_provider;

#[cfg(test)]
#[path = "agents_tests.rs"]
mod tests;
