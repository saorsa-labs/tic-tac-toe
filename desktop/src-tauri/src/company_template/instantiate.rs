//! Idempotent, resumable instantiation executor.
//!
//! [`instantiate_company`] walks an [`InstantiationPlan`], provisioning each
//! resource through a [`CompanyProvisioner`] and checkpointing progress into a
//! durable [`InstanceManifest`] after every step. Re-running with the same
//! `instance_id` (and the same manifest) skips every already-completed step —
//! **zero** new provisioning calls. On a partial failure the manifest is left
//! `Resumable` and the caller is handed back a durable, resumable state.
//!
//! Failure semantics (per the M4 contract: "roll back **or** return a durable
//! resumable state"): this path returns a durable resumable state. Rollback is
//! intentionally avoided — created groups are not safely deletable, and stores
//! / task-lists are already `409`-idempotent, so leaving them in place is safe
//! and a replay adopts or skips them.
//!
//! No relay events are emitted. The Symphony config and WORKFLOW.md are pure
//! deterministic artifacts already carried by the plan; the executor marks
//! their write-steps complete and surfaces their text in the outcome for the
//! caller to persist.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::company_template::plan::{InstantiationPlan, PlanStep};
use crate::company_template::provisioner::{
    CompanyProvisioner, EnsureGroupRequest, EnsureStoreRequest, EnsureTaskListRequest,
};

/// Manifest lifecycle status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManifestStatus {
    /// Some steps done; more remain.
    InProgress,
    /// Every step completed.
    Complete,
    /// A step failed; the manifest records all completed steps and the run can
    /// be resumed by re-calling [`instantiate_company`].
    Resumable,
    /// The operator cancelled the run. Terminal: boot reconciliation and
    /// resumption skip cancelled instances. Set only by the cancel tombstone,
    /// never by the provisioning executor.
    Cancelled,
}

/// One completed step recorded in the durable manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompletedStep {
    pub key: String,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_id: Option<String>,
    pub resource_id: String,
    pub created: bool,
}

/// The durable, resumable record of an instantiation. Persisted by the
/// [`ManifestSink`] after every step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstanceManifest {
    pub instance_id: String,
    pub template_id: String,
    pub status: ManifestStatus,
    pub completed: Vec<CompletedStep>,
    /// The Symphony issue id created for this instance's run, persisted after
    /// the run is created so the frontend can reconstruct the runId→instanceId
    /// mapping from disk (not just localStorage).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    /// Epoch-millis at which the operator cancelled the run (`None` unless
    /// `status == Cancelled`). Boot reconciliation reads this to skip/respect
    /// cancelled instances across restarts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cancelled_at: Option<u128>,
    /// Monotonic cancellation generation. Incremented every time a `Cancelled`
    /// tombstone is written. The provisioning executor captures the value at
    /// the start of a run and re-reads durable state before every checkpoint
    /// and post-step: a generation that advanced (or a `Cancelled` status)
    /// means the run was cancelled and must stop without overwriting the
    /// tombstone. Defaults to `0` so legacy manifests load unchanged.
    #[serde(default)]
    pub cancel_generation: u64,
    /// The operator-supplied display name, persisted so a resume recreates the
    /// Symphony run issue with the same title. `None` on legacy manifests.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
}

impl InstanceManifest {
    pub fn fresh(instance_id: &str, template_id: &str) -> Self {
        InstanceManifest {
            instance_id: instance_id.to_string(),
            template_id: template_id.to_string(),
            status: ManifestStatus::InProgress,
            completed: Vec::new(),
            run_id: None,
            cancelled_at: None,
            cancel_generation: 0,
            display_name: None,
        }
    }

    /// Record the operator display name on the durable manifest. Used by the
    /// orchestrator after reservation so a resume can recreate the run issue.
    pub fn with_display_name(mut self, display_name: &str) -> Self {
        if !display_name.trim().is_empty() {
            self.display_name = Some(display_name.to_string());
        }
        self
    }

    #[cfg(test)]
    pub fn is_complete(&self) -> bool {
        self.status == ManifestStatus::Complete
    }

    pub fn completed_key(&self, key: &str) -> Option<&CompletedStep> {
        self.completed.iter().find(|c| c.key == key)
    }

    /// Whether a post-provisioning phase (`identities` / `membership` /
    /// `symphony_bind` / `run_issue`) is already checkpointed for this instance.
    pub fn has_post_phase(&self, instance_id: &str, kind: &str) -> bool {
        self.completed_key(&post_phase_key(instance_id, kind))
            .is_some()
    }
}

