//! Managed-agent native identity binding for community-scoped runtimes.
//!
//! A managed-agent record pubkey is a desktop record key, not an x0x AgentId.
//! Before the ACP harness starts, this module provisions the record's dedicated
//! x0xd child, establishes mutual
//! contact consent with the owner daemon, adds the child's actual AgentId to
//! the requested community, and waits until the child has installed that
//! community state.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use reqwest::{Method, StatusCode};
use serde_json::{json, Value};
use tauri::AppHandle;

use super::agent_identity::{
    extract_agent_id, provision_managed_agent_child, read_child_port, read_child_token,
    ManagedAgentChild,
};
use super::{load_managed_agents, ManagedAgentRecord, RespondTo};
use crate::local_stack::loopback_api_base;
use crate::x0x_client::{X0xClient, X0xClientError};

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
    /// Spawn-effective inbound AgentIds. Managed-record pubkeys are translated
    /// to their dedicated child identities without mutating persisted records.
    pub(crate) effective_respond_to_allowlist: Vec<String>,
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

    let effective_respond_to_allowlist =
        resolve_effective_respond_to_allowlist(app, record, &child).await?;

    let loopback_http = loopback_http_client()?;
    let owner_client = X0xClient::new(loopback_http.clone());
    let owner: Value = owner_client
        .get_json("/agent", &[])
        .await
        .map_err(|error| format!("owner identity lookup failed: {error}"))?;
    let owner_agent_id = extract_agent_id(&owner)
        .ok_or_else(|| "owner /agent response did not contain a canonical AgentId".to_string())?;

    establish_mutual_consent(&owner_client, &loopback_http, &child, &owner_agent_id).await?;
    bind_child_to_group(
        &owner_client,
        &loopback_http,
        record,
        &child,
        &owner_agent_id,
        group_id,
    )
    .await?;

    Ok(ManagedAgentLaunchContext {
        child_agent_id: child.agent_id,
        owner_agent_id,
        child_data_dir: child.data_dir,
        group_id: group_id.to_string(),
        effective_respond_to_allowlist,
    })
}

/// Bearer-authenticated x0xd calls must never inherit system proxy settings.
/// The app-wide HTTP client intentionally supports external traffic, so native
/// loopback orchestration owns this dedicated no-proxy client instead.
fn loopback_http_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .no_proxy()
        .pool_idle_timeout(Duration::from_secs(10))
        .pool_max_idle_per_host(1)
        .build()
        .map_err(|error| format!("failed to build managed-agent loopback client: {error}"))
}

async fn resolve_effective_respond_to_allowlist(
    app: &AppHandle,
    record: &ManagedAgentRecord,
    current_child: &ManagedAgentChild,
) -> Result<Vec<String>, String> {
    let normalized = super::validate_respond_to_allowlist(&record.respond_to_allowlist)?;
    if record.respond_to != RespondTo::Allowlist {
        return Ok(normalized);
    }

    let records = load_managed_agents(app)?;
    let mut translated = HashMap::new();
    for entry in &normalized {
        let Some(managed_record) = records
            .iter()
            .find(|candidate| candidate.pubkey.eq_ignore_ascii_case(entry))
        else {
            continue;
        };

        let child_agent_id = if managed_record.pubkey.eq_ignore_ascii_case(&record.pubkey) {
            current_child.agent_id.clone()
        } else {
            let teammate_pubkey = managed_record.pubkey.clone();
            tokio::task::spawn_blocking(move || provision_managed_agent_child(&teammate_pubkey))
                .await
                .map_err(|error| format!("managed-agent allowlist identity task failed: {error}"))?
                .map_err(|error| {
                    format!("managed-agent allowlist identity provisioning failed: {error}")
                })?
                .agent_id
        };
        translated.insert(entry.clone(), child_agent_id);
    }

    translate_managed_allowlist(&normalized, &translated)
}

fn translate_managed_allowlist(
    normalized: &[String],
    translated: &HashMap<String, String>,
) -> Result<Vec<String>, String> {
    let mut seen = HashSet::new();
    let mut effective = Vec::with_capacity(normalized.len());
    for entry in normalized {
        let value = translated.get(entry).unwrap_or(entry);
        let canonical = value.to_ascii_lowercase();
        if seen.insert(canonical.clone()) {
            effective.push(canonical);
        }
    }
    super::validate_respond_to_allowlist(&effective)
}

