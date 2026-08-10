//! Managed-agent native identity binding for community-scoped runtimes.
//!
//! A managed-agent record pubkey is a desktop record key, not an x0x AgentId.
//! Before the ACP harness starts, this module provisions the record's dedicated
//! x0xd child, establishes mutual
//! contact consent with the owner daemon, adds the child's actual AgentId to
//! the requested community, and waits until the child has installed that
//! community state.

use std::path::{Path, PathBuf};
use std::time::Duration;

use reqwest::{Method, StatusCode};
use serde_json::{json, Value};
use tauri::{AppHandle, Manager};

use super::agent_identity::{
    extract_agent_id, provision_managed_agent_child, read_child_port, read_child_token,
    ManagedAgentChild,
};
use super::ManagedAgentRecord;
use crate::app_state::AppState;
use crate::local_stack::loopback_api_base;
use crate::x0x_client::X0xClientError;

const CHILD_REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const GROUP_CONVERGENCE_TIMEOUT: Duration = Duration::from_secs(15);
const GROUP_CONVERGENCE_POLL: Duration = Duration::from_millis(250);

/// Native launch values that are safe to pass to the ACP harness.
///
/// The bearer token is deliberately absent. The harness resolves it
/// transiently from `child_data_dir`, exactly as the desktop does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ManagedAgentLaunchContext {
    pub(crate) child_agent_id: String,
    pub(crate) owner_agent_id: String,
    pub(crate) child_data_dir: PathBuf,
    pub(crate) group_id: String,
}

#[derive(Debug)]
struct ChildHttpError {
    status: Option<StatusCode>,
    message: String,
}

impl std::fmt::Display for ChildHttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

/// Provision and bind one managed agent to its requested native community.
///
/// Network work completes before runtime transition/store locks are acquired
/// by callers. This keeps those synchronous locks out of async I/O paths.
pub(crate) async fn prepare_managed_agent_launch(
    app: &AppHandle,
    record: &ManagedAgentRecord,
    group_id: &str,
) -> Result<ManagedAgentLaunchContext, String> {
    if group_id.trim().is_empty() {
        return Err("managed-agent launch requires a native community group id".to_string());
    }

    let pubkey = record.pubkey.clone();
    let child = tokio::task::spawn_blocking(move || provision_managed_agent_child(&pubkey))
        .await
        .map_err(|error| format!("managed-agent child provisioning task failed: {error}"))?
        .map_err(|error| format!("managed-agent child provisioning failed: {error}"))?;

    let state = app.state::<AppState>();
    let owner: Value = state
        .x0x_client
        .get_json("/agent", &[])
        .await
        .map_err(|error| format!("owner identity lookup failed: {error}"))?;
    let owner_agent_id = extract_agent_id(&owner)
        .ok_or_else(|| "owner /agent response did not contain a canonical AgentId".to_string())?;

    establish_mutual_consent(&state, &child, &owner_agent_id).await?;
    bind_child_to_group(&state, record, &child, &owner_agent_id, group_id).await?;

    Ok(ManagedAgentLaunchContext {
        child_agent_id: child.agent_id,
        owner_agent_id,
        child_data_dir: child.data_dir,
        group_id: group_id.to_string(),
    })
}

async fn establish_mutual_consent(
    state: &AppState,
    child: &ManagedAgentChild,
    owner_agent_id: &str,
) -> Result<(), String> {
    let owner_card: Value = state
        .x0x_client
        .get_json(
            "/agent/card",
            &[
                ("display_name".to_string(), "Community owner".to_string()),
                ("include_local_addresses".to_string(), "true".to_string()),
            ],
        )
        .await
        .map_err(|error| format!("owner AgentCard generation failed: {error}"))?;
    let owner_link = card_link(&owner_card, "owner")?;

    let child_card = child_request_json(
        &state.http_client,
        &child.data_dir,
        Method::GET,
        "/agent/card?include_local_addresses=true",
        None,
    )
    .await
    .map_err(|error| format!("managed-agent AgentCard generation failed: {error}"))?;
    let child_link = card_link(&child_card, "managed agent")?;

    // Consent must exist on the receiver before the owner emits the public
    // group bootstrap. A sticky Blocked contact is an explicit user decision
    // and therefore fails closed instead of being silently overwritten.
    let child_import = child_request_json(
        &state.http_client,
        &child.data_dir,
        Method::POST,
        "/agent/card/import",
        Some(&json!({ "card": owner_link, "trust_level": "known" })),
    )
    .await
    .map_err(|error| format!("managed-agent owner consent failed: {error}"))?;
    require_known_contact(&child_import, owner_agent_id, "managed agent")?;

    let owner_import: Value = state
        .x0x_client
        .post_json(
            "/agent/card/import",
            &json!({ "card": child_link, "trust_level": "known" }),
        )
        .await
        .map_err(|error| format!("owner managed-agent consent failed: {error}"))?;
    require_known_contact(&owner_import, &child.agent_id, "owner")?;

    // Card import seeds exact local addresses in x0xd's discovery cache.
    // Explicit connect is a best-effort warm-up; membership bootstrap remains
    // authoritative and convergence below is the required success signal.
    let _ = child_request_json(
        &state.http_client,
        &child.data_dir,
        Method::POST,
        "/agents/connect",
        Some(&json!({ "agent_id": owner_agent_id })),
    )
    .await;
    let _: Result<Value, _> = state
        .x0x_client
        .post_json("/agents/connect", &json!({ "agent_id": child.agent_id }))
        .await;

    Ok(())
}