/// Stable manifest key for a post-provisioning phase. Prefixed `post:` and
/// namespaced by `instance_id` so it never collides with a provisioning
/// `step_key` and is unique per instance.
pub fn post_phase_key(instance_id: &str, kind: &str) -> String {
    format!("post:{kind}:{instance_id}")
}

/// A completed post-provisioning phase recorded as a [`CompletedStep`].
pub fn post_phase_step(instance_id: &str, kind: &str, resource_id: &str) -> CompletedStep {
    CompletedStep {
        key: post_phase_key(instance_id, kind),
        kind: kind.to_string(),
        local_id: None,
        resource_id: resource_id.to_string(),
        created: true,
    }
}

/// Durable storage for the instance manifest. Implementations: a JSON file
/// sink for production, an in-memory sink for tests.
pub trait ManifestSink: Send + Sync {
    fn load(&self) -> Result<InstanceManifest, ManifestError>;
    fn save(&self, manifest: &InstanceManifest) -> Result<(), ManifestError>;
}

/// Manifest I/O error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManifestError {
    NotFound,
    Decode(String),
    Io(String),
    /// A non-Cancelled manifest was not persisted because a durable `Cancelled`
    /// tombstone already exists for this instance. Returned by [`ManifestSink`]
    /// saves to make cancellation terminal: the executor can never overwrite a
    /// cancel. The caller treats this as "the run was cancelled".
    Cancelled,
}

impl std::fmt::Display for ManifestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ManifestError::NotFound => write!(f, "manifest not found"),
            ManifestError::Decode(m) => write!(f, "manifest decode error: {m}"),
            ManifestError::Io(m) => write!(f, "manifest io error: {m}"),
            ManifestError::Cancelled => write!(f, "manifest cancelled"),
        }
    }
}

impl std::error::Error for ManifestError {}

/// Outcome of a single step in an instantiation run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StepOutcome {
    pub key: String,
    pub kind: String,
    pub local_id: Option<String>,
    /// The realized x0xd resource id (group_id / topic), if any.
    pub resource_id: Option<String>,
    /// `true` if newly created this run; `false` if adopted or skipped.
    pub created: bool,
    /// `true` if this step was skipped because it was already in the manifest.
    pub skipped: bool,
    pub error: Option<String>,
}

/// The full outcome of an instantiation run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InstantiateOutcome {
    pub instance_id: String,
    pub template_id: String,
    pub status: ManifestStatus,
    pub steps: Vec<StepOutcome>,
    /// Deterministic WORKFLOW.md text (present once the workflow step clears).
    pub workflow_md: Option<String>,
    /// Deterministic Symphony config TOML (present once the config step clears).
    pub symphony_config_toml: Option<String>,
    /// A durable-checkpoint (manifest save) failure, if any; `None` on a clean
    /// run. Separate from per-step `steps[].error` because a checkpoint failure
    /// halts the whole run regardless of which step triggered it. The caller
    /// folds this into the structured error channel.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub manifest_error: Option<String>,
}

impl InstantiateOutcome {
    #[cfg(test)]
    pub fn is_complete(&self) -> bool {
        self.status == ManifestStatus::Complete
    }

    /// Per-item errors for UI rendering (kind, ref, message).
    pub fn errors(&self) -> Vec<(&str, Option<&str>, &str)> {
        self.steps
            .iter()
            .filter_map(|s| {
                s.error
                    .as_ref()
                    .map(|m| (s.kind.as_str(), s.local_id.as_deref(), m.as_str()))
            })
            .collect()
    }
}

