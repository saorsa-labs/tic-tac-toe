//! Deterministic, idempotent instantiation planning.
//!
//! [`plan_instantiation`] is a **pure** function of `(template, instance_id)`:
//! identical inputs yield a byte-identical [`InstantiationPlan`] — same step
//! order, same step keys, same derived store/task-list topics, same generated
//! Symphony config and WORKFLOW.md. This determinism is what makes
//! instantiation idempotent: re-running with the same `instance_id` reproduces
//! the same step keys, so the durable [`crate::company_template::instantiate`]
//! manifest skips every already-completed step.
//!
//! No I/O, no relay events.

use sha2::{Digest, Sha256};

use crate::company_template::spec::{CompanyTemplate, GroupVisibility, StoreKind, StorePolicy};
use crate::company_template::symphony_config::generate_symphony_config;
use crate::company_template::workflow::generate_workflow_md;

/// A single ordered, idempotent step in an instantiation plan.
///
/// Every variant carries a deterministic `key` (sha256 of
/// `instance_id|kind|local_id`) used as the manifest identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanStep {
    CreateGroup {
        key: String,
        local_id: String,
        /// The x0xd group `name` (info.name) — the dedup key for ensure_group.
        name: String,
        visibility: GroupVisibility,
        purpose: Option<String>,
    },
    CreateStore {
        key: String,
        local_id: String,
        name: String,
        topic: String,
        kind: StoreKind,
        policy: StorePolicy,
        group_local_id: Option<String>,
    },
    CreateTaskList {
        key: String,
        local_id: String,
        name: String,
        topic: String,
        group_local_id: Option<String>,
    },
    WriteSymphonyConfig {
        key: String,
    },
    WriteWorkflowMd {
        key: String,
    },
}

impl PlanStep {
    /// The deterministic manifest key for this step.
    pub fn key(&self) -> &str {
        match self {
            PlanStep::CreateGroup { key, .. }
            | PlanStep::CreateStore { key, .. }
            | PlanStep::CreateTaskList { key, .. }
            | PlanStep::WriteSymphonyConfig { key }
            | PlanStep::WriteWorkflowMd { key } => key,
        }
    }

    /// A stable, human-readable kind label for diagnostics/logging.
    pub fn kind_label(&self) -> &'static str {
        match self {
            PlanStep::CreateGroup { .. } => "group",
            PlanStep::CreateStore { .. } => "store",
            PlanStep::CreateTaskList { .. } => "task_list",
            PlanStep::WriteSymphonyConfig { .. } => "symphony_config",
            PlanStep::WriteWorkflowMd { .. } => "workflow_md",
        }
    }

    /// The template-local target id of this step, or `None` for synthetic
    /// file-write steps.
    pub fn local_target(&self) -> Option<&str> {
        match self {
            PlanStep::CreateGroup { local_id, .. }
            | PlanStep::CreateStore { local_id, .. }
            | PlanStep::CreateTaskList { local_id, .. } => Some(local_id),
            PlanStep::WriteSymphonyConfig { .. } | PlanStep::WriteWorkflowMd { .. } => None,
        }
    }
}

/// The full, deterministic plan for one instantiation.
#[derive(Debug, Clone)]
pub struct InstantiationPlan {
    pub instance_id: String,
    pub template_id: String,
    pub steps: Vec<PlanStep>,
    pub symphony_config_toml: String,
    pub workflow_md: String,
}

impl InstantiationPlan {
    #[cfg(test)]
    pub fn step_count(&self) -> usize {
        self.steps.len()
    }

    /// Keys of every step, in order. Stable for a given plan.
    #[cfg(test)]
    pub fn step_keys(&self) -> Vec<&str> {
        self.steps.iter().map(|s| s.key()).collect()
    }
}
/// Compute the deterministic manifest key for a step.
///
/// `kind` is the stable step-kind label; `target` is the template-local id
/// (or a synthetic id for file-write steps).
fn step_key(instance_id: &str, kind: &str, target: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(instance_id.as_bytes());
    hasher.update(b"|");
    hasher.update(kind.as_bytes());
    hasher.update(b"|");
    hasher.update(target.as_bytes());
    hex::encode(hasher.finalize())
}

/// Slugify an instance id for embedding in x0xd topics: lowercase ascii
/// alphanumerics and `-`/`_`, everything else → `-`, trimmed. Guarantees a
/// topic-safe token even for arbitrary caller input.
pub fn instance_slug(instance_id: &str) -> String {
    let mut slug = String::new();
    let mut previous_was_separator = false;
    for character in instance_id.chars() {
        if character.is_ascii_alphanumeric() || character == '_' {
            slug.push(character.to_ascii_lowercase());
            previous_was_separator = false;
        } else if !previous_was_separator && !slug.is_empty() {
            slug.push('-');
            previous_was_separator = true;
        }
        if slug.len() >= 48 {
            break;
        }
    }
    let trimmed = slug.trim_matches('-');
    if trimmed.is_empty() {
        "default".to_string()
    } else {
        trimmed.to_string()
    }
}

/// Short, stable hex suffix (8 chars) derived from the FULL `instance_id`.
///
/// This is the uniqueness anchor: two instances that share a display-name slug
/// (e.g. the same template instantiated twice) produce distinct suffixes, so
/// group names, topics, dirs, and identity keys never collide across them.
pub(crate) fn instance_hash_suffix(instance_id: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(instance_id.as_bytes());
    hex::encode(&hasher.finalize()[..4])
}