async fn establish_mutual_consent(
    owner_client: &X0xClient,
    loopback_http: &reqwest::Client,
    child: &ManagedAgentChild,
    owner_agent_id: &str,
) -> Result<(), String> {
    let owner_card: Value = owner_client
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
        loopback_http,
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
        loopback_http,
        &child.data_dir,
        Method::POST,
        "/agent/card/import",
        Some(&json!({ "card": owner_link, "trust_level": "known" })),
    )
    .await
    .map_err(|error| format!("managed-agent owner consent failed: {error}"))?;
    require_known_contact(&child_import, owner_agent_id, "managed agent")?;

    let owner_import: Value = owner_client
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
        loopback_http,
        &child.data_dir,
        Method::POST,
        "/agents/connect",
        Some(&json!({ "agent_id": owner_agent_id })),
    )
    .await;
    let _: Result<Value, _> = owner_client
        .post_json("/agents/connect", &json!({ "agent_id": child.agent_id }))
        .await;

    Ok(())
}

async fn bind_child_to_group(
    owner_client: &X0xClient,
    loopback_http: &reqwest::Client,
    record: &ManagedAgentRecord,
    child: &ManagedAgentChild,
    owner_agent_id: &str,
    group_id: &str,
) -> Result<(), String> {
    let binding = membership_binding(&record.pubkey, &child.agent_id, owner_agent_id);
    let encoded_group = crate::path_segment(group_id)?;
    let group_path = format!("/groups/{encoded_group}");
    let mut owner_group: Value = owner_client
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

    let owner_child_state = group_member_state(&owner_group, binding.attach_agent_id);
    reject_banned_member(
        owner_child_state,
        binding.attach_agent_id,
        "owner community roster",
    )?;

    let mut expected_roster_revision = roster_revision(&owner_group)?;
    let child_installed = child_group_matches(
        loopback_http,
        child,
        &group_path,
        expected_roster_revision,
        None,
    )
    .await?;
    if !child_installed {
        match owner_child_state {
            GroupMemberState::Active => {
                // A previous add may have committed before contact consent
                // existed. x0xd returns 409 without resending bootstrap, so
                // remove/re-add emits a fresh bootstrap. Banned and other
                // non-active states are never rewritten by this repair path.
                remove_owner_member(owner_client, &encoded_group, binding.attach_agent_id).await?;
            }
            GroupMemberState::Absent => {}
            GroupMemberState::Banned => {
                return Err(format!(
                    "managed-agent child {} is banned in the owner community roster; explicitly unban it before starting the runtime",
                    binding.attach_agent_id
                ));
            }
            GroupMemberState::Other(state) => {
                return Err(format!(
                    "managed-agent child {} has non-active community state {state:?}; manual roster repair is required",
                    binding.attach_agent_id
                ));
            }
        }

        let members_path = format!("{group_path}/members");
        let add_body = group_member_body(binding.attach_agent_id, &record.name);
        let add_result: Result<Value, X0xClientError> =
            owner_client.post_json(&members_path, &add_body).await;
        match add_result {
            Ok(_) | Err(X0xClientError::Status(409, _)) => {}
            Err(error) => return Err(format!("managed-agent community attach failed: {error}")),
        }
        owner_group = owner_client
            .get_json(&group_path, &[])
            .await
            .map_err(|error| format!("community post-attach lookup failed: {error}"))?;
        require_active_member(
            &owner_group,
            binding.attach_agent_id,
            "owner community roster after attach",
        )?;
        expected_roster_revision = roster_revision(&owner_group)?;
    }

    wait_for_child_group(
        loopback_http,
        child,
        &group_path,
        group_id,
        expected_roster_revision,
        None,
    )
    .await?;

    // Compatibility migration: older UI code inserted record.pubkey under the
    // assumption that it was the AgentId. Remove that stale roster entry only
    // after the actual child is confirmed installed, so migration cannot leave
    // the community without a working managed-agent member.
    if let Some(legacy_agent_id) = binding.remove_legacy_agent_id {
        owner_group = owner_client
            .get_json(&group_path, &[])
            .await
            .map_err(|error| format!("community migration lookup failed: {error}"))?;
        if group_member_state(&owner_group, legacy_agent_id) == GroupMemberState::Active {
            remove_owner_member(owner_client, &encoded_group, legacy_agent_id).await?;
            owner_group = owner_client
                .get_json(&group_path, &[])
                .await
                .map_err(|error| format!("community post-migration lookup failed: {error}"))?;
            require_active_member(
                &owner_group,
                binding.attach_agent_id,
                "owner community roster after migration",
            )?;
            expected_roster_revision = roster_revision(&owner_group)?;
            wait_for_child_group(
                loopback_http,
                child,
                &group_path,
                group_id,
                expected_roster_revision,
                Some(legacy_agent_id),
            )
            .await?;
        }
    }

    Ok(())
}