async fn bind_child_to_group(
    state: &AppState,
    record: &ManagedAgentRecord,
    child: &ManagedAgentChild,
    owner_agent_id: &str,
    group_id: &str,
) -> Result<(), String> {
    let binding = membership_binding(&record.pubkey, &child.agent_id, owner_agent_id);
    let encoded_group = crate::path_segment(group_id)?;
    let group_path = format!("/groups/{encoded_group}");
    let mut owner_group: Value = state
        .x0x_client
        .get_json(&group_path, &[])
        .await
        .map_err(|error| format!("community lookup failed: {error}"))?;
    if owner_group
        .pointer("/policy/confidentiality")
        .and_then(Value::as_str)
        != Some("signed_public")
    {
        return Err(format!(
            "community {group_id} is not signed_public; managed-agent direct attachment is unsupported"
        ));
    }

    let child_installed = child_group_matches(state, child, &group_path, None).await?;
    if !child_installed {
        if group_has_member(&owner_group, binding.attach_agent_id) {
            // A previous add may have committed before contact consent existed.
            // x0xd returns 409 without resending bootstrap, so remove/re-add is
            // the deterministic recovery that emits a fresh bootstrap.
            remove_owner_member(state, &encoded_group, binding.attach_agent_id).await?;
        }

        let members_path = format!("{group_path}/members");
        let add_body = group_member_body(binding.attach_agent_id, &record.name);
        let add_result: Result<Value, X0xClientError> =
            state.x0x_client.post_json(&members_path, &add_body).await;
        match add_result {
            Ok(_) | Err(X0xClientError::Status(409, _)) => {}
            Err(error) => return Err(format!("managed-agent community attach failed: {error}")),
        }
    }

    wait_for_child_group(state, child, &group_path, group_id, None).await?;

    // Compatibility migration: older UI code inserted record.pubkey under the
    // assumption that it was the AgentId. Remove that stale roster entry only
    // after the actual child is confirmed installed, so migration cannot leave
    // the community without a working managed-agent member.
    if let Some(legacy_agent_id) = binding.remove_legacy_agent_id {
        owner_group = state
            .x0x_client
            .get_json(&group_path, &[])
            .await
            .map_err(|error| format!("community migration lookup failed: {error}"))?;
        if group_has_member(&owner_group, legacy_agent_id) {
            remove_owner_member(state, &encoded_group, legacy_agent_id).await?;
            wait_for_child_group(state, child, &group_path, group_id, Some(legacy_agent_id))
                .await?;
        }
    }

    Ok(())
}

async fn child_group_matches(
    state: &AppState,
    child: &ManagedAgentChild,
    group_path: &str,
    forbidden_agent_id: Option<&str>,
) -> Result<bool, String> {
    match child_request_json(
        &state.http_client,
        &child.data_dir,
        Method::GET,
        group_path,
        None,
    )
    .await
    {
        Ok(group) => {
            let contains_child = group_has_member(&group, &child.agent_id);
            let contains_forbidden =
                forbidden_agent_id.is_some_and(|id| group_has_member(&group, id));
            Ok(contains_child && !contains_forbidden)
        }
        Err(ChildHttpError {
            status: Some(StatusCode::NOT_FOUND),
            ..
        }) => Ok(false),
        Err(error) => Err(format!("managed-agent community lookup failed: {error}")),
    }
}

async fn wait_for_child_group(
    state: &AppState,
    child: &ManagedAgentChild,
    group_path: &str,
    group_id: &str,
    forbidden_agent_id: Option<&str>,
) -> Result<(), String> {
    let deadline = tokio::time::Instant::now() + GROUP_CONVERGENCE_TIMEOUT;
    loop {
        if child_group_matches(state, child, group_path, forbidden_agent_id).await? {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(format!(
                "managed-agent community {group_id} did not converge on child {} within {} seconds",
                child.agent_id,
                GROUP_CONVERGENCE_TIMEOUT.as_secs()
            ));
        }
        tokio::time::sleep(GROUP_CONVERGENCE_POLL).await;
    }
}

