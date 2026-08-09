//! Built-in company templates.
//!
//! Templates are embedded as TOML text and parsed once (validated) via a
//! [`std::sync::LazyLock`]. The canonical M4 slice is
//! [`software_dev_and_sales`]: Engineering, Sales, and All-Hands as native
//! `public_open` x0xd groups (M4 realizes roles identity-only — no
//! TreeKEM/MLS member-add), with the canonical Symphony pipeline stores
//! (backlog / claims / runs / handoffs / proofs) shared company-wide plus
//! group-scoped extras, a shared task list, and a single-harness role roster
//! driven by one supported Symphony runner preset.

#[cfg(test)]
use std::sync::LazyLock;

#[cfg(test)]
use crate::company_template::parse::parse_company_template;
#[cfg(test)]
use crate::company_template::spec::CompanyTemplate;

/// The `software-dev-and-sales` company template (TOML).
///
/// Groups: Engineering, Sales, and All-Hands are all native `public_open`
/// x0xd groups — M4 realizes roles identity-only (no secure/TreeKEM member
/// add). Every role shares one supported Symphony runner preset (a single
/// uniform runner). The canonical Symphony pipeline stores are company-shared;
/// group-scoped stores are extras.
pub const SOFTWARE_DEV_AND_SALES_TOML: &str = r#"
id = "software-dev-and-sales"
name = "Software Dev & Sales"
description = "A hybrid engineering + sales company with an All-Hands room."
version = "1"

# ── Groups ────────────────────────────────────────────────────────────────
[[groups]]
id = "engineering"
name = "Engineering"
visibility = "public_open"
purpose = "Product engineering, code review, and releases."
members = ["staff-engineer", "frontend-engineer"]

[[groups]]
id = "sales"
name = "Sales"
visibility = "public_open"
purpose = "Discovery, demos, and account management."
members = ["sales-engineer", "account-executive"]

[[groups]]
id = "all-hands"
name = "All-Hands"
visibility = "public_open"
purpose = "Company-wide announcements and cross-team coordination."
members = ["staff-engineer", "frontend-engineer", "sales-engineer", "account-executive"]

# ── Roles (harness + skills — Symphony-layer, not x0xd AgentCards) ────────
[[roles]]
id = "staff-engineer"
name = "Staff Engineer"
harness = "codex"
skills = ["code-review", "git-ops", "architecture"]
groups = ["engineering", "all-hands"]

[[roles]]
id = "frontend-engineer"
name = "Frontend Engineer"
harness = "codex"
skills = ["ui", "a11y", "design-systems"]
groups = ["engineering", "all-hands"]

[[roles]]
id = "sales-engineer"
name = "Sales Engineer"
harness = "codex"
skills = ["discovery", "demos", "solutions"]
groups = ["sales", "all-hands"]

[[roles]]
id = "account-executive"
name = "Account Executive"
harness = "codex"
skills = ["crm", "outreach", "forecasting"]
groups = ["sales", "all-hands"]

# ── Stores: canonical Symphony pipeline (company-shared come first) ───────
[[stores]]
id = "company-backlog"
name = "Company Backlog"
kind = "backlog"
policy = "append_only"

[[stores]]
id = "company-claims"
name = "Company Claims"
kind = "claims"

[[stores]]
id = "company-runs"
name = "Company Runs"
kind = "runs"

[[stores]]
id = "company-handoffs"
name = "Company Handoffs"
kind = "handoffs"

[[stores]]
id = "company-proofs"
name = "Company Proofs"
kind = "proofs"
policy = "append_only"

# Group-scoped extras.
[[stores]]
id = "engineering-backlog"
name = "Engineering Backlog"
kind = "backlog"
group = "engineering"

[[stores]]
id = "engineering-proofs"
name = "Engineering Proofs"
kind = "proofs"
group = "engineering"

[[stores]]
id = "sales-backlog"
name = "Sales Backlog"
kind = "backlog"
group = "sales"

# ── Task lists ─────────────────────────────────────────────────────────────
[[task_lists]]
id = "company-tasks"
name = "Company Task List"

[[task_lists]]
id = "engineering-tasks"
name = "Engineering Tasks"
group = "engineering"
"#;

#[cfg(test)]
static SOFTWARE_DEV_AND_SALES: LazyLock<CompanyTemplate> = LazyLock::new(|| {
    parse_company_template(SOFTWARE_DEV_AND_SALES_TOML)
        .expect("builtin software-dev-and-sales template must parse and validate")
});

/// The parsed, validated `software-dev-and-sales` company template.
#[cfg(test)]
pub fn software_dev_and_sales() -> CompanyTemplate {
    SOFTWARE_DEV_AND_SALES.clone()
}

/// `(id, toml)` pairs for every built-in template.
pub fn builtin_templates() -> Vec<(&'static str, &'static str)> {
    vec![("software-dev-and-sales", SOFTWARE_DEV_AND_SALES_TOML)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_template_parses_and_has_expected_topology() {
        let tpl = software_dev_and_sales();
        assert_eq!(tpl.id, "software-dev-and-sales");
        // M4 public-only contract: every shipping group is public_open
        // (Engineering + Sales + All-Hands) — validate_supported_contract
        // rejects any non-public group before reservation, so the builtin the
        // production gate runs against must itself be all-public.
        assert_eq!(tpl.groups.len(), 3);
        for group in &tpl.groups {
            assert_eq!(
                group.visibility,
                crate::company_template::spec::GroupVisibility::PublicOpen,
                "shipping group `{}` must be public_open",
                group.id
            );
        }
        // Canonical pipeline stores present.
        let kinds: Vec<_> = tpl.stores.iter().map(|s| s.kind).collect();
        assert!(kinds.contains(&crate::company_template::spec::StoreKind::Backlog));
        assert!(kinds.contains(&crate::company_template::spec::StoreKind::Claims));
        assert!(kinds.contains(&crate::company_template::spec::StoreKind::Runs));
        assert!(kinds.contains(&crate::company_template::spec::StoreKind::Handoffs));
        assert!(kinds.contains(&crate::company_template::spec::StoreKind::Proofs));
        // Shared task list.
        assert!(tpl
            .task_lists
            .iter()
            .any(|t| t.id == "company-tasks" && t.group.is_none()));
        // Roles have harness + skills.
        let staff = tpl.role("staff-engineer").unwrap();
        assert_eq!(staff.harness, "codex");
        assert!(!staff.skills.is_empty());
    }
}
