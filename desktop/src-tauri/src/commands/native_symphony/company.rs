use tauri::State;

use crate::app_state::AppState;
use crate::company_template::instantiate::ManifestStatus;

use super::instances::{company_instance_dir, existing_active_instance};
use super::lifecycle::{lifecycle_is_complete, run_company_lifecycle};

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
    pub(super) id: String,
    pub(super) name: String,
    pub(super) kind: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct CompanyInstantiatedAgent {
    pub(super) agent_id: String,
    pub(super) role: String,
    pub(super) group_id: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct CompanyInstantiationError {
    pub(super) kind: String,
    pub(super) r#ref: String,
    pub(super) message: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct CompanyInstantiationResult {
    pub(super) instance_id: String,
    pub(super) run_id: Option<String>,
    pub(super) groups: Vec<CompanyInstantiatedGroup>,
    pub(super) agents: Vec<CompanyInstantiatedAgent>,
    pub(super) workflow_md: Option<String>,
    pub(super) errors: Vec<CompanyInstantiationError>,
    /// Manifest lifecycle status serialized snake_case: "complete" |
    /// "resumable" | "in_progress". Discriminates a fully-provisioned company
    /// from a durable, resumable partial run (`run_id`/`workflow_md` are `None`
    /// and `agents` is empty until "complete").
    pub(super) status: ManifestStatus,
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
/// template.
///
/// Fail-closed guarantees:
/// - **Contract gate.** The template must satisfy the supported-runtime
///   contract (one uniform Symphony runner, one primary task queue).
/// - **Single-active-company.** Refused while ANY non-cancelled instance exists
///   on disk (not only the live binding); the active process is never killed —
///   cancel first.
/// - **Race-free reservation.** The reservation (scan + initial manifest write)
///   is atomic under `company_instantiate_lock`; the full lifecycle is then
///   serialized under `company_run_lock` so instantiate/resume/reconcile never
///   overlap.
/// - **Provisioning gate.** Identities are minted, members are added, Symphony
///   is started, and the run issue is created ONLY once every provisioning step
///   is `Complete`; each post-phase is a durable checkpoint. A partial outcome
///   is durable + resumable (`status: "resumable"`); cancel during any phase is
///   terminal (`status: "cancelled"`) and owns no processes.
/// No relay fallback exists; every error propagates as a structured item.
#[tauri::command]
pub async fn instantiate_company_template(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    template_id: String,
    input: InstantiateCompanyTemplateInput,
) -> Result<CompanyInstantiationResult, String> {
    use crate::company_template::instantiate::{InstanceManifest, JsonManifestSink, ManifestSink};
    use crate::company_template::parse::validate_supported_contract;
    use crate::company_template::plan::instance_slug;

    let template = crate::company_template::registry::get_company_template(&template_id)
        .ok_or_else(|| format!("unknown Company template `{template_id}`"))?;
    validate_supported_contract(&template).map_err(|e| e.to_string())?;

    let display_name = input
        .display_name
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| template.name.clone());

    // Canonical lock order: run-outermost → instantiate (nested). The async run
    // lock serializes the full lifecycle; the sync instantiate lock guards the
    // reservation. resume_company_template uses the SAME single nesting order,
    // so no path takes instantiate-then-run (no ABBA inverse).
    let _run_guard = state.company_run_lock.lock().await;

    // Atomic single-active reservation. The lock spans the active-instance
    // scan AND the initial reservation-manifest write, so two concurrent
    // instantiate calls cannot both pass the scan and create a second company
    // (the classic check-then-act race). The critical section is sync and fast
    // (a dir scan + one small atomic file write); the guard is released before
    // any await.
    let instance_id = {
        let _guard = state
            .company_instantiate_lock
            .lock()
            .map_err(|e| format!("company instantiate lock poisoned: {e}"))?;

        // Single-active-company: refuse while ANY non-cancelled instance exists
        // (Complete OR incomplete). The active process is never killed — cancel
        // or resume the existing one first.
        if let Some(existing) = existing_active_instance() {
            return Err(format!(
                "another Company instance `{existing}` is already active; cancel it before starting a new company"
            ));
        }

        let created_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|error| format!("system clock is before Unix epoch: {error}"))?
            .as_millis();
        let instance_id = format!("{}-{created_at}", instance_slug(&display_name));
        let instance_dir = company_instance_dir(&instance_id)?;
        let manifest = JsonManifestSink::new(instance_dir.join("manifest.json"));
        // Reserve the slot on disk (in_progress, non-cancelled) so a concurrent
        // instantiate's scan observes it and refuses. The display name is
        // persisted so a resume recreates the run issue with the same title.
        manifest
            .save(
                &InstanceManifest::fresh(&instance_id, &template.id)
                    .with_display_name(&display_name),
            )
            .map_err(|error| format!("failed to reserve Company instance slot: {error}"))?;
        instance_id
    };

    run_company_lifecycle(&app, &state, &template, &instance_id, &display_name).await
}

/// Resume an existing in_progress/resumable Company instance by its exact id —
/// NEVER mints a new id. Re-runs the durable lifecycle: provisioning skips every
/// already-completed step, then the post-phases (identities, membership, Symphony
/// bind, run issue) advance idempotently to `Complete`, or remain honestly
/// `Resumable`. A `Complete` instance whose lifecycle is already finished, and a
/// `Cancelled` instance, are refused (cancel is terminal; complete needs no
/// resume). Serialized under `company_run_lock` like instantiate.
#[tauri::command]
pub async fn resume_company_instance(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    instance_id: String,
) -> Result<CompanyInstantiationResult, String> {
    use crate::company_template::instantiate::{JsonManifestSink, ManifestSink};

    // Lock order is run-outermost → instantiate, the SAME single nesting order
    // used by `instantiate_company_template` (run lock held across the whole
    // lifecycle, instantiate lock nested inside for the sync reservation
    // validation). Establishing one order on both paths eliminates the ABBA
    // inverse: there is no path that takes instantiate-then-run.
    let _run_guard = state.company_run_lock.lock().await;

    // Validate the manifest exists and is resumable under the reservation lock
    // (sync, fast), nested inside the run lock.
    let (template_id, display_name) = {
        let _guard = state
            .company_instantiate_lock
            .lock()
            .map_err(|e| format!("company instantiate lock poisoned: {e}"))?;
        let manifest_path = company_instance_dir(&instance_id)?.join("manifest.json");
        let manifest = JsonManifestSink::new(&manifest_path)
            .load()
            .map_err(|error| format!("failed to load Company manifest for resume: {error}"))?;
        if manifest.instance_id != instance_id {
            return Err(format!(
                "instance id mismatch: requested `{instance_id}` but manifest is `{}`",
                manifest.instance_id
            ));
        }
        if manifest.status == ManifestStatus::Cancelled {
            return Err(format!(
                "Company instance `{instance_id}` is cancelled; cancelled runs are terminal"
            ));
        }
        if lifecycle_is_complete(&manifest) {
            return Err(format!(
                "Company instance `{instance_id}` is already complete; nothing to resume"
            ));
        }
        let template_id = manifest.template_id.clone();
        let display_name = manifest
            .display_name
            .clone()
            .unwrap_or_else(|| template_id.clone());
        (template_id, display_name)
    };

    let template = crate::company_template::registry::get_company_template(&template_id)
        .ok_or_else(|| format!("unknown Company template `{template_id}`"))?;
    run_company_lifecycle(&app, &state, &template, &instance_id, &display_name).await
}

pub(super) fn company_group_kind(group_id: &str) -> String {
    match group_id {
        "engineering" => "engineering",
        "sales" => "sales",
        "all-hands" => "all_hands",
        _ => "custom",
    }
    .to_string()
}
