//! Parser + validator for the declarative `CompanyTemplate` TOML format.
//!
//! Parsing is two-phase: deserialize (TOML → [`CompanyTemplate`]) followed by
//! referential validation. Validation is deterministic and side-effect-free.

use std::collections::HashSet;
use std::fmt;

use crate::company_template::spec::CompanyTemplate;
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
    UnknownGroupRef { field: String, group: String },
    UnknownRoleRef { group: String, role: String },
    EmptyHarness { role: String },
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

#[cfg(test)]
pub(crate) fn parse_lenient(src: &str) -> Result<CompanyTemplate, toml::de::Error> {
    CompanyTemplate::deserialize(toml::Deserializer::new(src))
}