/// Walk `plan`, provisioning each step through `provisioner` and checkpointing
/// into `sink`. Idempotent + resumable.
pub async fn instantiate_company<P: CompanyProvisioner>(
    plan: &InstantiationPlan,
    provisioner: &P,
    sink: &dyn ManifestSink,
) -> InstantiateOutcome {
    let mut manifest = sink
        .load()
        .ok()
        .filter(|m| m.instance_id == plan.instance_id && m.template_id == plan.template_id)
        .unwrap_or_else(|| InstanceManifest::fresh(&plan.instance_id, &plan.template_id));

    // Cancelled is terminal: a cancelled instance is never resumed or replayed.
    // Boot reconciliation and the single-active gate skip it; so does the
    // executor.
    if manifest.status == ManifestStatus::Cancelled {
        return InstantiateOutcome {
            instance_id: plan.instance_id.clone(),
            template_id: plan.template_id.clone(),
            status: ManifestStatus::Cancelled,
            steps: Vec::new(),
            workflow_md: None,
            symphony_config_toml: None,
            manifest_error: None,
        };
    }

    // Re-entering a Complete manifest re-runs idempotently: every step is
    // skipped, then the loop re-asserts Complete. This is the resume/replay
    // path exercised by the idempotency tests.
    if manifest.status == ManifestStatus::Complete {
        manifest.status = ManifestStatus::InProgress;
    }

    // Cancellation generation observed at the start of the run. Before every
    // checkpoint we re-read durable state; a cancel tombstone (status Cancelled
    // or an advanced generation) stops the run without overwriting the tombstone.
    let observed_generation = manifest.cancel_generation;

    let mut steps_out: Vec<StepOutcome> = Vec::with_capacity(plan.steps.len());
    let mut failed = false;
    let mut cancelled = false;
    // A durable-checkpoint failure (manifest save) halts the whole run and is
    // surfaced separately from per-step provisioning errors.
    let mut checkpoint_failure: Option<String> = None;

    for step in &plan.steps {
        // Cooperative cancellation: re-read the durable tombstone before each
        // step. A cancel that landed between steps stops the run without doing
        // further provisioning work. (The sink's save-time guard is the hard
        // backstop; this check avoids needless work after a cancel.)
        if cancellation_requested(sink, &plan.instance_id, observed_generation) {
            cancelled = true;
            break;
        }

        // Skip already-completed steps (idempotent replay).
        if let Some(done) = manifest.completed_key(step.key()) {
            steps_out.push(StepOutcome {
                key: step.key().to_string(),
                kind: step.kind_label().to_string(),
                local_id: step.local_target().map(|s| s.to_string()),
                resource_id: Some(done.resource_id.clone()),
                created: false,
                skipped: true,
                error: None,
            });
            continue;
        }

        let attempt = match step {
            PlanStep::CreateGroup {
                name,
                purpose,
                visibility,
                ..
            } => provisioner
                .ensure_group(EnsureGroupRequest {
                    name: name.clone(),
                    description: purpose.clone().unwrap_or_default(),
                    visibility: *visibility,
                    idempotency_key: step.key().to_string(),
                })
                .await
                .map(Some),
            PlanStep::CreateStore {
                name,
                topic,
                policy,
                ..
            } => provisioner
                .ensure_store(EnsureStoreRequest {
                    name: name.clone(),
                    topic: topic.clone(),
                    policy: *policy,
                })
                .await
                .map(Some),
            PlanStep::CreateTaskList { name, topic, .. } => provisioner
                .ensure_task_list(EnsureTaskListRequest {
                    name: name.clone(),
                    topic: topic.clone(),
                })
                .await
                .map(Some),
            // Pure deterministic artifacts — content already in the plan.
            PlanStep::WriteSymphonyConfig { .. } | PlanStep::WriteWorkflowMd { .. } => Ok(None),
        };

        match attempt {
            Ok(Some(res)) => {
                let record = CompletedStep {
                    key: step.key().to_string(),
                    kind: step.kind_label().to_string(),
                    local_id: step.local_target().map(|s| s.to_string()),
                    resource_id: res.id.clone(),
                    created: res.created,
                };
                manifest.completed.push(record);
                match sink.save(&manifest) {
                    Ok(()) => {}
                    Err(ManifestError::Cancelled) => {
                        cancelled = true;
                        break;
                    }
                    Err(err) => {
                        // Durable checkpoint failed: the resource was created
                        // but cannot be recorded. Stop provisioning fail-closed
                        // — a resume re-runs this step and adopts the resource
                        // idempotently. Never claim progress we could not
                        // persist.
                        manifest.status = ManifestStatus::Resumable;
                        checkpoint_failure = Some(format!("manifest checkpoint failed: {err}"));
                        steps_out.push(StepOutcome {
                            key: step.key().to_string(),
                            kind: step.kind_label().to_string(),
                            local_id: step.local_target().map(|s| s.to_string()),
                            resource_id: None,
                            created: false,
                            skipped: false,
                            error: Some(format!("manifest checkpoint failed: {err}")),
                        });
                        failed = true;
                        break;
                    }
                }
                steps_out.push(StepOutcome {
                    key: step.key().to_string(),
                    kind: step.kind_label().to_string(),
                    local_id: step.local_target().map(|s| s.to_string()),
                    resource_id: Some(res.id),
                    created: res.created,
                    skipped: false,
                    error: None,
                });
            }
            Ok(None) => {
                // File-write step: mark done; content surfaces in the outcome.
                manifest.completed.push(CompletedStep {
                    key: step.key().to_string(),
                    kind: step.kind_label().to_string(),
                    local_id: None,
                    resource_id: step.kind_label().to_string(),
                    created: true,
                });
                match sink.save(&manifest) {
                    Ok(()) => {}
                    Err(ManifestError::Cancelled) => {
                        cancelled = true;
                        break;
                    }
                    Err(err) => {
                        manifest.status = ManifestStatus::Resumable;
                        checkpoint_failure = Some(format!("manifest checkpoint failed: {err}"));
                        steps_out.push(StepOutcome {
                            key: step.key().to_string(),
                            kind: step.kind_label().to_string(),
                            local_id: None,
                            resource_id: None,
                            created: false,
                            skipped: false,
                            error: Some(format!("manifest checkpoint failed: {err}")),
                        });
                        failed = true;
                        break;
                    }
                }
                steps_out.push(StepOutcome {
                    key: step.key().to_string(),
                    kind: step.kind_label().to_string(),
                    local_id: None,
                    resource_id: None,
                    created: true,
                    skipped: false,
                    error: None,
                });
            }
            Err(err) => {
                manifest.status = ManifestStatus::Resumable;
                // Best-effort persist of the resumable state; if even this
                // fails, surface it alongside the provisioning error. A cancel
                // that landed here wins: the run is cancelled, not resumable.
                match sink.save(&manifest) {
                    Ok(()) => {}
                    Err(ManifestError::Cancelled) => {
                        cancelled = true;
                        checkpoint_failure = None;
                        failed = false;
                        break;
                    }
                    Err(io_err) => {
                        checkpoint_failure = Some(format!(
                            "manifest checkpoint failed while recording resumable state: {io_err}"
                        ));
                    }
                }
                steps_out.push(StepOutcome {
                    key: step.key().to_string(),
                    kind: step.kind_label().to_string(),
                    local_id: step.local_target().map(|s| s.to_string()),
                    resource_id: None,
                    created: false,
                    skipped: false,
                    error: Some(err.to_string()),
                });
                failed = true;
                break;
            }
        }
    }

    // Resolve the provisioning-phase outcome. The durable manifest is NEVER
    // marked Complete by the provisioning executor: full lifecycle completion
    // (identities + membership + symphony bind + run_id) is owned by the
    // orchestrator (`run_company_lifecycle`), which marks Complete atomically
    // only after every post-phase and a durable run_id. Provisioning success
    // therefore persists InProgress (every step is already checkpointed); the
    // outcome signals provisioning-phase completion so the orchestrator advances
    // to the post-phases. Cancelled is never persisted here — the durable
    // tombstone written by the cancel path is authoritative.
    let outcome_status = if cancelled {
        ManifestStatus::Cancelled
    } else if failed {
        // A step or checkpoint failed and already persisted Resumable above.
        manifest.status
    } else {
        manifest.status = ManifestStatus::InProgress;
        match sink.save(&manifest) {
            Ok(()) => ManifestStatus::Complete,
            Err(ManifestError::Cancelled) => ManifestStatus::Cancelled,
            Err(err) => {
                checkpoint_failure = Some(format!("manifest checkpoint failed: {err}"));
                ManifestStatus::Resumable
            }
        }
    };

    let workflow_md = artifact_if_cleared(&steps_out, plan, "workflow_md");
    let symphony_config_toml = artifact_if_cleared(&steps_out, plan, "symphony_config");
    InstantiateOutcome {
        instance_id: plan.instance_id.clone(),
        template_id: plan.template_id.clone(),
        status: outcome_status,
        steps: steps_out,
        workflow_md,
        symphony_config_toml,
        manifest_error: checkpoint_failure,
    }
}

