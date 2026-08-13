//! Native Welcome/onboarding sends authored by managed-agent child daemons.
//!
//! Managed record pubkeys select local runtime records only. Group authorship
//! is always the provisioned child x0x AgentId, and the child bearer token is
//! resolved transiently from its isolated data directory per request.

use base64::Engine as _;
use reqwest::Method;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::{future::Future, time::Duration};
use tauri::State;

use crate::app_state::AppState;
use crate::local_stack::{loopback_api_base, read_api_port, read_api_token};
use crate::managed_agents::agent_identity::{managed_agent_child_identity, ManagedAgentChild};
use crate::x0x_client::{HistoryListRequest, HistoryRow, X0xClient};

const HISTORY_PAGE_SIZE: usize = 200;
const MAX_HISTORY_PAGES: usize = 500;
const CHILD_REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const SECURE_SEND_BLOCKER: &str = "Secure managed-agent group sends are unavailable.";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManagedAgentMessageEnvelope {
    #[serde(default)]
    markers: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct OutboundManagedAgentMessageEnvelope<'a> {
    text: &'a str,
    created_at: u64,
    client_id: &'a str,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    mentions: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    markers: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct ManagedAgentChannelMessageResult {
    event_id: String,
    parent_event_id: Option<String>,
    root_event_id: Option<String>,
    depth: u32,
    created_at: u64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SendManagedAgentChannelMessageInput {
    agent_pubkey: String,
    channel_id: String,
    content: String,
    marker: Option<String>,
    marker_scope: Option<String>,
    mention_pubkeys: Option<Vec<String>>,
    parent_event_id: Option<String>,
    additional_markers: Option<Vec<String>>,
}

#[derive(Deserialize)]
struct ChildHistoryResponse {
    #[serde(default)]
    records: Vec<HistoryRow>,
    #[serde(default)]
    next_before_id: Option<i64>,
}

fn canonical_agent_id(value: &str) -> Option<String> {
    (value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f')))
    .then(|| value.to_string())
}

fn clean_markers(marker: Option<String>, additional: Option<Vec<String>>) -> Vec<String> {
    let mut markers = Vec::new();
    for candidate in marker.into_iter().chain(additional.unwrap_or_default()) {
        let candidate = candidate.trim();
        if !candidate.is_empty() && !markers.iter().any(|existing| existing == candidate) {
            markers.push(candidate.to_string());
        }
    }
    markers
}

fn payload_has_marker(payload: &str, marker: &str) -> bool {
    let Ok(decoded) = base64::engine::general_purpose::STANDARD.decode(payload) else {
        return false;
    };
    let Ok(envelope) = serde_json::from_slice::<ManagedAgentMessageEnvelope>(&decoded) else {
        return false;
    };
    envelope.markers.iter().any(|value| value == marker)
}

fn row_matches_marker(row: &HistoryRow, marker: &str, author_agent_id: Option<&str>) -> bool {
    author_agent_id.is_none_or(|expected| {
        row.author_agent
            .as_deref()
            .is_some_and(|actual| actual.eq_ignore_ascii_case(expected))
    }) && payload_has_marker(&row.payload, marker)
}

async fn find_marker(
    client: &X0xClient,
    channel_id: &str,
    marker: &str,
    author_agent_id: Option<&str>,
) -> Result<Option<HistoryRow>, String> {
    let transport = client.resolve_group_transport(channel_id).await?;
    let scope = format!("group:{}", transport.stable_group_id);
    let mut before_id: Option<i64> = None;

    for _ in 0..MAX_HISTORY_PAGES {
        let page = client
            .history_list(&HistoryListRequest {
                scope: scope.clone(),
                since_ms: None,
                until_ms: None,
                limit: Some(HISTORY_PAGE_SIZE),
                before_id,
            })
            .await?;
        if let Some(row) = page
            .rows
            .into_iter()
            .find(|row| row_matches_marker(row, marker, author_agent_id))
        {
            return Ok(Some(row));
        }
        let Some(next_before_id) = page.next_before_id else {
            return Ok(None);
        };
        before_id = Some(next_before_id);
    }

    Err("managed-agent marker lookup exceeded the history page safety limit".to_string())
}

async fn child_request_json<T: DeserializeOwned>(
    http: &reqwest::Client,
    child: &ManagedAgentChild,
    method: Method,
    path: &str,
    query: &[(String, String)],
    body: Option<&serde_json::Value>,
) -> Result<T, String> {
    let port = read_api_port(&child.data_dir)
        .ok_or_else(|| "managed-agent api.port is missing or non-loopback".to_string())?;
    let token = read_api_token(&child.data_dir)
        .ok_or_else(|| "managed-agent api-token is missing".to_string())?;
    let url = format!("{}{path}", loopback_api_base(port));
    let mut request = http
        .request(method, url)
        .bearer_auth(&token)
        .query(query)
        .timeout(CHILD_REQUEST_TIMEOUT);
    if let Some(body) = body {
        request = request.json(body);
    }
    let response = request
        .send()
        .await
        .map_err(|error| format!("managed-agent x0xd request failed: {error}"))?;
    drop(token);
    let status = response.status();
    let text = response
        .text()
        .await
        .map_err(|error| format!("managed-agent x0xd response read failed: {error}"))?;
    if !status.is_success() {
        return Err(format!(
            "managed-agent x0xd returned HTTP {}: {}",
            status.as_u16(),
            text.chars().take(300).collect::<String>()
        ));
    }
    serde_json::from_str(&text)
        .map_err(|error| format!("managed-agent x0xd returned invalid JSON: {error}"))
}

async fn child_group_scope(
    http: &reqwest::Client,
    child: &ManagedAgentChild,
    channel_id: &str,
) -> Result<String, String> {
    let detail: serde_json::Value = child_request_json(
        http,
        child,
        Method::GET,
        &format!("/groups/{channel_id}"),
        &[],
        None,
    )
    .await?;
    let confidentiality = detail
        .get("policy")
        .and_then(|policy| policy.get("confidentiality"))
        .and_then(|value| value.as_str());
    if confidentiality != Some("signed_public") {
        return Err(SECURE_SEND_BLOCKER.to_string());
    }
    let state: serde_json::Value = child_request_json(
        http,
        child,
        Method::GET,
        &format!("/groups/{channel_id}/state"),
        &[],
        None,
    )
    .await?;
    let stable_group_id = state
        .get("group_id")
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "managed-agent group state is missing its stable group id".to_string())?;
    Ok(format!("group:{stable_group_id}"))
}

async fn find_child_marker(
    http: &reqwest::Client,
    child: &ManagedAgentChild,
    channel_id: &str,
    marker: &str,
    author_agent_id: Option<&str>,
) -> Result<Option<HistoryRow>, String> {
    let scope = child_group_scope(http, child, channel_id).await?;
    let mut before_id: Option<i64> = None;
    for _ in 0..MAX_HISTORY_PAGES {
        let mut query = vec![
            ("scope".to_string(), scope.clone()),
            ("limit".to_string(), HISTORY_PAGE_SIZE.to_string()),
        ];
        if let Some(cursor) = before_id {
            query.push(("before_id".to_string(), cursor.to_string()));
        }
        let page: ChildHistoryResponse =
            child_request_json(http, child, Method::GET, "/history", &query, None).await?;
        if let Some(row) = page
            .records
            .into_iter()
            .find(|row| row_matches_marker(row, marker, author_agent_id))
        {
            return Ok(Some(row));
        }
        let Some(next_before_id) = page.next_before_id else {
            return Ok(None);
        };
        before_id = Some(next_before_id);
    }
    Err("managed-agent child marker lookup exceeded the history page safety limit".to_string())
}

async fn send_child_group_message(
    http: &reqwest::Client,
    child: &ManagedAgentChild,
    channel_id: &str,
    body: &str,
    parent_event_id: Option<&str>,
) -> Result<Option<String>, String> {
    let _scope = child_group_scope(http, child, channel_id).await?;
    let response: serde_json::Value = child_request_json(
        http,
        child,
        Method::POST,
        &format!("/groups/{channel_id}/send"),
        &[],
        Some(&serde_json::json!({
            "body": body,
            "kind": "chat",
            "thread_root": parent_event_id,
            "thread_parent": parent_event_id,
        })),
    )
    .await?;
    Ok(response
        .get("msg_id")
        .and_then(|value| value.as_str())
        .map(str::to_string))
}

async fn find_child_then_owner_marker<T, ChildLookup, ChildFuture, OwnerLookup, OwnerFuture>(
    child_lookup: ChildLookup,
    owner_lookup: OwnerLookup,
) -> Result<Option<T>, String>
where
    ChildLookup: FnOnce() -> ChildFuture,
    ChildFuture: Future<Output = Result<Option<T>, String>>,
    OwnerLookup: FnOnce() -> OwnerFuture,
    OwnerFuture: Future<Output = Result<Option<T>, String>>,
{
    match child_lookup().await? {
        Some(row) => Ok(Some(row)),
        None => owner_lookup().await,
    }
}

fn marker_author_agent_id(
    marker_scope: Option<&str>,
    agent_pubkey: Option<&str>,
) -> Result<Option<String>, String> {
    if marker_scope == Some("channel") {
        return Ok(None);
    }
    let pubkey = agent_pubkey
        .ok_or_else(|| "agentPubkey is required for an agent-scoped marker lookup".to_string())?;
    managed_agent_child_identity(pubkey)
        .map(|child| Some(child.agent_id))
        .ok_or_else(|| "managed agent has no provisioned native identity".to_string())
}

#[tauri::command]
pub async fn has_managed_agent_channel_message_marker(
    state: State<'_, AppState>,
    channel_id: String,
    marker: String,
    agent_pubkey: Option<String>,
    marker_scope: Option<String>,
) -> Result<bool, String> {
    let marker = marker.trim();
    if marker.is_empty() {
        return Ok(false);
    }
    let author_agent_id = marker_author_agent_id(marker_scope.as_deref(), agent_pubkey.as_deref())?;
    find_marker(
        &state.x0x_client,
        &channel_id,
        marker,
        author_agent_id.as_deref(),
    )
    .await
    .map(|row| row.is_some())
}

#[tauri::command]
pub async fn send_managed_agent_channel_message(
    state: State<'_, AppState>,
    input: SendManagedAgentChannelMessageInput,
) -> Result<ManagedAgentChannelMessageResult, String> {
    let child = managed_agent_child_identity(&input.agent_pubkey)
        .ok_or_else(|| "managed agent has no provisioned native identity".to_string())?;
    let mentions = input
        .mention_pubkeys
        .unwrap_or_default()
        .iter()
        .map(|value| {
            canonical_agent_id(value).ok_or_else(|| {
                "managed-agent message contains an invalid native mention".to_string()
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let markers = clean_markers(input.marker, input.additional_markers);
    let http = reqwest::Client::builder()
        .no_proxy()
        .build()
        .map_err(|error| format!("failed to build managed-agent loopback client: {error}"))?;

    if let Some(primary_marker) = markers.first() {
        let marker_author = if input.marker_scope.as_deref() == Some("channel") {
            None
        } else {
            Some(child.agent_id.as_str())
        };
        if let Some(existing) = find_child_then_owner_marker(
            || {
                find_child_marker(
                    &http,
                    &child,
                    &input.channel_id,
                    primary_marker,
                    marker_author,
                )
            },
            || {
                find_marker(
                    &state.x0x_client,
                    &input.channel_id,
                    primary_marker,
                    marker_author,
                )
            },
        )
        .await?
        {
            let depth = u32::from(existing.thread_parent.is_some());
            return Ok(ManagedAgentChannelMessageResult {
                event_id: existing.msg_id,
                parent_event_id: existing.thread_parent,
                root_event_id: existing.thread_root,
                depth,
                created_at: existing.seen_at_ms.max(0) as u64 / 1_000,
            });
        }
    }

    let created_at_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| format!("system clock is before the Unix epoch: {error}"))?
        .as_millis() as u64;
    let client_id = uuid::Uuid::new_v4().to_string();
    let envelope = serde_json::to_string(&OutboundManagedAgentMessageEnvelope {
        text: input.content.trim(),
        created_at: created_at_ms,
        client_id: &client_id,
        mentions,
        markers,
    })
    .map_err(|error| format!("failed to encode managed-agent message: {error}"))?;
    let msg_id = send_child_group_message(
        &http,
        &child,
        &input.channel_id,
        &envelope,
        input.parent_event_id.as_deref(),
    )
    .await?;

    Ok(ManagedAgentChannelMessageResult {
        event_id: msg_id.unwrap_or(client_id),
        parent_event_id: input.parent_event_id.clone(),
        root_event_id: input.parent_event_id.clone(),
        depth: u32::from(input.parent_event_id.is_some()),
        created_at: created_at_ms / 1_000,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marker_parser_reads_native_envelope_without_matching_visible_text() {
        let with_marker = base64::engine::general_purpose::STANDARD.encode(
            br#"{"text":"not the marker","createdAt":1,"clientId":"id","markers":["kickoff.v1"]}"#,
        );
        let text_only = base64::engine::general_purpose::STANDARD
            .encode(br#"{"text":"kickoff.v1","createdAt":1,"clientId":"id"}"#);
        assert!(payload_has_marker(&with_marker, "kickoff.v1"));
        assert!(!payload_has_marker(&text_only, "kickoff.v1"));
    }

    #[test]
    fn managed_message_mentions_require_canonical_native_agent_ids() {
        assert_eq!(canonical_agent_id(&"a".repeat(64)), Some("a".repeat(64)));
        assert_eq!(canonical_agent_id(&"A".repeat(64)), None);
        assert_eq!(canonical_agent_id("legacy-record-key"), None);
    }

    #[tokio::test]
    async fn immediate_repeat_is_caught_in_child_history_before_owner_convergence() {
        let owner_queried = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let owner_observer = owner_queried.clone();

        let found = find_child_then_owner_marker(
            || async { Ok(Some("child-local-message")) },
            || async move {
                owner_observer.store(true, std::sync::atomic::Ordering::SeqCst);
                Ok(None)
            },
        )
        .await;

        assert_eq!(found, Ok(Some("child-local-message")));
        assert!(!owner_queried.load(std::sync::atomic::Ordering::SeqCst));
    }
}