async fn remove_owner_member(
    state: &AppState,
    encoded_group: &str,
    agent_id: &str,
) -> Result<(), String> {
    let encoded_agent = crate::path_segment(agent_id)?;
    let _: Value = state
        .x0x_client
        .delete_json(&format!("/groups/{encoded_group}/members/{encoded_agent}"))
        .await
        .map_err(|error| format!("community member migration failed: {error}"))?;
    Ok(())
}

fn card_link<'a>(value: &'a Value, label: &str) -> Result<&'a str, String> {
    value
        .get("link")
        .and_then(Value::as_str)
        .filter(|link| !link.is_empty())
        .ok_or_else(|| format!("{label} AgentCard response did not contain a link"))
}

fn require_known_contact(
    value: &Value,
    expected_agent_id: &str,
    label: &str,
) -> Result<(), String> {
    let returned_id = value.get("agent_id").and_then(Value::as_str);
    let trust = value.get("trust_level").and_then(Value::as_str);
    if returned_id != Some(expected_agent_id) {
        return Err(format!(
            "{label} AgentCard import returned a different AgentId"
        ));
    }
    if !matches!(trust, Some("Known" | "Trusted")) {
        return Err(format!(
            "{label} contact consent is {trust:?}; Known or Trusted is required"
        ));
    }
    Ok(())
}

fn group_has_member(group: &Value, agent_id: &str) -> bool {
    group
        .get("members")
        .and_then(Value::as_array)
        .is_some_and(|members| {
            members
                .iter()
                .any(|member| member.get("agent_id").and_then(Value::as_str) == Some(agent_id))
        })
}

#[derive(Debug, PartialEq, Eq)]
struct MembershipBinding<'a> {
    attach_agent_id: &'a str,
    remove_legacy_agent_id: Option<&'a str>,
}

fn membership_binding<'a>(
    record_pubkey: &'a str,
    child_agent_id: &'a str,
    owner_agent_id: &str,
) -> MembershipBinding<'a> {
    let remove_legacy_agent_id = (!record_pubkey.eq_ignore_ascii_case(child_agent_id)
        && !record_pubkey.eq_ignore_ascii_case(owner_agent_id))
    .then_some(record_pubkey);
    MembershipBinding {
        attach_agent_id: child_agent_id,
        remove_legacy_agent_id,
    }
}

fn group_member_body(agent_id: &str, display_name: &str) -> Value {
    json!({
        "agent_id": agent_id,
        "display_name": display_name,
    })
}

async fn child_request_json(
    http: &reqwest::Client,
    data_dir: &Path,
    method: Method,
    path: &str,
    body: Option<&Value>,
) -> Result<Value, ChildHttpError> {
    let port = read_child_port(data_dir).ok_or_else(|| ChildHttpError {
        status: None,
        message: "managed-agent api.port is missing or non-loopback".to_string(),
    })?;
    let token = read_child_token(data_dir).ok_or_else(|| ChildHttpError {
        status: None,
        message: "managed-agent api-token is missing".to_string(),
    })?;
    let url = format!("{}{}", loopback_api_base(port), path);
    let mut request = http
        .request(method, &url)
        .bearer_auth(&token)
        .timeout(CHILD_REQUEST_TIMEOUT);
    if let Some(body) = body {
        request = request.json(body);
    }
    let response = request.send().await.map_err(|error| ChildHttpError {
        status: None,
        message: format!("managed-agent x0xd request failed: {error}"),
    })?;
    drop(token);
    let status = response.status();
    let text = response.text().await.map_err(|error| ChildHttpError {
        status: Some(status),
        message: format!("managed-agent x0xd response read failed: {error}"),
    })?;
    if !status.is_success() {
        return Err(ChildHttpError {
            status: Some(status),
            message: format!(
                "managed-agent x0xd returned HTTP {}: {}",
                status.as_u16(),
                text.chars().take(300).collect::<String>()
            ),
        });
    }
    serde_json::from_str(&text).map_err(|error| ChildHttpError {
        status: Some(status),
        message: format!("managed-agent x0xd returned invalid JSON: {error}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn membership_matching_uses_actual_child_agent_id() {
        let record_pubkey = "aa".repeat(32);
        let child_agent_id = "bb".repeat(32);
        let owner_agent_id = "cc".repeat(32);
        let binding = membership_binding(&record_pubkey, &child_agent_id, &owner_agent_id);
        let add_body = group_member_body(binding.attach_agent_id, "Guide");

        assert_eq!(binding.attach_agent_id, child_agent_id);
        assert_eq!(binding.remove_legacy_agent_id, Some(record_pubkey.as_str()));
        assert_eq!(add_body["agent_id"], child_agent_id);
        assert_ne!(add_body["agent_id"], record_pubkey);
    }

    #[test]
    fn blocked_import_does_not_satisfy_bootstrap_consent() {
        let id = "bb".repeat(32);
        let imported = json!({ "agent_id": id, "trust_level": "Blocked" });
        assert!(require_known_contact(&imported, &id, "managed agent").is_err());
    }
}