/// Whether a cancel tombstone has been written for `instance_id` since the run
/// observed `observed_generation`. Re-reads durable state: true iff the on-disk
/// manifest for this instance is `Cancelled` or its `cancel_generation` advanced.
/// A missing/unreadable manifest is treated as not-cancelled (the save-time
/// guard is the backstop).
fn cancellation_requested(
    sink: &dyn ManifestSink,
    instance_id: &str,
    observed_generation: u64,
) -> bool {
    match sink.load() {
        Ok(m) if m.instance_id == instance_id => {
            m.status == ManifestStatus::Cancelled || m.cancel_generation > observed_generation
        }
        _ => false,
    }
}

/// Surface a deterministic plan artifact iff its write step cleared this run.
fn artifact_if_cleared(
    steps_out: &[StepOutcome],
    plan: &InstantiationPlan,
    kind: &str,
) -> Option<String> {
    if steps_out
        .iter()
        .any(|s| s.kind == kind && s.error.is_none())
    {
        match kind {
            "workflow_md" => Some(plan.workflow_md.clone()),
            "symphony_config" => Some(plan.symphony_config_toml.clone()),
            _ => None,
        }
    } else {
        None
    }
}

// ── Durable JSON manifest sink ────────────────────────────────────────────

/// File-backed manifest storage for production Company instances.
///
/// Saves are atomic (write a sibling temporary file, then rename), so a crash
/// cannot leave a truncated manifest that silently replays completed steps.
pub struct JsonManifestSink {
    path: PathBuf,
}

