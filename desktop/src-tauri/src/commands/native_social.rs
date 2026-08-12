//! Native x0xd contacts and presence commands.
//!
//! These commands proxy only the authenticated loopback daemon surface. They
//! never sign or publish a relay/Nostr event, and malformed identifiers fail
//! before a request is sent.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::State;

use crate::app_state::AppState;

const AGENT_ID_HEX_LEN: usize = 64;
const CONNECT_REQUEST_TIMEOUT: Duration = Duration::from_secs(65);

pub(crate) fn validate_agent_id(value: &str) -> Result<String, String> {
    if value.len() != AGENT_ID_HEX_LEN
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err("invalid x0x AgentId (expected 64 lowercase hex characters)".to_string());
    }
    Ok(value.to_string())
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeConnectResult {
    pub outcome: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
}

#[derive(Debug, Serialize)]
struct ConnectAgentBody<'a> {
    agent_id: &'a str,
}

#[derive(Debug, Deserialize)]
struct ConnectAgentResponse {
    ok: bool,
    outcome: String,
    #[serde(default)]
    addr: Option<String>,
}

/// POST /agents/connect — bounded exact-AgentId connection through the
/// authenticated loopback daemon. The request deadline is 65 seconds: bounded,
/// but long enough for x0xd's documented 60-second connect outcome. The bearer
/// token stays entirely inside the request header.
///
/// The command does not synthesize contact/trust state. First contact remains
/// the signed AgentCard import flow; this command connects an exact canonical
/// AgentId that the daemon already knows how to discover.
#[tauri::command]
pub async fn x0x_connect_agent(
    agent_id: String,
    state: State<'_, AppState>,
) -> Result<NativeConnectResult, String> {
    let agent_id = validate_agent_id(&agent_id)?;
    let response: ConnectAgentResponse = state
        .x0x_client
        .post_json_with_timeout(
            "/agents/connect",
            &ConnectAgentBody {
                agent_id: &agent_id,
            },
            CONNECT_REQUEST_TIMEOUT,
        )
        .await?;
    if !response.ok {
        return Err("x0xd rejected the connection request".to_string());
    }
    if !matches!(
        response.outcome.as_str(),
        "Direct" | "Coordinated" | "AlreadyConnected" | "Unreachable" | "NotFound"
    ) {
        return Err("x0xd returned an unknown connection outcome".to_string());
    }
    Ok(NativeConnectResult {
        outcome: response.outcome,
        address: response.addr,
    })
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NativeContact {
    pub agent_id: String,
    pub trust_level: String,
    pub label: Option<String>,
    pub added_at: u64,
    pub last_seen: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct RawContacts {
    #[serde(default)]
    contacts: Vec<RawContact>,
}

#[derive(Debug, Deserialize)]
struct RawContact {
    agent_id: String,
    trust_level: String,
    #[serde(default)]
    label: Option<String>,
    #[serde(default)]
    added_at: u64,
    #[serde(default)]
    last_seen: Option<u64>,
}

impl TryFrom<RawContact> for NativeContact {
    type Error = String;

    fn try_from(value: RawContact) -> Result<Self, Self::Error> {
        Ok(Self {
            agent_id: validate_agent_id(&value.agent_id)?,
            trust_level: value.trust_level,
            label: value.label,
            added_at: value.added_at,
            last_seen: value.last_seen,
        })
    }
}

#[derive(Debug, Serialize)]
struct AddContactBody<'a> {
    agent_id: &'a str,
    trust_level: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    label: Option<&'a str>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AddNativeContactRequest {
    agent_id: String,
    #[serde(default = "default_trust")]
    trust_level: String,
    label: Option<String>,
}

fn default_trust() -> String {
    "known".to_string()
}

#[derive(Debug, Serialize)]
struct DisplayNameBody<'a> {
    name: &'a str,
}

#[derive(Debug, Deserialize)]
struct PresenceResponse {
    #[serde(default)]
    agents: Vec<String>,
}

