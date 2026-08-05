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
}

impl InstanceManifest {
    pub fn fresh(instance_id: &str, template_id: &str) -> Self {
        InstanceManifest {
            instance_id: instance_id.to_string(),
            template_id: template_id.to_string(),
            status: ManifestStatus::InProgress,
            completed: Vec::new(),
        }
    }

    #[cfg(test)]
    pub fn is_complete(&self) -> bool {
        self.status == ManifestStatus::Complete
    }

    pub fn completed_key(&self, key: &str) -> Option<&CompletedStep> {
        self.completed.iter().find(|c| c.key == key)
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
}

impl std::fmt::Display for ManifestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ManifestError::NotFound => write!(f, "manifest not found"),
            ManifestError::Decode(m) => write!(f, "manifest decode error: {m}"),
            ManifestError::Io(m) => write!(f, "manifest io error: {m}"),
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

    if manifest.status == ManifestStatus::Complete {
        manifest.status = ManifestStatus::InProgress;
    }

    let mut steps_out: Vec<StepOutcome> = Vec::with_capacity(plan.steps.len());
    let mut failed = false;

    for step in &plan.steps {
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
                let _ = sink.save(&manifest); // checkpoint per step (durable)
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
                let _ = sink.save(&manifest);
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
                let _ = sink.save(&manifest); // persist resumable state
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

    if !failed {
        manifest.status = ManifestStatus::Complete;
        let _ = sink.save(&manifest);
    }

    // Surface deterministic artifacts if their steps cleared.
    let mut workflow_md = None;
    let mut symphony_config_toml = None;
    if steps_out
        .iter()
        .any(|s| s.kind == "workflow_md" && s.error.is_none())
    {
        workflow_md = Some(plan.workflow_md.clone());
    }
    if steps_out
        .iter()
        .any(|s| s.kind == "symphony_config" && s.error.is_none())
    {
        symphony_config_toml = Some(plan.symphony_config_toml.clone());
    }

    InstantiateOutcome {
        instance_id: plan.instance_id.clone(),
        template_id: plan.template_id.clone(),
        status: manifest.status,
        steps: steps_out,
        workflow_md,
        symphony_config_toml,
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
        let parent = self.path.parent().ok_or_else(|| {
            ManifestError::Io("manifest path has no parent directory".to_string())
        })?;
        std::fs::create_dir_all(parent).map_err(|error| ManifestError::Io(error.to_string()))?;
        let bytes = serde_json::to_vec_pretty(manifest)
            .map_err(|error| ManifestError::Decode(error.to_string()))?;
        let tmp_path = self.path.with_extension("json.tmp");
        std::fs::write(&tmp_path, bytes).map_err(|error| ManifestError::Io(error.to_string()))?;
        std::fs::rename(&tmp_path, &self.path).map_err(|error| ManifestError::Io(error.to_string()))
    }
}

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
        let json = serde_json::to_string(manifest)
            .map_err(|error| ManifestError::Decode(error.to_string()))?;
        let mut guard = self.cell.lock().unwrap_or_else(|error| error.into_inner());
        *guard = Some(json);
        Ok(())
    }
}
