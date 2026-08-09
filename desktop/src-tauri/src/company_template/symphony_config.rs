//! Valid `x0x-symphonyd` `WORKFLOW.md` generation for a Company instance.
//!
//! The generated frontmatter binds Symphony directly to the native x0xd task
//! list and signing endpoint. It contains no relay configuration or fallback.

use crate::company_template::plan::{instance_topic_fragment, task_list_topic};
use crate::company_template::spec::CompanyTemplate;

/// Generate a deterministic workflow using the daemon's conventional loopback
/// endpoint. Production callers should use [`generate_symphony_config_for_x0xd`]
/// with the supervised x0xd endpoint resolved from its artifacts.
pub fn generate_symphony_config(template: &CompanyTemplate, instance_id: &str) -> String {
    generate_symphony_config_for_x0xd(template, instance_id, "http://127.0.0.1:12700")
}

/// Generate the complete Symphony `WORKFLOW.md` for one Company instance.
///
/// `x0xd_url` must be the supervised loopback daemon URL. The bearer token is
/// deliberately absent: the supervisor injects it transiently through
/// `X0X_API_TOKEN` when spawning `x0x-symphonyd`.
pub fn generate_symphony_config_for_x0xd(
    template: &CompanyTemplate,
    instance_id: &str,
    x0xd_url: &str,
) -> String {
    let fragment = instance_topic_fragment(instance_id);
    // Single consumed backlog: Symphony binds exactly ONE task list
    // (`tracker.list_id`). `validate_supported_contract` guarantees the first
    // task list is the company-shared (primary) one, so `task_lists.first()`
    // is the Symphony backlog; any other task lists are auxiliary x0xd
    // resources the UI may surface independently.
    let task_list_id = template
        .task_lists
        .first()
        .map(|task_list| task_list_topic(&fragment, &task_list.id))
        .unwrap_or_else(|| task_list_topic(&fragment, "company-tasks"));
    let workspace_root = format!("~/.x0x-company/{fragment}/workspaces");
    // Single uniform runner: one supervised daemon resolves one RunnerSpec and
    // routes every issue through it (no per-role routing). Every role shares
    // one harness (enforced by `validate_supported_contract`), so
    // `roles.first()` is the whole roster's preset — never a silent
    // first-role-wins collapse.
    let runner_preset = template
        .roles
        .first()
        .map(|role| crate::company_template::spec::runner_preset(&role.harness))
        .unwrap_or("pi");
    let mut out = String::new();
    out.push_str("---\n");
    out.push_str("tracker:\n  kind: x0x_crdt\n");
    out.push_str(&format!("  list_id: {}\n", yaml_scalar(&task_list_id)));
    out.push_str("  active_states: [todo, in_progress]\n");
    out.push_str("  terminal_states: [done, cancelled, duplicate]\n");
    out.push_str("polling:\n  interval_ms: 2000\n");
    out.push_str(&format!(
        "workspace:\n  root: {}\n",
        yaml_scalar(&workspace_root)
    ));
    out.push_str("hooks:\n  timeout_ms: 120000\n");
    out.push_str("agent:\n  max_concurrent_agents: 4\n");
    out.push_str("  max_concurrent_agents_by_state:\n    todo: 2\n    in_progress: 2\n");
    out.push_str("  max_turns: 8\n  max_retry_backoff_ms: 300000\n");
    out.push_str("security:\n  network_dispatch: approve\n  required_trust: trusted\n");
    out.push_str("signing:\n  policy: required\n");
    out.push_str(&format!("  x0xd_url: {}\n", yaml_scalar(x0xd_url)));
    out.push_str("workers:\n  publish_enabled: true\n  ttl_seconds: 60\n");
    out.push_str("runner:\n  kind: shell\n");
    out.push_str(&format!("  preset: {}\n", yaml_scalar(runner_preset)));
    out.push_str("  approval_policy: untrusted\n");
    out.push_str("  turn_timeout_ms: 3600000\n  read_timeout_ms: 5000\n");
    out.push_str("  stall_timeout_ms: 300000\n---\n");
    out.push_str(&format!("# {}\n\n", template.name));
    if let Some(description) = &template.description {
        out.push_str(description.trim());
        out.push_str("\n\n");
    }
    out.push_str("You are working on `{{ issue.identifier }}`: **{{ issue.title }}**.\n\n");
    out.push_str("{{ issue.description }}\n\n");
    out.push_str("Complete the task, record a handoff, and attach concise proof of the result.\n");
    out
}

/// JSON string syntax is a valid YAML quoted scalar and gives deterministic,
/// injection-safe escaping without a second serializer dependency.
fn yaml_scalar(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "\"\"".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_is_valid_frontmatter_and_deterministic() {
        let template = crate::company_template::builtin::software_dev_and_sales();
        let a = generate_symphony_config_for_x0xd(&template, "acme-2026", "http://127.0.0.1:49152");
        let b = generate_symphony_config_for_x0xd(&template, "acme-2026", "http://127.0.0.1:49152");
        assert_eq!(a, b);
        assert!(a.starts_with("---\ntracker:\n"));
        assert!(
            a.contains("list_id: \"ttt.acme-2026-") && a.contains(".tasklist.company-tasks\""),
            "list_id must use instance-scoped fragment: {a}"
        );
        assert!(a.contains("x0xd_url: \"http://127.0.0.1:49152\""));
        assert!(!a.contains("relay"));
    }
}
