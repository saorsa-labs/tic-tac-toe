//! Declarative `CompanyTemplate` format.
//!
//! A `CompanyTemplate` is a portable description of a company's x0x-native
//! topology: groups (Engineering / Sales / All-Hands), the typed Symphony
//! stores they share (backlog / claims / runs / handoffs / proofs), shared
//! task lists, and the role agents that staff them.
//!
//! Every field maps to a **native x0xd** resource or to a Symphony-layer
//! concern — never to a Nostr relay event. Group visibility maps 1:1 to an
//! x0xd group `preset`; store kinds map to x0x-symphony's typed stores. The
//! `harness`/`skills` on a [`RoleSpec`] are a template/Symphony-layer concern:
//! x0xd `AgentCard`s carry no skills/harness, so roles are realized in the
//! generated Symphony config, not as x0xd group members (direct member-add to
//! a `private_secure` group would require a TreeKEM key package, which is
//! crypto and out of M4 scope).

use serde::{Deserialize, Serialize};

/// Group visibility — maps 1:1 to a native x0xd group `preset`.
///
/// - [`GroupVisibility::PrivateSecure`] → `private_secure`:
///   Hidden / InviteOnly / MLS-encrypted (TreeKEM), secure-by-default.
/// - [`GroupVisibility::PublicOpen`] → `public_open`:
///   PublicDirectory / OpenJoin / SignedPublic (the channel preset).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GroupVisibility {
    PrivateSecure,
    PublicOpen,
}

impl GroupVisibility {
    /// The x0xd `preset` string passed to `POST /groups`.
    pub fn as_x0xd_preset(self) -> &'static str {
        match self {
            GroupVisibility::PrivateSecure => "private_secure",
            GroupVisibility::PublicOpen => "public_open",
        }
    }
}

impl std::fmt::Display for GroupVisibility {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_x0xd_preset())
    }
}

/// Typed x0x-symphony store kind. The first five are the canonical Symphony
/// workflow stores; [`StoreKind::Generic`] is a free-form typed store.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StoreKind {
    Backlog,
    Claims,
    Runs,
    Handoffs,
    Proofs,
    Generic,
}

impl StoreKind {
    /// Stable lowercase wire string used in derived store topics and in the
    /// generated Symphony config.
    pub fn as_str(self) -> &'static str {
        match self {
            StoreKind::Backlog => "backlog",
            StoreKind::Claims => "claims",
            StoreKind::Runs => "runs",
            StoreKind::Handoffs => "handoffs",
            StoreKind::Proofs => "proofs",
            StoreKind::Generic => "generic",
        }
    }
}

impl std::fmt::Display for StoreKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// x0xd store write policy. Maps to the `policy` field on `POST /stores`.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StorePolicy {
    /// `signed` (the x0xd default): entries are ML-DSA signed by the writer.
    #[default]
    Signed,
    /// `append_only`: no mutation/deletion of prior entries.
    AppendOnly,
}

impl StorePolicy {
    pub fn as_x0xd_policy(self) -> &'static str {
        match self {
            StorePolicy::Signed => "signed",
            StorePolicy::AppendOnly => "append_only",
        }
    }
}

impl std::fmt::Display for StorePolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_x0xd_policy())
    }
}

/// The single supported Symphony runner preset a role `harness` maps to.
///
/// One supervised `x0x-symphonyd` resolves exactly ONE `RunnerSpec` (one
/// command/args/env preset) and runs every claimed issue through it — there is
/// no per-role or per-agent routing. Company therefore supports a single
/// uniform runner across its role roster; `validate_supported_contract`
/// enforces that every role normalizes to the same preset, replacing the old
/// silent first-role-wins collapse with a visible, fail-closed invariant.
///
/// This is the canonical mapping (also used by Symphony config generation); it
/// lives in the lowest template module so both validation and config
/// generation agree on one notion of "supported runner".
pub(crate) fn runner_preset(harness: &str) -> &'static str {
    match harness.trim().to_ascii_lowercase().as_str() {
        "claude" | "claude-code" | "claude_code" => "claude_code",
        "codex" => "codex",
        "kimi" => "kimi",
        "glm" => "glm",
        "minimax" => "minimax",
        _ => "pi",
    }
}

fn default_version() -> String {
    "1".to_string()
}

