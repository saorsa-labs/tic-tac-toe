use std::path::Path;

use tauri::Manager as _;

use super::{canonical_dev_data_dir, patch_json_records};

/// Normalize local agent state imported from Buzz releases whose default
/// worker count was 24. The native x0x ACP transport exposes one ordered
/// prompt stream, so both definition defaults and materialized instances must
/// carry the only supported value before launch restore.
///
/// Missing values already deserialize through the current default of one and
/// are left absent. Explicit non-legacy values, provider-backed records, and
/// malformed state are preserved so the normal typed/runtime validation still
/// fails loudly instead of this compatibility migration changing intent.
pub(super) fn reconcile_native_agent_parallelism_in_file(path: &Path) {
    patch_json_records(path, |obj| {
        let is_definition = obj
            .get("pubkey")
            .and_then(serde_json::Value::as_str)
            .is_none_or(str::is_empty);
        let is_local_instance = obj.get("backend").is_none_or(|backend| {
            backend.get("type").and_then(serde_json::Value::as_str) == Some("local")
        });
        if !is_definition && !is_local_instance {
            return false;
        }

        let mut changed = false;
        for field in ["parallelism", "definition_parallelism"] {
            const LEGACY_AGENT_PARALLELISM: u64 = 24;
            if obj.get(field).and_then(serde_json::Value::as_u64) != Some(LEGACY_AGENT_PARALLELISM)
            {
                continue;
            }
            obj.insert(
                field.to_string(),
                serde_json::Value::from(crate::managed_agents::DEFAULT_AGENT_PARALLELISM),
            );
            changed = true;
        }
        changed
    });
}

/// Reconcile legacy multi-worker agent records in both the active app-data
/// directory and the canonical shared dev directory. This runs after persona
/// folding/backfill so every definition and instance is present, and before
/// launch restore validates the native one-worker contract.
pub(super) fn reconcile_native_agent_parallelism(app: &tauri::AppHandle) {
    let Ok(current_dir) = app.path().app_data_dir() else {
        return;
    };
    let mut dirs = vec![current_dir.clone()];
    if let Some(canonical) = canonical_dev_data_dir(&current_dir) {
        if canonical.exists() && canonical != current_dir {
            dirs.push(canonical);
        }
    }
    for dir in dirs {
        let path = dir.join("agents/managed-agents.json");
        if path.exists() {
            reconcile_native_agent_parallelism_in_file(&path);
        }
    }
}