/// GET /contacts — daemon-owned contact frontier.
#[tauri::command]
pub async fn x0x_list_contacts(state: State<'_, AppState>) -> Result<Vec<NativeContact>, String> {
    let raw: RawContacts = state.x0x_client.get_json("/contacts", &[]).await?;
    raw.contacts.into_iter().map(TryInto::try_into).collect()
}

/// POST /contacts — add an AgentId without accepting npub/bech32 aliases.
#[tauri::command]
pub async fn x0x_add_contact(
    input: AddNativeContactRequest,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let agent_id = validate_agent_id(&input.agent_id)?;
    let body = AddContactBody {
        agent_id: &agent_id,
        trust_level: &input.trust_level,
        label: input.label.as_deref(),
    };
    let _: serde_json::Value = state.x0x_client.post_json("/contacts", &body).await?;
    Ok(())
}

/// DELETE /contacts/:agent_id.
#[tauri::command]
pub async fn x0x_remove_contact(
    agent_id: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let agent_id = validate_agent_id(&agent_id)?;
    state
        .x0x_client
        .delete(&format!("/contacts/{agent_id}"))
        .await?;
    Ok(())
}

/// PUT /groups/:id/display-name — persist this agent's roster profile name.
#[tauri::command]
pub async fn x0x_set_group_display_name(
    group_id: String,
    name: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let group_id = group_id.trim();
    let name = name.trim();
    if group_id.is_empty() || name.is_empty() {
        return Err("group id and display name must be non-empty".to_string());
    }
    let _: serde_json::Value = state
        .x0x_client
        .put_json(
            &format!("/groups/{group_id}/display-name"),
            &DisplayNameBody { name },
        )
        .await?;
    Ok(())
}

/// GET /presence — map requested AgentIds to the daemon's online/offline view.
#[tauri::command]
pub async fn x0x_get_presence(
    agent_ids: Vec<String>,
    state: State<'_, AppState>,
) -> Result<HashMap<String, String>, String> {
    let requested: Vec<String> = agent_ids
        .iter()
        .map(|agent_id| validate_agent_id(agent_id))
        .collect::<Result<_, _>>()?;
    let response: PresenceResponse = state.x0x_client.get_json("/presence", &[]).await?;
    let online: HashSet<String> = response
        .agents
        .into_iter()
        .filter_map(|agent_id| validate_agent_id(&agent_id).ok())
        .collect();
    Ok(requested
        .into_iter()
        .map(|agent_id| {
            let status = if online.contains(&agent_id) {
                "online"
            } else {
                "offline"
            };
            (agent_id, status.to_string())
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_id_validation_requires_canonical_lowercase_hex() {
        assert!(validate_agent_id(&format!("npub1{}", "0".repeat(59))).is_err());
        assert!(validate_agent_id(&"AB".repeat(32)).is_err());
        assert!(validate_agent_id(&format!(" {}", "ab".repeat(32))).is_err());
        assert_eq!(
            validate_agent_id(&"ab".repeat(32)).unwrap(),
            "ab".repeat(32)
        );
    }

    #[test]
    fn connect_result_serializes_without_daemon_credentials() {
        let value = serde_json::to_value(NativeConnectResult {
            outcome: "Direct".to_string(),
            address: Some("127.0.0.1:5483".to_string()),
        })
        .unwrap();
        assert_eq!(value["outcome"], "Direct");
        assert_eq!(value["address"], "127.0.0.1:5483");
        assert!(value.get("token").is_none());
        assert!(value.get("apiToken").is_none());
    }

    #[test]
    fn contact_serializes_agent_id_without_pubkey_alias() {
        let contact = NativeContact {
            agent_id: "ab".repeat(32),
            trust_level: "known".to_string(),
            label: Some("Ada".to_string()),
            added_at: 7,
            last_seen: None,
        };
        let value = serde_json::to_value(contact).unwrap();
        assert_eq!(value["agentId"], "ab".repeat(32));
        assert!(value.get("pubkey").is_none());
        assert!(value.get("npub").is_none());
    }
}