/// A role agent declared by the template.
///
/// `harness` is the runtime harness id (`codex` | `claude` | `goose` |
/// `buzz`); `skills` are skill identifiers. These are a **template /
/// Symphony-layer** concern: they are written into the generated Symphony
/// config, not into an x0xd `AgentCard` (which carries no skills/harness).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoleSpec {
    /// Stable local id, unique within the template (e.g. `staff-engineer`).
    pub id: String,
    /// Human-readable name (e.g. `Staff Engineer`).
    pub name: String,
    /// Runtime harness id (`codex` | `claude` | `goose` | `buzz`).
    pub harness: String,
    /// Skill identifiers (e.g. `code-review`, `git-ops`).
    #[serde(default)]
    pub skills: Vec<String>,
    /// Optional model override for the harness.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Group ids this role staffs.
    #[serde(default)]
    pub groups: Vec<String>,
}

/// A group declared by the template. Created as a native x0xd group.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupSpec {
    /// Stable local id, unique within the template (e.g. `engineering`).
    pub id: String,
    /// Display name (becomes the x0xd group `display_name` / `name`).
    pub name: String,
    pub visibility: GroupVisibility,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub purpose: Option<String>,
    /// Role ids that staff this group (declared, not realized as x0xd members
    /// in M4 — `private_secure` direct-add needs a TreeKEM key package).
    #[serde(default)]
    pub members: Vec<String>,
}

/// A typed Symphony store. Created as a native x0xd store (`POST /stores`).
///
/// A store with `group = None` is **company-shared**; one with `group = Some`
/// is logically scoped to that group. In M4 both are created as
/// company-scoped x0xd topics (the creator — the daemon identity — owns them),
/// avoiding the group-local-membership requirement entirely.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoreSpec {
    /// Stable local id, unique within the template.
    pub id: String,
    /// Human-readable name.
    pub name: String,
    pub kind: StoreKind,
    #[serde(default)]
    pub policy: StorePolicy,
    /// Group id this store is logically scoped to; `None` = company-shared.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
}

/// A shared task list. Created as a native x0xd task list (`POST /task-lists`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskListSpec {
    /// Stable local id, unique within the template.
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// Group id this task list is logically scoped to; `None` = company-shared.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
}

/// A declarative, portable company topology. Parsed from TOML by
/// [`crate::company_template::parse::parse_company_template`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompanyTemplate {
    /// Stable template id (e.g. `software-dev-and-sales`).
    pub id: String,
    /// Human-readable template name.
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default = "default_version")]
    pub version: String,
    #[serde(default)]
    pub groups: Vec<GroupSpec>,
    #[serde(default)]
    pub roles: Vec<RoleSpec>,
    #[serde(default)]
    pub stores: Vec<StoreSpec>,
    #[serde(default)]
    pub task_lists: Vec<TaskListSpec>,
}

impl CompanyTemplate {
    /// Find a group by local id.
    pub fn group(&self, id: &str) -> Option<&GroupSpec> {
        self.groups.iter().find(|g| g.id == id)
    }

    /// Find a role by local id.
    pub fn role(&self, id: &str) -> Option<&RoleSpec> {
        self.roles.iter().find(|r| r.id == id)
    }

    #[cfg(test)]
    pub fn group_count(&self) -> usize {
        self.groups.len()
    }
    #[cfg(test)]
    pub fn role_count(&self) -> usize {
        self.roles.len()
    }
    #[cfg(test)]
    pub fn store_count(&self) -> usize {
        self.stores.len()
    }
    #[cfg(test)]
    pub fn task_list_count(&self) -> usize {
        self.task_lists.len()
    }
}

/// A compact summary for registry listings.
#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TemplateSummary {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub version: String,
    pub group_count: usize,
    pub role_count: usize,
    pub store_count: usize,
    pub task_list_count: usize,
}

#[cfg(test)]
impl From<&CompanyTemplate> for TemplateSummary {
    fn from(t: &CompanyTemplate) -> Self {
        TemplateSummary {
            id: t.id.clone(),
            name: t.name.clone(),
            description: t.description.clone(),
            version: t.version.clone(),
            group_count: t.groups.len(),
            role_count: t.roles.len(),
            store_count: t.stores.len(),
            task_list_count: t.task_lists.len(),
        }
    }
}
