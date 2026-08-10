//! Read-only startup eligibility and durable-child recovery for native groups.

use serde_json::Value;
use std::path::PathBuf;

use super::super::agent_identity::{
    bring_up_existing_managed_agent_child, managed_agent_child_identity,
    provision_managed_agent_child, ManagedAgentChild,
};

/// Native launch values that are safe to pass to the ACP harness.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ManagedAgentLaunchContext {
    pub(crate) child_agent_id: String,
    pub(crate) owner_agent_id: String,
    pub(crate) child_data_dir: PathBuf,
    pub(crate) group_id: String,
    pub(crate) effective_respond_to_allowlist: Vec<String>,
}

/// Controls whether launch preparation may change owner-side membership.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum GroupBindIntent {
    /// Interactive/manual paths may provision identity and attach or repair the
    /// requested group using the existing guarded bootstrap flow.
    EnsureAttached,
    /// Startup reconciliation may only reuse the durable identity whose active
    /// owner membership was already authenticated by the planning probe.
    ExistingOnly { expected_child_agent_id: String },
}

pub(super) fn owner_roster_mutation_allowed(intent: &GroupBindIntent) -> bool {
    matches!(intent, GroupBindIntent::EnsureAttached)
}

pub(super) async fn resolve_launch_child(
    pubkey: String,
    intent: GroupBindIntent,
) -> Result<ManagedAgentChild, String> {
    tokio::task::spawn_blocking(move || match intent {
        GroupBindIntent::EnsureAttached => provision_managed_agent_child(&pubkey)
            .map_err(|error| format!("managed-agent child provisioning failed: {error}")),
        GroupBindIntent::ExistingOnly {
            expected_child_agent_id,
        } => {
            let observed = managed_agent_child_identity(&pubkey).ok_or_else(|| {
                format!(
                    "managed-agent {pubkey} has no durable native identity; start it manually before enabling startup reconciliation"
                )
            })?;
            if observed.agent_id != expected_child_agent_id {
                return Err(
                    "managed-agent durable identity changed after roster eligibility was checked"
                        .to_string(),
                );
            }
            bring_up_existing_managed_agent_child(&pubkey, &expected_child_agent_id)
                .map_err(|error| format!("managed-agent existing child startup failed: {error}"))
        }
    })
    .await
    .map_err(|error| format!("managed-agent child identity task failed: {error}"))?
}

pub(super) fn require_signed_public_group(group: &Value, group_id: &str) -> Result<(), String> {
    if group
        .pointer("/policy/confidentiality")
        .and_then(Value::as_str)
        == Some("signed_public")
    {
        return Ok(());
    }
    Err(format!(
        "community {group_id} is not signed_public; managed-agent direct attachment is unsupported"
    ))
}

/// Parse the authenticated owner group and report exact ACTIVE eligibility.
/// Non-active/absent members are intentional skips; malformed group data is an
/// error so callers can return an observable per-pair failure row.
pub(crate) fn owner_group_has_existing_active_member(
    group: &Value,
    group_id: &str,
    child_agent_id: &str,
) -> Result<bool, String> {
    require_signed_public_group(group, group_id)?;
    let members = group
        .get("members")
        .and_then(Value::as_array)
        .ok_or_else(|| format!("community {group_id} response is missing a members array"))?;
    let mut child_state = None;
    for member in members {
        let agent_id = member
            .get("agent_id")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("community {group_id} contains a malformed member AgentId"))?;
        let state = member
            .get("state")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("community {group_id} contains a malformed member state"))?;
        if agent_id == child_agent_id {
            child_state = Some(state);
        }
    }
    Ok(child_state == Some("active"))
}