impl JsonManifestSink {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    #[cfg(test)]
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }
}

impl ManifestSink for JsonManifestSink {
    fn load(&self) -> Result<InstanceManifest, ManifestError> {
        let bytes = std::fs::read(&self.path).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                ManifestError::NotFound
            } else {
                ManifestError::Io(error.to_string())
            }
        })?;
        serde_json::from_slice(&bytes).map_err(|error| ManifestError::Decode(error.to_string()))
    }

    fn save(&self, manifest: &InstanceManifest) -> Result<(), ManifestError> {
        // A Cancelled tombstone is terminal. Refuse to clobber it with a
        // non-Cancelled state — this is the hard backstop for cooperative
        // cancellation: even if the executor's in-memory generation check
        // misses a cancel, it can never overwrite a durable tombstone. Saving a
        // fresh/Cancelled manifest over a missing file is always allowed.
        if manifest.status != ManifestStatus::Cancelled {
            if let Ok(existing) = self.load() {
                if existing.instance_id == manifest.instance_id
                    && existing.status == ManifestStatus::Cancelled
                {
                    return Err(ManifestError::Cancelled);
                }
            }
        }
        let parent = self.path.parent().ok_or_else(|| {
            ManifestError::Io("manifest path has no parent directory".to_string())
        })?;
        std::fs::create_dir_all(parent).map_err(|error| ManifestError::Io(error.to_string()))?;
        restrict_dir(parent);
        let bytes = serde_json::to_vec_pretty(manifest)
            .map_err(|error| ManifestError::Decode(error.to_string()))?;
        let tmp_path = self.path.with_extension("json.tmp");
        std::fs::write(&tmp_path, bytes).map_err(|error| ManifestError::Io(error.to_string()))?;
        restrict_file(&tmp_path);
        std::fs::rename(&tmp_path, &self.path).map_err(|error| ManifestError::Io(error.to_string()))
    }
}

/// Set directory permissions to 0700 (owner-only) on Unix; no-op elsewhere.
#[cfg(unix)]
fn restrict_dir(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700));
}

/// Set file permissions to 0600 (owner-only) on Unix; no-op elsewhere.
#[cfg(unix)]
fn restrict_file(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn restrict_dir(_path: &std::path::Path) {}

#[cfg(not(unix))]
fn restrict_file(_path: &std::path::Path) {}

/// In-memory manifest storage used by deterministic provisioning tests.
#[cfg(test)]
pub struct InMemorySink {
    cell: std::sync::Mutex<Option<String>>,
}

#[cfg(test)]
impl InMemorySink {
    pub fn new() -> Self {
        Self {
            cell: std::sync::Mutex::new(None),
        }
    }
}

#[cfg(test)]
impl Default for InMemorySink {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
impl ManifestSink for InMemorySink {
    fn load(&self) -> Result<InstanceManifest, ManifestError> {
        let guard = self.cell.lock().unwrap_or_else(|error| error.into_inner());
        match guard.as_ref() {
            None => Err(ManifestError::NotFound),
            Some(json) => {
                serde_json::from_str(json).map_err(|error| ManifestError::Decode(error.to_string()))
            }
        }
    }

    fn save(&self, manifest: &InstanceManifest) -> Result<(), ManifestError> {
        // Mirror the production sink: a Cancelled tombstone is terminal.
        if manifest.status != ManifestStatus::Cancelled {
            let guard = self.cell.lock().unwrap_or_else(|error| error.into_inner());
            if let Some(existing_json) = guard.as_ref() {
                if let Ok(existing) = serde_json::from_str::<InstanceManifest>(existing_json) {
                    if existing.instance_id == manifest.instance_id
                        && existing.status == ManifestStatus::Cancelled
                    {
                        return Err(ManifestError::Cancelled);
                    }
                }
            }
        }
        let json = serde_json::to_string(manifest)
            .map_err(|error| ManifestError::Decode(error.to_string()))?;
        let mut guard = self.cell.lock().unwrap_or_else(|error| error.into_inner());
        *guard = Some(json);
        Ok(())
    }
}
