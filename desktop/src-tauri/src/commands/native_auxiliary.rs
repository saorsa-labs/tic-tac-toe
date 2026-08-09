//! Native x0xd auxiliary-data commands for M3.
//!
//! This module is deliberately thin: it preserves the daemon's JSON response
//! envelopes while enforcing authenticated loopback transport through
//! [`AppState::x0x_client`]. Task lists, KV stores, and AgentCards never fall
//! back to relay/Nostr commands when x0xd is unavailable.

use serde_json::{json, Value};
use tauri::State;

use crate::{app_state::AppState, x0x_client::X0xClientError};

/// Percent-encode one dynamic URL path segment. Identifiers and KV keys are
/// opaque application data; allowing `/`, `?`, or `#` through would change the
/// daemon route rather than address the requested object.
pub(crate) fn path_segment(value: &str) -> Result<String, String> {
    if value.is_empty() {
        return Err("x0xd path identifier must not be empty".to_string());
    }
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(char::from(byte));
        } else {
            use std::fmt::Write as _;
            write!(&mut encoded, "%{byte:02X}")
                .map_err(|_| "failed to encode x0xd path identifier".to_string())?;
        }
    }
    Ok(encoded)
}

// ── Collaborative task lists ───────────────────────────────────────────────

#[tauri::command]
pub async fn x0x_list_task_lists(state: State<'_, AppState>) -> Result<Value, String> {
    state
        .x0x_client
        .get_json("/task-lists", &[])
        .await
        .map_err(Into::into)
}

#[tauri::command]
pub async fn x0x_create_task_list(
    state: State<'_, AppState>,
    name: String,
    topic: String,
) -> Result<Value, String> {
    state
        .x0x_client
        .post_json("/task-lists", &json!({ "name": name, "topic": topic }))
        .await
        .map_err(Into::into)
}

#[tauri::command]
pub async fn x0x_list_tasks(state: State<'_, AppState>, list_id: String) -> Result<Value, String> {
    let id = path_segment(&list_id)?;
    state
        .x0x_client
        .get_json(&format!("/task-lists/{id}/tasks"), &[])
        .await
        .map_err(Into::into)
}

#[tauri::command]
pub async fn x0x_add_task(
    state: State<'_, AppState>,
    list_id: String,
    title: String,
    description: Option<String>,
) -> Result<Value, String> {
    let id = path_segment(&list_id)?;
    state
        .x0x_client
        .post_json(
            &format!("/task-lists/{id}/tasks"),
            &json!({ "title": title, "description": description }),
        )
        .await
        .map_err(Into::into)
}

#[tauri::command]
pub async fn x0x_update_task(
    state: State<'_, AppState>,
    list_id: String,
    task_id: String,
    action: String,
    fence_token: Option<String>,
) -> Result<Value, String> {
    if !matches!(action.as_str(), "claim" | "complete") {
        return Err("task action must be 'claim' or 'complete'".to_string());
    }
    let list = path_segment(&list_id)?;
    let task = path_segment(&task_id)?;
    state
        .x0x_client
        .patch_json(
            &format!("/task-lists/{list}/tasks/{task}"),
            &json!({ "action": action, "fence_token": fence_token }),
        )
        .await
        .map_err(Into::into)
}

// ── Key-value stores ───────────────────────────────────────────────────────

#[tauri::command]
pub async fn x0x_list_stores(state: State<'_, AppState>) -> Result<Value, String> {
    state
        .x0x_client
        .get_json("/stores", &[])
        .await
        .map_err(Into::into)
}

#[tauri::command]
pub async fn x0x_create_store(
    state: State<'_, AppState>,
    name: String,
    topic: String,
    policy: Option<String>,
) -> Result<Value, String> {
    if policy
        .as_deref()
        .is_some_and(|value| !matches!(value, "signed" | "append_only"))
    {
        return Err("store policy must be 'signed' or 'append_only'".to_string());
    }
    state
        .x0x_client
        .post_json(
            "/stores",
            &json!({ "name": name, "topic": topic, "policy": policy }),
        )
        .await
        .map_err(Into::into)
}

#[tauri::command]
pub async fn x0x_join_store(
    state: State<'_, AppState>,
    store_id: String,
    expected_owner: String,
) -> Result<Value, String> {
    let id = path_segment(&store_id)?;
    state
        .x0x_client
        .post_json(
            &format!("/stores/{id}/join"),
            &json!({ "expected_owner": expected_owner }),
        )
        .await
        .map_err(Into::into)
}

#[tauri::command]
pub async fn x0x_list_store_keys(
    state: State<'_, AppState>,
    store_id: String,
) -> Result<Value, String> {
    let id = path_segment(&store_id)?;
    state
        .x0x_client
        .get_json(&format!("/stores/{id}/keys"), &[])
        .await
        .map_err(Into::into)
}

#[tauri::command]
pub async fn x0x_get_store_value(
    state: State<'_, AppState>,
    store_id: String,
    key: String,
) -> Result<Option<Value>, String> {
    let id = path_segment(&store_id)?;
    let key = path_segment(&key)?;
    match state
        .x0x_client
        .get_json(&format!("/stores/{id}/{key}"), &[])
        .await
    {
        Ok(value) => Ok(Some(value)),
        Err(X0xClientError::Status(404, _)) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

#[tauri::command]
pub async fn x0x_put_store_value(
    state: State<'_, AppState>,
    store_id: String,
    key: String,
    value_b64: String,
    content_type: Option<String>,
) -> Result<Value, String> {
    let id = path_segment(&store_id)?;
    let key = path_segment(&key)?;
    state
        .x0x_client
        .put_json(
            &format!("/stores/{id}/{key}"),
            &json!({ "value": value_b64, "content_type": content_type }),
        )
        .await
        .map_err(Into::into)
}

#[tauri::command]
pub async fn x0x_delete_store_value(
    state: State<'_, AppState>,
    store_id: String,
    key: String,
) -> Result<Value, String> {
    let id = path_segment(&store_id)?;
    let key = path_segment(&key)?;
    state
        .x0x_client
        .delete_json(&format!("/stores/{id}/{key}"))
        .await
        .map_err(Into::into)
}

// ── Managed-agent identity cards ───────────────────────────────────────────

/// Generate the local daemon's signed AgentCard. x0xd does not expose remote
/// card lookup by AgentId; remote cards enter through the explicit import path.
#[tauri::command]
pub async fn x0x_get_agent_card(
    state: State<'_, AppState>,
    display_name: Option<String>,
    include_groups: Option<bool>,
) -> Result<Value, String> {
    let mut query = Vec::new();
    if let Some(name) = display_name {
        query.push(("display_name".to_string(), name));
    }
    if include_groups.unwrap_or(false) {
        query.push(("include_groups".to_string(), "true".to_string()));
    }
    state
        .x0x_client
        .get_json("/agent/card", &query)
        .await
        .map_err(Into::into)
}

#[tauri::command]
pub async fn x0x_import_agent_card(
    state: State<'_, AppState>,
    card: String,
    trust_level: Option<String>,
) -> Result<Value, String> {
    state
        .x0x_client
        .post_json(
            "/agent/card/import",
            &json!({ "card": card, "trust_level": trust_level.unwrap_or_else(|| "known".to_string()) }),
        )
        .await
        .map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::path_segment;

    #[test]
    fn path_segment_encodes_route_delimiters() {
        assert_eq!(
            path_segment("workflow/a?b#c").as_deref(),
            Ok("workflow%2Fa%3Fb%23c")
        );
    }

    #[test]
    fn path_segment_rejects_empty_identifiers() {
        assert!(path_segment("").is_err());
    }
}