/// A filesystem- and identity-safe key derived from the FULL `instance_id`.
///
/// Unlike [`instance_slug`], this NEVER re-truncates, so the unique timestamp
/// (or any trailing qualifier) is preserved. Used for manifest directories and
/// per-role child keys where two instances with the same display name MUST NOT
/// collapse onto one directory or one child handle.
pub fn instance_key(instance_id: &str) -> String {
    let mut key = String::new();
    for character in instance_id.chars() {
        if character.is_ascii_alphanumeric() || character == '_' {
            key.push(character.to_ascii_lowercase());
        } else if !key.ends_with('-') {
            key.push('-');
        }
    }
    let trimmed = key.trim_matches('-');
    if trimmed.is_empty() {
        "default".to_string()
    } else {
        trimmed.to_string()
    }
}

/// A bounded (≤ 48 char), topic-safe fragment derived from the FULL
/// `instance_id`.
///
/// Combines a truncated display slug with [`instance_hash_suffix`] so the
/// fragment is unique per instance even when the display-name slug alone would
/// collide or exceed the topic bound. Used for store/task-list topics where
/// per-instance isolation matters but length must stay reasonable.
pub(crate) fn instance_topic_fragment(instance_id: &str) -> String {
    let display = instance_slug(instance_id);
    let suffix = instance_hash_suffix(instance_id);
    let max_display = 39; // 39 + 1 ('-') + 8 = 48
    let display_part = if display.len() > max_display {
        &display[..max_display]
    } else {
        &display
    };
    format!("{display_part}-{suffix}")
}

/// Company-scoped x0xd store topic (no `x0x.group.<gid>` prefix → no
/// group-local membership requirement, so the daemon identity owns it without
/// crypto). Stable for a given `(instance, kind, local_id)`.
pub(crate) fn store_topic(instance_slug: &str, kind: StoreKind, local_id: &str) -> String {
    format!("ttt.{instance_slug}.store.{kind}.{local_id}")
}

/// Company-scoped x0xd task-list topic. Stable for a given `(instance, id)`.
pub(crate) fn task_list_topic(instance_slug: &str, local_id: &str) -> String {
    format!("ttt.{instance_slug}.tasklist.{local_id}")
}

/// Build the deterministic instantiation plan for `template` under
/// `instance_id`.
///
/// Pure: same inputs → identical output (including generated config/md text).
pub fn plan_instantiation(template: &CompanyTemplate, instance_id: &str) -> InstantiationPlan {
    let fragment = instance_topic_fragment(instance_id);

    let mut steps = Vec::new();

    // 1. Groups — native x0xd groups, in template declaration order. The x0xd
    //    group `name` is the dedup key for ensure_group, so it is suffixed
    //    with the per-instance hash to prevent a second company from adopting
    //    the first company's private groups (display name stays template-side).
    for g in &template.groups {
        steps.push(PlanStep::CreateGroup {
            key: step_key(instance_id, "group", &g.id),
            local_id: g.id.clone(),
            name: format!("{} · {}", g.name, instance_hash_suffix(instance_id)),
            visibility: g.visibility,
            purpose: g.purpose.clone(),
        });
    }

    // 2. Stores — native x0xd stores, in template declaration order.
    for s in &template.stores {
        steps.push(PlanStep::CreateStore {
            key: step_key(instance_id, "store", &s.id),
            local_id: s.id.clone(),
            name: s.name.clone(),
            topic: store_topic(&fragment, s.kind, &s.id),
            kind: s.kind,
            policy: s.policy,
            group_local_id: s.group.clone(),
        });
    }

    // 3. Task lists — native x0xd task-lists.
    for t in &template.task_lists {
        steps.push(PlanStep::CreateTaskList {
            key: step_key(instance_id, "task_list", &t.id),
            local_id: t.id.clone(),
            name: t.name.clone(),
            topic: task_list_topic(&fragment, &t.id),
            group_local_id: t.group.clone(),
        });
    }

    // 4. Symphony config (template/Symphony-layer role roster + store refs).
    steps.push(PlanStep::WriteSymphonyConfig {
        key: step_key(instance_id, "symphony_config", "config"),
    });

    // 5. WORKFLOW.md (Saorsa).
    steps.push(PlanStep::WriteWorkflowMd {
        key: step_key(instance_id, "workflow_md", "workflow"),
    });

    // Generate the deterministic artifacts from the template + the plan-so-far.
    // Built from `template` + `instance_id` only, so they are stable.
    let symphony_config_toml = generate_symphony_config(template, instance_id);
    let workflow_md = {
        // Reconstruct a lightweight plan view for the generator without
        // recursing: workflow generation depends only on template + instance.
        let partial = InstantiationPlan {
            instance_id: instance_id.to_string(),
            template_id: template.id.clone(),
            steps: steps.clone(),
            symphony_config_toml: symphony_config_toml.clone(),
            workflow_md: String::new(),
        };
        generate_workflow_md(template, &partial)
    };

    InstantiationPlan {
        instance_id: instance_id.to_string(),
        template_id: template.id.clone(),
        steps,
        symphony_config_toml,
        workflow_md,
    }
}
