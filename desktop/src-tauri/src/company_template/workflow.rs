//! Saorsa `WORKFLOW.md` generation.
//!
//! Produces a deterministic, human-readable workflow document from a template
//! and its plan. The document describes the instantiated company topology
//! (groups, roles, stores, task lists) and the x0x-symphony pipeline
//! (`backlog → claims → runs → handoffs → proofs`).
//!
//! "Saorsa" is the project's workflow-document brand; the generated file is
//! always named `WORKFLOW.md`. Generation is pure: identical inputs yield an
//! identical document, byte-for-byte.

use crate::company_template::plan::InstantiationPlan;
use crate::company_template::spec::{CompanyTemplate, StoreKind};

/// Generate the Saorsa `WORKFLOW.md` body for `template` under `plan`.
///
/// Deterministic. No I/O, no relay events.
pub fn generate_workflow_md(template: &CompanyTemplate, plan: &InstantiationPlan) -> String {
    let mut out = String::new();

    out.push_str("# WORKFLOW.md\n\n");
    out.push_str(&format!(
        "> Saorsa company workflow — generated from template `{}` (v{}), instance `{}`.\n",
        template.id, template.version, plan.instance_id
    ));
    out.push_str(">\n");
    out.push_str("> This file is regenerated on each instantiation. Do not edit by hand;\n");
    out.push_str("> change the source template and re-instantiate.\n\n");

    if let Some(desc) = &template.description {
        out.push_str(desc.trim());
        out.push_str("\n\n");
    }

    // ── Groups ───────────────────────────────────────────────────────────
    out.push_str("## Groups\n\n");
    out.push_str("| Group | Visibility | Purpose |\n");
    out.push_str("| --- | --- | --- |\n");
    for g in &template.groups {
        let purpose = g.purpose.as_deref().unwrap_or("—");
        out.push_str(&format!(
            "| {} | `{}` | {} |\n",
            g.name, g.visibility, purpose
        ));
    }
    out.push('\n');

    // ── Roles ────────────────────────────────────────────────────────────
    if !template.roles.is_empty() {
        out.push_str("## Roles\n\n");
        out.push_str("| Role | Harness | Skills | Groups |\n");
        out.push_str("| --- | --- | --- | --- |\n");
        for r in &template.roles {
            let skills = if r.skills.is_empty() {
                "—".to_string()
            } else {
                r.skills.join(", ")
            };
            let groups = if r.groups.is_empty() {
                "—".to_string()
            } else {
                r.groups.join(", ")
            };
            out.push_str(&format!(
                "| {} | `{}` | {} | {} |\n",
                r.name, r.harness, skills, groups
            ));
        }
        out.push('\n');
    }

    // ── Symphony stores ──────────────────────────────────────────────────
    if !template.stores.is_empty() {
        out.push_str("## Symphony stores\n\n");
        out.push_str("Typed x0x-symphony stores backing the workflow pipeline. Each is a\n");
        out.push_str("native x0xd store created at instantiation.\n\n");
        out.push_str("| Store | Kind | Policy | Scoped to |\n");
        out.push_str("| --- | --- | --- | --- |\n");
        for s in &template.stores {
            let scoped = s.group.as_deref().unwrap_or("company-shared");
            out.push_str(&format!(
                "| {} | `{}` | `{}` | {} |\n",
                s.name, s.kind, s.policy, scoped
            ));
        }
        out.push('\n');
    }

    // ── Task lists ───────────────────────────────────────────────────────
    if !template.task_lists.is_empty() {
        out.push_str("## Task lists\n\n");
        out.push_str("| Task list | Scoped to |\n");
        out.push_str("| --- | --- |\n");
        for t in &template.task_lists {
            let scoped = t.group.as_deref().unwrap_or("company-shared");
            out.push_str(&format!("| {} | {} |\n", t.name, scoped));
        }
        out.push('\n');
    }

    // ── Pipeline ─────────────────────────────────────────────────────────
    out.push_str("## Workflow pipeline\n\n");
    out.push_str("Work moves through the x0x-symphony stores in this order. Each stage is\n");
    out.push_str("a typed store; the supervised `x0x-symphonyd` consumes claims and emits\n");
    out.push_str("handoffs and proofs.\n\n");
    out.push_str("1. **Backlog** — claimed work items enter here.\n");
    out.push_str("2. **Claims** — a role agent claims a backlog item.\n");
    out.push_str("3. **Runs** — the claimed work executes under supervision.\n");
    out.push_str("4. **Handoffs** — completed runs hand off to the next role/group.\n");
    out.push_str("5. **Proofs** — verifiable proof artifacts are recorded for each run.\n");

    // Note which pipeline stages are realized by this template's stores.
    let present_kinds: Vec<&'static str> = template
        .stores
        .iter()
        .filter_map(|s| match s.kind {
            StoreKind::Backlog => Some("backlog"),
            StoreKind::Claims => Some("claims"),
            StoreKind::Runs => Some("runs"),
            StoreKind::Handoffs => Some("handoffs"),
            StoreKind::Proofs => Some("proofs"),
            StoreKind::Generic => None,
        })
        .collect();
    if !present_kinds.is_empty() {
        out.push_str("\n_Realized stages:_ ");
        out.push_str(&present_kinds.join(", "));
        out.push_str(".\n");
    }

    out.push_str("\n---\n_Generated by the Saorsa company-template backend. No relay events are emitted by this path._\n");

    out
}
