//! Parser + validator for the declarative `CompanyTemplate` TOML format.
//!
//! Parsing is two-phase: deserialize (TOML → [`CompanyTemplate`]) followed by
//! referential validation. Validation is deterministic and side-effect-free.

use std::collections::HashSet;
use std::fmt;

use crate::company_template::spec::{CompanyTemplate, GroupVisibility};
#[cfg(test)]
use serde::Deserialize;

/// A parse or validation error. Deterministic: the same input always yields
/// the same variant + message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    /// The TOML could not be deserialized into the template shape.
    Decode(String),
    /// Referential / structural validation failed.
    Invalid(ValidationIssue),
}

/// Structured validation diagnostics, so callers can assert on kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidationIssue {
    EmptyTemplateId,
    EmptyGroupName,
    DuplicateGroupId(String),
    DuplicateRoleId(String),
    DuplicateStoreId(String),
    DuplicateTaskListId(String),
    UnknownGroupRef {
        field: String,
        group: String,
    },
    UnknownRoleRef {
        group: String,
        role: String,
    },
    EmptyHarness {
        role: String,
    },
    /// Template declares more than one distinct Symphony runner preset. One
    /// supervised `x0x-symphonyd` resolves a single runner and routes every
    /// issue through it, so a multi-harness roster is rejected (never silently
    /// collapsed to the first role's harness).
    MultipleHarnessesUnsupported {
        harnesses: Vec<String>,
    },
    /// The template must declare exactly one company-shared (`group = None`)
    /// task list — Symphony's single consumed backlog. `count` is the number
    /// found (0 or >1 are both rejected).
    PrimaryTaskListContract {
        count: usize,
    },
    /// The company-shared task list is not declared first, so Symphony would
    /// bind a group-scoped list as its backlog.
    PrimaryTaskListNotFirst,
    /// A group declares a visibility Company cannot provision against the
    /// current production x0xd group API. Production `POST /groups` accepts
    /// only the `public_open` preset; a `private_secure` group would fail
    /// mid-provisioning (after reservation) with an opaque daemon error, so it
    /// is rejected at the contract boundary before any reservation or
    /// provisioning. (Secure groups are not creatable in this build.)
    UnsupportedGroupVisibility {
        group: String,
        visibility: String,
    },
}

impl fmt::Display for ValidationIssue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ValidationIssue::EmptyTemplateId => write!(f, "template id must be non-empty"),
            ValidationIssue::EmptyGroupName => write!(f, "group names must be non-empty"),
            ValidationIssue::DuplicateGroupId(id) => {
                write!(f, "duplicate group id `{id}`")
            }
            ValidationIssue::DuplicateRoleId(id) => write!(f, "duplicate role id `{id}`"),
            ValidationIssue::DuplicateStoreId(id) => {
                write!(f, "duplicate store id `{id}`")
            }
            ValidationIssue::DuplicateTaskListId(id) => {
                write!(f, "duplicate task-list id `{id}`")
            }
            ValidationIssue::UnknownGroupRef { field, group } => {
                write!(f, "{field} references unknown group `{group}`")
            }
            ValidationIssue::UnknownRoleRef { group, role } => {
                write!(f, "group `{group}` references unknown role `{role}`")
            }
            ValidationIssue::EmptyHarness { role } => {
                write!(f, "role `{role}` has an empty harness")
            }
            ValidationIssue::MultipleHarnessesUnsupported { harnesses } => {
                write!(
                    f,
                    "template declares multiple Symphony runner harnesses {harnesses:?}; exactly one uniform runner is supported"
                )
            }
            ValidationIssue::PrimaryTaskListContract { count } => {
                write!(
                    f,
                    "template must declare exactly one company-shared (primary) task list; found {count}"
                )
            }
            ValidationIssue::PrimaryTaskListNotFirst => {
                write!(
                    f,
                    "the company-shared (primary) task list must be declared first so Symphony binds it as the backlog"
                )
            }
            ValidationIssue::UnsupportedGroupVisibility { group, visibility } => {
                write!(
                    f,
                    "group `{group}` declares unsupported visibility `{visibility}`; Company provisioning only supports `public_open` groups"
                )
            }
        }
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ParseError::Decode(msg) => write!(f, "template decode failed: {msg}"),
            ParseError::Invalid(issue) => write!(f, "template invalid: {issue}"),
        }
    }
}

impl std::error::Error for ParseError {}

/// Parse and validate a `CompanyTemplate` from its TOML text.
///
/// Deterministic: identical input yields an identical `Ok` template or an
/// identical `Err`. No I/O, no relay events.
pub fn parse_company_template(src: &str) -> Result<CompanyTemplate, ParseError> {
    let template =
        toml::from_str::<CompanyTemplate>(src).map_err(|e| ParseError::Decode(e.to_string()))?;
    validate(&template)?;
    Ok(template)
}