async fn child_group_matches(
    loopback_http: &reqwest::Client,
    child: &ManagedAgentChild,
    group_path: &str,
    expected_roster_revision: u64,
    forbidden_agent_id: Option<&str>,
) -> Result<bool, String> {
    match child_request_json(
        loopback_http,
        &child.data_dir,
        Method::GET,
        group_path,
        None,
    )
    .await
    {
        Ok(group) => group_satisfies_convergence(
            &group,
            &child.agent_id,
            expected_roster_revision,
            forbidden_agent_id,
        ),
        Err(ChildHttpError {
            status: Some(StatusCode::NOT_FOUND),
            ..
        }) => Ok(false),
        Err(error) => Err(format!("managed-agent community lookup failed: {error}")),
    }
}

async fn wait_for_child_group(
    loopback_http: &reqwest::Client,
    child: &ManagedAgentChild,
    group_path: &str,
    group_id: &str,
    expected_roster_revision: u64,
    forbidden_agent_id: Option<&str>,
) -> Result<(), String> {
    let deadline = tokio::time::Instant::now() + GROUP_CONVERGENCE_TIMEOUT;
    loop {
        if child_group_matches(
            loopback_http,
            child,
            group_path,
            expected_roster_revision,
            forbidden_agent_id,
        )
        .await?
        {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(format!(
                "managed-agent community {group_id} did not converge on child {} at roster revision {expected_roster_revision} within {} seconds",
                child.agent_id,
                GROUP_CONVERGENCE_TIMEOUT.as_secs()
            ));
        }
        tokio::time::sleep(GROUP_CONVERGENCE_POLL).await;
    }
}

async fn remove_owner_member(
    owner_client: &X0xClient,
    encoded_group: &str,
    agent_id: &str,
) -> Result<(), String> {
    let encoded_agent = crate::path_segment(agent_id)?;
    let _: Value = owner_client
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GroupMemberState<'a> {
    Absent,
    Active,
    Banned,
    Other(&'a str),
}

fn roster_revision(group: &Value) -> Result<u64, String> {
    group
        .get("roster_revision")
        .and_then(Value::as_u64)
        .ok_or_else(|| "x0xd community response is missing roster_revision".to_string())
}

fn group_member_state<'a>(group: &'a Value, agent_id: &str) -> GroupMemberState<'a> {
    let state = group
        .get("members")
        .and_then(Value::as_array)
        .and_then(|members| {
            members
                .iter()
                .find(|member| member.get("agent_id").and_then(Value::as_str) == Some(agent_id))
        })
        .and_then(|member| member.get("state"))
        .and_then(Value::as_str);
    match state {
        None => GroupMemberState::Absent,
        Some("active") => GroupMemberState::Active,
        Some("banned") => GroupMemberState::Banned,
        Some(other) => GroupMemberState::Other(other),
    }
}

fn reject_banned_member(
    state: GroupMemberState<'_>,
    agent_id: &str,
    location: &str,
) -> Result<(), String> {
    if state == GroupMemberState::Banned {
        return Err(format!(
            "managed-agent child {agent_id} is banned in the {location}; explicitly unban it before starting the runtime"
        ));
    }
    Ok(())
}

fn require_active_member(group: &Value, agent_id: &str, location: &str) -> Result<(), String> {
    let state = group_member_state(group, agent_id);
    reject_banned_member(state, agent_id, location)?;
    if state != GroupMemberState::Active {
        return Err(format!(
            "managed-agent child {agent_id} is not active in the {location}; manual roster repair is required"
        ));
    }
    Ok(())
}