/// Referential + structural validation. Deterministic.
pub fn validate(template: &CompanyTemplate) -> Result<(), ParseError> {
    if template.id.trim().is_empty() {
        return Err(ParseError::Invalid(ValidationIssue::EmptyTemplateId));
    }

    let mut seen = HashSet::new();
    for g in &template.groups {
        if g.name.trim().is_empty() {
            return Err(ParseError::Invalid(ValidationIssue::EmptyGroupName));
        }
        if !seen.insert(g.id.as_str()) {
            return Err(ParseError::Invalid(ValidationIssue::DuplicateGroupId(
                g.id.clone(),
            )));
        }
    }
    let group_ids = seen.clone();

    let mut role_ids = HashSet::new();
    for r in &template.roles {
        if r.harness.trim().is_empty() {
            return Err(ParseError::Invalid(ValidationIssue::EmptyHarness {
                role: r.id.clone(),
            }));
        }
        if !role_ids.insert(r.id.as_str()) {
            return Err(ParseError::Invalid(ValidationIssue::DuplicateRoleId(
                r.id.clone(),
            )));
        }
    }

    // group.members → roles must exist.
    for g in &template.groups {
        for m in &g.members {
            if !role_ids.contains(m.as_str()) {
                return Err(ParseError::Invalid(ValidationIssue::UnknownRoleRef {
                    group: g.id.clone(),
                    role: m.clone(),
                }));
            }
        }
    }
    // roles.groups → groups must exist.
    for r in &template.roles {
        for gid in &r.groups {
            if !group_ids.contains(gid.as_str()) {
                return Err(ParseError::Invalid(ValidationIssue::UnknownGroupRef {
                    field: format!("role `{}`.groups", r.id),
                    group: gid.clone(),
                }));
            }
        }
    }

    let mut store_ids = HashSet::new();
    for s in &template.stores {
        if !store_ids.insert(s.id.as_str()) {
            return Err(ParseError::Invalid(ValidationIssue::DuplicateStoreId(
                s.id.clone(),
            )));
        }
        if let Some(gid) = &s.group {
            if !group_ids.contains(gid.as_str()) {
                return Err(ParseError::Invalid(ValidationIssue::UnknownGroupRef {
                    field: format!("store `{}`.group", s.id),
                    group: gid.clone(),
                }));
            }
        }
    }

    let mut task_list_ids = HashSet::new();
    for t in &template.task_lists {
        if !task_list_ids.insert(t.id.as_str()) {
            return Err(ParseError::Invalid(ValidationIssue::DuplicateTaskListId(
                t.id.clone(),
            )));
        }
        if let Some(gid) = &t.group {
            if !group_ids.contains(gid.as_str()) {
                return Err(ParseError::Invalid(ValidationIssue::UnknownGroupRef {
                    field: format!("task-list `{}`.group", t.id),
                    group: gid.clone(),
                }));
            }
        }
    }

    Ok(())
}

/// Validate the supported-runtime contract a Company template must satisfy
/// before the supervised Symphony daemon can run it.
///
/// Enforced at the production boundary (instantiation command), NOT in
/// [`validate`]: this encodes Symphony-daemon runtime constraints (single
/// uniform runner, single primary task queue), not template well-formedness.
/// Keeping it separate lets curated fixtures parse freely while the shipping
/// path still fails closed.
///
/// Contract:
/// 1. **Single uniform runner.** Every role's `harness` normalizes (via
///    [`crate::company_template::spec::runner_preset`]) to the SAME preset.
///    One supervised daemon resolves one runner and routes every issue through
///    it; per-role routing does not exist, so a multi-harness roster is
///    rejected instead of silently collapsing to the first.
/// 2. **One primary task queue.** Exactly one company-shared (`group = None`)
///    task list exists and is declared first — the single backlog Symphony
///    binds (`tracker.list_id = task_lists.first()`).
/// 3. **Public groups only.** Every group is `public_open`. Production
///    `POST /groups` accepts only that preset; a `private_secure` group would
///    fail mid-provisioning (after reservation) with an opaque daemon error,
///    so it is rejected here, before reservation.
pub fn validate_supported_contract(template: &CompanyTemplate) -> Result<(), ParseError> {
    // 1. Single uniform runner: every role normalizes to one preset.
    let mut distinct: Vec<String> = Vec::new();
    for role in &template.roles {
        let preset = crate::company_template::spec::runner_preset(&role.harness).to_string();
        if !distinct.contains(&preset) {
            distinct.push(preset);
        }
    }
    distinct.sort();
    if distinct.len() > 1 {
        return Err(ParseError::Invalid(
            ValidationIssue::MultipleHarnessesUnsupported {
                harnesses: distinct,
            },
        ));
    }

    // 2. Exactly one primary (company-shared) task list, declared first.
    let primary_count = template
        .task_lists
        .iter()
        .filter(|t| t.group.is_none())
        .count();
    if primary_count != 1 {
        return Err(ParseError::Invalid(
            ValidationIssue::PrimaryTaskListContract {
                count: primary_count,
            },
        ));
    }
    let first_is_group_scoped = template
        .task_lists
        .first()
        .is_some_and(|t| t.group.is_some());
    if first_is_group_scoped {
        return Err(ParseError::Invalid(
            ValidationIssue::PrimaryTaskListNotFirst,
        ));
    }

    // 3. Public groups only. Production `POST /groups` accepts only the
    //    `public_open` preset; a `private_secure` group would fail mid-
    //    provisioning (after reservation) with an opaque daemon error.
    //    Rejected here, before reservation, with a clear message.
    for group in &template.groups {
        if group.visibility != GroupVisibility::PublicOpen {
            return Err(ParseError::Invalid(
                ValidationIssue::UnsupportedGroupVisibility {
                    group: group.id.clone(),
                    visibility: group.visibility.as_x0xd_preset().to_string(),
                },
            ));
        }
    }

    Ok(())
}

#[cfg(test)]
pub(crate) fn parse_lenient(src: &str) -> Result<CompanyTemplate, toml::de::Error> {
    CompanyTemplate::deserialize(toml::Deserializer::new(src))
}