fn group_satisfies_convergence(
    group: &Value,
    child_agent_id: &str,
    expected_roster_revision: u64,
    forbidden_agent_id: Option<&str>,
) -> Result<bool, String> {
    let child_state = group_member_state(group, child_agent_id);
    reject_banned_member(child_state, child_agent_id, "managed-agent child community")?;
    let contains_forbidden_active = forbidden_agent_id
        .is_some_and(|id| group_member_state(group, id) == GroupMemberState::Active);
    Ok(child_state == GroupMemberState::Active
        && !contains_forbidden_active
        && roster_revision(group)? == expected_roster_revision)
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

    const PROXY_TEST_CHILD: &str = "BUZZ_LOOPBACK_PROXY_TEST_CHILD";

    #[tokio::test(flavor = "current_thread")]
    async fn loopback_http_client_bypasses_system_proxy() {
        if std::env::var_os(PROXY_TEST_CHILD).is_some() {
            run_loopback_proxy_probe().await;
            return;
        }

        let test_binary = std::env::current_exe().expect("resolve current test binary");
        let output = std::process::Command::new(test_binary)
            .args([
                "--exact",
                "managed_agents::orchestration::tests::loopback_http_client_bypasses_system_proxy",
                "--nocapture",
            ])
            .env(PROXY_TEST_CHILD, "1")
            .output()
            .expect("run isolated proxy environment regression");
        assert!(
            output.status.success(),
            "isolated proxy regression failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    async fn run_loopback_proxy_probe() {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

        let target = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind target listener");
        let target_addr = target.local_addr().expect("read target address");
        let proxy = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind proxy trap");
        let proxy_addr = proxy.local_addr().expect("read proxy address");
        let proxy_url = format!("http://{proxy_addr}");
        for key in ["HTTP_PROXY", "http_proxy", "ALL_PROXY", "all_proxy"] {
            std::env::set_var(key, &proxy_url);
        }
        for key in ["NO_PROXY", "no_proxy"] {
            std::env::remove_var(key);
        }

        let target_task = tokio::spawn(async move {
            let Ok(Ok((mut stream, _))) =
                tokio::time::timeout(Duration::from_secs(2), target.accept()).await
            else {
                return false;
            };
            let mut request = [0_u8; 1024];
            let _ = stream.read(&mut request).await;
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
                .await
                .is_ok()
        });
        let proxy_task = tokio::spawn(async move {
            let Ok(Ok((mut stream, _))) =
                tokio::time::timeout(Duration::from_millis(500), proxy.accept()).await
            else {
                return false;
            };
            let mut request = [0_u8; 1024];
            let _ = stream.read(&mut request).await;
            let _ = stream
                .write_all(
                    b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .await;
            true
        });

        let response = loopback_http_client()
            .expect("build no-proxy client")
            .get(format!("http://{target_addr}/health"))
            .timeout(Duration::from_secs(2))
            .send()
            .await
            .expect("direct loopback request");

        assert!(response.status().is_success());
        assert!(target_task.await.expect("join target task"));
        assert!(!proxy_task.await.expect("join proxy task"));
    }

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

    #[test]
    fn managed_allowlist_translation_preserves_external_agent_ids() {
        let legacy_managed_key = "aa".repeat(32);
        let managed_child_id = "bb".repeat(32);
        let external_agent_id = "cc".repeat(32);
        let normalized = vec![legacy_managed_key.clone(), external_agent_id.clone()];
        let translated = HashMap::from([(legacy_managed_key, managed_child_id.clone())]);

        let effective = translate_managed_allowlist(&normalized, &translated).unwrap();

        assert_eq!(effective, vec![managed_child_id, external_agent_id]);
    }

    #[test]
    fn banned_child_is_rejected_on_owner_and_child_rosters() {
        let child_id = "bb".repeat(32);
        let group = json!({
            "roster_revision": 7,
            "members": [{ "agent_id": child_id, "state": "banned" }],
        });
        let state = group_member_state(&group, &child_id);

        let owner_error = reject_banned_member(state, &child_id, "owner community roster")
            .expect_err("owner must not auto-repair a banned child");
        let child_error = group_satisfies_convergence(&group, &child_id, 7, None)
            .expect_err("child-side banned state must not converge");

        assert!(owner_error.contains("explicitly unban"));
        assert!(child_error.contains("explicitly unban"));
    }

    #[test]
    fn banned_legacy_member_is_not_an_active_cleanup_target() {
        let legacy_id = "aa".repeat(32);
        let group = json!({
            "members": [{ "agent_id": legacy_id, "state": "banned" }],
        });

        assert_eq!(
            group_member_state(&group, &legacy_id),
            GroupMemberState::Banned
        );
        assert_ne!(
            group_member_state(&group, &legacy_id),
            GroupMemberState::Active
        );
    }

    #[test]
    fn stale_child_roster_revision_does_not_converge() {
        let child_id = "bb".repeat(32);
        let group = json!({
            "roster_revision": 6,
            "members": [{ "agent_id": child_id, "state": "active" }],
        });

        assert!(!group_satisfies_convergence(&group, &child_id, 7, None).unwrap());
        assert!(group_satisfies_convergence(&group, &child_id, 6, None).unwrap());
    }
}
