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
/// A healthy child API can become reachable shortly before its persisted
/// communities are visible. Never rewrite an already-active owner roster on
/// that first transient gap; give the child a bounded restore window first.
const CHILD_GROUP_RESTORE_GRACE: Duration = Duration::from_secs(3);

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
    let mut owner_state = read_owner_group_state(
        owner_client,
        &OwnerGroupExpectation {
            group_path: &group_path,
            group_id,
            roster_revision: expected_roster_revision,
            required_agent_id: binding.attach_agent_id,
            required_member_state: owner_child_state,
            forbidden_agent_id: None,
        },
    )
    .await?;
    let needs_attach = match owner_child_state {
        GroupMemberState::Active => {
            let observation = wait_for_child_group_observation(
                loopback_http,
                child,
                &ChildGroupExpectation {
                    group_path: &group_path,
                    group_id,
                    owner_state: &owner_state,
                    forbidden_agent_id: None,
                },
                CHILD_GROUP_RESTORE_GRACE,
                GROUP_CONVERGENCE_POLL,
            )
            .await?;
            match active_owner_action(observation) {
                ActiveOwnerAction::Keep => false,
                ActiveOwnerAction::RejectStale => {
                    return Err(format!(
                        "managed-agent community {group_id} is present on child {} but is not equivalent to the owner state at revision {expected_roster_revision}; refusing to rewrite an active owner roster",
                        child.agent_id
                    ));
                }
                ActiveOwnerAction::RepairMissing => {
                    // Only a group that remained absent for the entire restore
                    // grace can use the legacy bootstrap repair. Re-read the
                    // owner first so an in-flight roster edit is never
                    // overwritten by this remove/re-add.
                    let refreshed_state = read_owner_group_state(
                        owner_client,
                        &OwnerGroupExpectation {
                            group_path: &group_path,
                            group_id,
                            roster_revision: expected_roster_revision,
                            required_agent_id: binding.attach_agent_id,
                            required_member_state: GroupMemberState::Active,
                            forbidden_agent_id: None,
                        },
                    )
                    .await?;
                    if refreshed_state != owner_state {
                        return Err(format!(
                            "managed-agent community {group_id} changed while child bootstrap repair was waiting; retry without rewriting the owner roster"
                        ));
                    }
                    remove_owner_member(owner_client, &encoded_group, binding.attach_agent_id)
                        .await?;
                    true
                }
            }
        }
        GroupMemberState::Absent => true,
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
    };
    if needs_attach {
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
        owner_state = read_owner_group_state(
            owner_client,
            &OwnerGroupExpectation {
                group_path: &group_path,
                group_id,
                roster_revision: expected_roster_revision,
                required_agent_id: binding.attach_agent_id,
                required_member_state: GroupMemberState::Active,
                forbidden_agent_id: None,
            },
        )
        .await?;
    }

    wait_for_child_group(
        loopback_http,
        child,
        &group_path,
        group_id,
        &owner_state,
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
            owner_state = read_owner_group_state(
                owner_client,
                &OwnerGroupExpectation {
                    group_path: &group_path,
                    group_id,
                    roster_revision: expected_roster_revision,
                    required_agent_id: binding.attach_agent_id,
                    required_member_state: GroupMemberState::Active,
                    forbidden_agent_id: Some(legacy_agent_id),
                },
            )
            .await?;
            wait_for_child_group(
                loopback_http,
                child,
                &group_path,
                group_id,
                &owner_state,
                Some(legacy_agent_id),
            )
            .await?;
        }
    }

    Ok(())
}

fn require_signed_public_group(group: &Value, group_id: &str) -> Result<(), String> {
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct GroupStateProjection {
    group_id: String,
    genesis: Value,
    roster_root: String,
    policy_hash: String,
    public_meta_hash: String,
    security_binding: Value,
    withdrawn: bool,
    state_hash: String,
    state_revision: u64,
}

impl GroupStateProjection {
    fn from_value(value: &Value, expected_group_id: &str) -> Result<Self, String> {
        fn required_string(value: &Value, field: &str) -> Result<String, String> {
            value
                .get(field)
                .and_then(Value::as_str)
                .filter(|text| !text.is_empty())
                .map(str::to_string)
                .ok_or_else(|| format!("x0xd group state is missing {field}"))
        }

        let group_id = required_string(value, "group_id")?;
        if group_id != expected_group_id {
            return Err("x0xd group state returned a different group_id".to_string());
        }
        let genesis = value
            .get("genesis")
            .filter(|entry| entry.is_object())
            .cloned()
            .ok_or_else(|| "x0xd group state is missing genesis".to_string())?;
        if genesis.get("group_id").and_then(Value::as_str) != Some(expected_group_id) {
            return Err("x0xd group state genesis returned a different group_id".to_string());
        }
        let security_binding = value
            .get("security_binding")
            .cloned()
            .ok_or_else(|| "x0xd group state is missing security_binding".to_string())?;
        let withdrawn = value
            .get("withdrawn")
            .and_then(Value::as_bool)
            .ok_or_else(|| "x0xd group state is missing withdrawn".to_string())?;
        let state_revision = value
            .get("state_revision")
            .and_then(Value::as_u64)
            .ok_or_else(|| "x0xd group state is missing state_revision".to_string())?;

        Ok(Self {
            group_id,
            genesis,
            roster_root: required_string(value, "roster_root")?,
            policy_hash: required_string(value, "policy_hash")?,
            public_meta_hash: required_string(value, "public_meta_hash")?,
            security_binding,
            withdrawn,
            state_hash: required_string(value, "state_hash")?,
            state_revision,
        })
    }

    fn authoritative_projection_eq(&self, other: &Self) -> bool {
        self.group_id == other.group_id
            && self.genesis == other.genesis
            && self.roster_root == other.roster_root
            && self.policy_hash == other.policy_hash
            && self.public_meta_hash == other.public_meta_hash
            && self.security_binding == other.security_binding
            && self.withdrawn == other.withdrawn
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChildGroupObservation {
    Missing,
    Strict,
    Equivalent,
    PresentStale,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ActiveOwnerAction {
    Keep,
    RepairMissing,
    RejectStale,
}

fn active_owner_action(observation: ChildGroupObservation) -> ActiveOwnerAction {
    match observation {
        ChildGroupObservation::Strict | ChildGroupObservation::Equivalent => {
            ActiveOwnerAction::Keep
        }
        ChildGroupObservation::Missing => ActiveOwnerAction::RepairMissing,
        ChildGroupObservation::PresentStale => ActiveOwnerAction::RejectStale,
    }
}

#[derive(Debug, Clone, Copy)]
struct ChildGroupExpectation<'a> {
    group_path: &'a str,
    group_id: &'a str,
    owner_state: &'a GroupStateProjection,
    forbidden_agent_id: Option<&'a str>,
}

#[derive(Debug, Clone, Copy)]
struct OwnerGroupExpectation<'a> {
    group_path: &'a str,
    group_id: &'a str,
    roster_revision: u64,
    required_agent_id: &'a str,
    required_member_state: GroupMemberState<'a>,
    forbidden_agent_id: Option<&'a str>,
}

async fn read_owner_group_state(
    owner_client: &X0xClient,
    expectation: &OwnerGroupExpectation<'_>,
) -> Result<GroupStateProjection, String> {
    let state_path = format!("{}/state", expectation.group_path);
    let state_before: Value = owner_client
        .get_json(&state_path, &[])
        .await
        .map_err(|error| format!("owner community state lookup failed: {error}"))?;
    let projection_before = GroupStateProjection::from_value(&state_before, expectation.group_id)?;
    let group: Value = owner_client
        .get_json(expectation.group_path, &[])
        .await
        .map_err(|error| format!("owner community snapshot lookup failed: {error}"))?;
    let state_after: Value = owner_client
        .get_json(&state_path, &[])
        .await
        .map_err(|error| format!("owner community state recheck failed: {error}"))?;
    let projection_after = GroupStateProjection::from_value(&state_after, expectation.group_id)?;
    validate_owner_group_snapshot(&projection_before, &group, &projection_after, expectation)?;
    if projection_after.withdrawn {
        return Err(format!(
            "owner community {} is withdrawn; managed-agent launch is unavailable",
            expectation.group_id
        ));
    }
    Ok(projection_after)
}

fn validate_owner_group_snapshot(
    state_before: &GroupStateProjection,
    group: &Value,
    state_after: &GroupStateProjection,
    expectation: &OwnerGroupExpectation<'_>,
) -> Result<(), String> {
    require_signed_public_group(group, expectation.group_id)?;
    if roster_revision(group)? != expectation.roster_revision {
        return Err(format!(
            "owner community roster changed while its state was being read (expected revision {})",
            expectation.roster_revision
        ));
    }
    if group_member_state(group, expectation.required_agent_id) != expectation.required_member_state
    {
        return Err(
            "owner community required membership changed while its state was being read"
                .to_string(),
        );
    }
    if expectation
        .forbidden_agent_id
        .is_some_and(|agent_id| group_member_state(group, agent_id) == GroupMemberState::Active)
    {
        return Err(
            "owner community forbidden legacy membership remained active after migration"
                .to_string(),
        );
    }
    if state_before != state_after {
        return Err("owner community state changed while it was being read".to_string());
    }
    Ok(())
}

async fn observe_child_group(
    loopback_http: &reqwest::Client,
    child: &ManagedAgentChild,
    expectation: &ChildGroupExpectation<'_>,
) -> Result<ChildGroupObservation, String> {
    let state_path = format!("{}/state", expectation.group_path);
    let state_before =
        read_child_group_state(loopback_http, child, &state_path, expectation.group_id).await?;
    let group = match child_request_json(
        loopback_http,
        &child.data_dir,
        Method::GET,
        expectation.group_path,
        None,
    )
    .await
    {
        Ok(group) => Some(group),
        Err(ChildHttpError {
            status: Some(StatusCode::NOT_FOUND),
            ..
        }) => None,
        Err(error) => return Err(format!("managed-agent community lookup failed: {error}")),
    };
    let state_after =
        read_child_group_state(loopback_http, child, &state_path, expectation.group_id).await?;
    let Some(group) = group else {
        return Ok(if state_before.is_none() && state_after.is_none() {
            ChildGroupObservation::Missing
        } else {
            ChildGroupObservation::PresentStale
        });
    };
    require_signed_public_group(&group, expectation.group_id)?;

    let child_state = group_member_state(&group, &child.agent_id);
    reject_banned_member(
        child_state,
        &child.agent_id,
        "managed-agent child community",
    )?;
    let contains_forbidden_active = expectation
        .forbidden_agent_id
        .is_some_and(|id| group_member_state(&group, id) == GroupMemberState::Active);
    if child_state != GroupMemberState::Active || contains_forbidden_active {
        return Ok(ChildGroupObservation::PresentStale);
    }

    let (Some(child_projection_before), Some(child_projection_after)) = (state_before, state_after)
    else {
        return Ok(ChildGroupObservation::PresentStale);
    };
    if child_projection_before != child_projection_after {
        return Ok(ChildGroupObservation::PresentStale);
    }
    if child_projection_after.withdrawn {
        return Ok(ChildGroupObservation::PresentStale);
    }
    if child_projection_after.state_revision == expectation.owner_state.state_revision
        && child_projection_after.state_hash == expectation.owner_state.state_hash
    {
        return Ok(ChildGroupObservation::Strict);
    }
    if child_projection_after.authoritative_projection_eq(expectation.owner_state) {
        return Ok(ChildGroupObservation::Equivalent);
    }
    Ok(ChildGroupObservation::PresentStale)
}

async fn read_child_group_state(
    loopback_http: &reqwest::Client,
    child: &ManagedAgentChild,
    state_path: &str,
    group_id: &str,
) -> Result<Option<GroupStateProjection>, String> {
    match child_request_json(
        loopback_http,
        &child.data_dir,
        Method::GET,
        state_path,
        None,
    )
    .await
    {
        Ok(state) => GroupStateProjection::from_value(&state, group_id).map(Some),
        Err(ChildHttpError {
            status: Some(StatusCode::NOT_FOUND),
            ..
        }) => Ok(None),
        Err(error) => Err(format!(
            "managed-agent community state lookup failed: {error}"
        )),
    }
}

async fn wait_for_child_group_observation(
    loopback_http: &reqwest::Client,
    child: &ManagedAgentChild,
    expectation: &ChildGroupExpectation<'_>,
    timeout: Duration,
    poll: Duration,
) -> Result<ChildGroupObservation, String> {
    let deadline = tokio::time::Instant::now() + timeout;
    let mut saw_present = false;
    loop {
        let observation = observe_child_group(loopback_http, child, expectation).await?;
        match observation {
            ChildGroupObservation::Strict | ChildGroupObservation::Equivalent => {
                return Ok(observation);
            }
            ChildGroupObservation::PresentStale => saw_present = true,
            ChildGroupObservation::Missing => {}
        }
        if tokio::time::Instant::now() >= deadline {
            return Ok(if saw_present {
                ChildGroupObservation::PresentStale
            } else {
                ChildGroupObservation::Missing
            });
        }
        tokio::time::sleep(poll).await;
    }
}

async fn wait_for_child_group(
    loopback_http: &reqwest::Client,
    child: &ManagedAgentChild,
    group_path: &str,
    group_id: &str,
    owner_state: &GroupStateProjection,
    forbidden_agent_id: Option<&str>,
) -> Result<(), String> {
    let observation = wait_for_child_group_observation(
        loopback_http,
        child,
        &ChildGroupExpectation {
            group_path,
            group_id,
            owner_state,
            forbidden_agent_id,
        },
        GROUP_CONVERGENCE_TIMEOUT,
        GROUP_CONVERGENCE_POLL,
    )
    .await?;
    match observation {
        ChildGroupObservation::Strict => return Ok(()),
        ChildGroupObservation::Equivalent => {
            eprintln!(
                "tic-tac-toe: managed-agent community {group_id} has an equivalent signed-public projection on child {} but a different signed head; launching without owner roster mutation",
                child.agent_id
            );
            return Ok(());
        }
        ChildGroupObservation::Missing | ChildGroupObservation::PresentStale => {}
    }
    Err(format!(
        "managed-agent community {group_id} did not converge on child {} at roster revision {} within {} seconds ({observation:?})",
        child.agent_id,
        owner_state.state_revision,
        GROUP_CONVERGENCE_TIMEOUT.as_secs()
    ))
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
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    use axum::{
        extract::State, http::StatusCode as AxumStatusCode, response::IntoResponse, routing::get,
        Json, Router,
    };

    const PROXY_TEST_CHILD: &str = "BUZZ_LOOPBACK_PROXY_TEST_CHILD";

    #[derive(Clone)]
    struct ScriptedChildApi {
        group_calls: Arc<AtomicUsize>,
        group_responses: Arc<Vec<Option<Value>>>,
        state_calls: Arc<AtomicUsize>,
        state_responses: Arc<Vec<Option<Value>>>,
    }

    async fn scripted_group(State(script): State<ScriptedChildApi>) -> impl IntoResponse {
        let index = script.group_calls.fetch_add(1, Ordering::SeqCst);
        let response = script
            .group_responses
            .get(index)
            .or_else(|| script.group_responses.last())
            .cloned()
            .flatten();
        match response {
            Some(group) => (AxumStatusCode::OK, Json(group)).into_response(),
            None => (AxumStatusCode::NOT_FOUND, Json(json!({}))).into_response(),
        }
    }

    async fn scripted_group_state(State(script): State<ScriptedChildApi>) -> impl IntoResponse {
        let index = script.state_calls.fetch_add(1, Ordering::SeqCst);
        let response = script
            .state_responses
            .get(index)
            .or_else(|| script.state_responses.last())
            .cloned()
            .flatten();
        match response {
            Some(state) => (AxumStatusCode::OK, Json(state)).into_response(),
            None => (AxumStatusCode::NOT_FOUND, Json(json!({}))).into_response(),
        }
    }

    async fn spawn_scripted_child_api(
        agent_id: &str,
        group_responses: Vec<Option<Value>>,
        state: Value,
    ) -> (
        tempfile::TempDir,
        ManagedAgentChild,
        Arc<AtomicUsize>,
        tokio::task::JoinHandle<()>,
    ) {
        spawn_scripted_child_api_with_states(agent_id, group_responses, vec![Some(state)]).await
    }

    async fn spawn_scripted_child_api_with_states(
        agent_id: &str,
        group_responses: Vec<Option<Value>>,
        state_responses: Vec<Option<Value>>,
    ) -> (
        tempfile::TempDir,
        ManagedAgentChild,
        Arc<AtomicUsize>,
        tokio::task::JoinHandle<()>,
    ) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind scripted child API");
        let address = listener.local_addr().expect("read child API address");
        let data_dir = tempfile::tempdir().expect("create child data dir");
        std::fs::write(
            data_dir.path().join("api.port"),
            format!("127.0.0.1:{}", address.port()),
        )
        .expect("write child api.port");
        std::fs::write(data_dir.path().join("api-token"), "test-token")
            .expect("write child api-token");

        let group_calls = Arc::new(AtomicUsize::new(0));
        let script = ScriptedChildApi {
            group_calls: group_calls.clone(),
            group_responses: Arc::new(group_responses),
            state_calls: Arc::new(AtomicUsize::new(0)),
            state_responses: Arc::new(state_responses),
        };
        let router = Router::new()
            .route("/groups/{group_id}", get(scripted_group))
            .route("/groups/{group_id}/state", get(scripted_group_state))
            .with_state(script);
        let task = tokio::spawn(async move {
            axum::serve(listener, router)
                .await
                .expect("serve scripted child API");
        });
        let child = ManagedAgentChild {
            agent_id: agent_id.to_string(),
            data_dir: data_dir.path().to_path_buf(),
        };
        (data_dir, child, group_calls, task)
    }

    fn group_json(agent_id: &str, revision: u64, state: &str) -> Value {
        json!({
            "policy": { "confidentiality": "signed_public" },
            "roster_revision": revision,
            "members": [{ "agent_id": agent_id, "state": state }],
        })
    }

    fn group_state_json(
        group_id: &str,
        revision: u64,
        state_hash: &str,
        roster_root: &str,
    ) -> Value {
        json!({
            "group_id": group_id,
            "genesis": {
                "group_id": group_id,
                "creator_agent_id": "owner",
                "created_at": 1,
                "creation_nonce": "nonce",
            },
            "roster_root": roster_root,
            "policy_hash": "policy-hash",
            "public_meta_hash": "public-meta-hash",
            "security_binding": null,
            "withdrawn": false,
            "state_hash": state_hash,
            "state_revision": revision,
        })
    }

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
    fn banned_child_is_rejected_on_owner_roster() {
        let child_id = "bb".repeat(32);
        let group = json!({
            "roster_revision": 7,
            "members": [{ "agent_id": child_id, "state": "banned" }],
        });
        let state = group_member_state(&group, &child_id);

        let owner_error = reject_banned_member(state, &child_id, "owner community roster")
            .expect_err("owner must not auto-repair a banned child");

        assert!(owner_error.contains("explicitly unban"));
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

    #[tokio::test]
    async fn transient_missing_group_converges_without_becoming_repairable() {
        let group_id = "group";
        let child_id = "bb".repeat(32);
        let owner_state = GroupStateProjection::from_value(
            &group_state_json(group_id, 22, "shared-hash", "shared-roster"),
            group_id,
        )
        .expect("owner projection");
        let (_data_dir, child, calls, task) = spawn_scripted_child_api(
            &child_id,
            vec![None, Some(group_json(&child_id, 22, "active"))],
            group_state_json(group_id, 22, "shared-hash", "shared-roster"),
        )
        .await;

        let observation = wait_for_child_group_observation(
            &loopback_http_client().expect("HTTP client"),
            &child,
            &ChildGroupExpectation {
                group_path: &format!("/groups/{group_id}"),
                group_id,
                owner_state: &owner_state,
                forbidden_agent_id: None,
            },
            Duration::from_millis(100),
            Duration::from_millis(1),
        )
        .await
        .expect("observe restored group");

        assert_eq!(observation, ChildGroupObservation::Strict);
        assert!(calls.load(Ordering::SeqCst) >= 2);
        task.abort();
    }

    #[tokio::test]
    async fn persistent_missing_group_is_the_only_repairable_observation() {
        let group_id = "group";
        let child_id = "bb".repeat(32);
        let owner_state = GroupStateProjection::from_value(
            &group_state_json(group_id, 22, "shared-hash", "shared-roster"),
            group_id,
        )
        .expect("owner projection");
        let (_data_dir, child, calls, task) =
            spawn_scripted_child_api_with_states(&child_id, vec![None], vec![None]).await;

        let observation = wait_for_child_group_observation(
            &loopback_http_client().expect("HTTP client"),
            &child,
            &ChildGroupExpectation {
                group_path: &format!("/groups/{group_id}"),
                group_id,
                owner_state: &owner_state,
                forbidden_agent_id: None,
            },
            Duration::from_millis(10),
            Duration::from_millis(1),
        )
        .await
        .expect("observe missing group");

        assert_eq!(observation, ChildGroupObservation::Missing);
        assert!(calls.load(Ordering::SeqCst) >= 2);
        task.abort();
    }

    #[tokio::test]
    async fn any_present_stale_snapshot_prevents_destructive_missing_repair() {
        let group_id = "group";
        let child_id = "bb".repeat(32);
        let owner_state = GroupStateProjection::from_value(
            &group_state_json(group_id, 22, "owner-hash", "owner-roster"),
            group_id,
        )
        .expect("owner projection");
        let (_data_dir, child, _calls, task) = spawn_scripted_child_api(
            &child_id,
            vec![Some(group_json(&child_id, 21, "active")), None],
            group_state_json(group_id, 21, "child-hash", "child-roster"),
        )
        .await;

        let observation = wait_for_child_group_observation(
            &loopback_http_client().expect("HTTP client"),
            &child,
            &ChildGroupExpectation {
                group_path: &format!("/groups/{group_id}"),
                group_id,
                owner_state: &owner_state,
                forbidden_agent_id: None,
            },
            Duration::from_millis(10),
            Duration::from_millis(1),
        )
        .await
        .expect("observe stale group");

        assert_eq!(observation, ChildGroupObservation::PresentStale);
        task.abort();
    }

    #[tokio::test]
    async fn authoritative_projection_accepts_equivalent_revision_history() {
        let group_id = "group";
        let child_id = "bb".repeat(32);
        let owner_state = GroupStateProjection::from_value(
            &group_state_json(group_id, 26, "owner-hash", "shared-roster"),
            group_id,
        )
        .expect("owner projection");
        let (_data_dir, child, _calls, task) = spawn_scripted_child_api(
            &child_id,
            vec![Some(group_json(&child_id, 22, "active"))],
            group_state_json(group_id, 22, "child-hash", "shared-roster"),
        )
        .await;

        let observation = observe_child_group(
            &loopback_http_client().expect("HTTP client"),
            &child,
            &ChildGroupExpectation {
                group_path: &format!("/groups/{group_id}"),
                group_id,
                owner_state: &owner_state,
                forbidden_agent_id: None,
            },
        )
        .await
        .expect("observe equivalent group");

        assert_eq!(observation, ChildGroupObservation::Equivalent);
        task.abort();
    }

    #[tokio::test]
    async fn policy_only_state_revision_can_converge_strictly() {
        let group_id = "group";
        let child_id = "bb".repeat(32);
        let owner_state = GroupStateProjection::from_value(
            &group_state_json(group_id, 26, "shared-hash", "shared-roster"),
            group_id,
        )
        .expect("owner projection");
        let (_data_dir, child, _calls, task) = spawn_scripted_child_api(
            &child_id,
            vec![Some(group_json(&child_id, 22, "active"))],
            group_state_json(group_id, 26, "shared-hash", "shared-roster"),
        )
        .await;

        let observation = observe_child_group(
            &loopback_http_client().expect("HTTP client"),
            &child,
            &ChildGroupExpectation {
                group_path: &format!("/groups/{group_id}"),
                group_id,
                owner_state: &owner_state,
                forbidden_agent_id: None,
            },
        )
        .await
        .expect("observe strict policy-only revision");

        assert_eq!(observation, ChildGroupObservation::Strict);
        task.abort();
    }

    #[tokio::test]
    async fn policy_only_state_revision_can_converge_equivalently() {
        let group_id = "group";
        let child_id = "bb".repeat(32);
        let owner_state = GroupStateProjection::from_value(
            &group_state_json(group_id, 30, "owner-hash", "shared-roster"),
            group_id,
        )
        .expect("owner projection");
        let (_data_dir, child, _calls, task) = spawn_scripted_child_api(
            &child_id,
            vec![Some(group_json(&child_id, 22, "active"))],
            group_state_json(group_id, 26, "child-hash", "shared-roster"),
        )
        .await;

        let observation = observe_child_group(
            &loopback_http_client().expect("HTTP client"),
            &child,
            &ChildGroupExpectation {
                group_path: &format!("/groups/{group_id}"),
                group_id,
                owner_state: &owner_state,
                forbidden_agent_id: None,
            },
        )
        .await
        .expect("observe equivalent policy-only revision");

        assert_eq!(observation, ChildGroupObservation::Equivalent);
        task.abort();
    }

    #[tokio::test]
    async fn child_state_change_during_group_read_fails_closed() {
        let group_id = "group";
        let child_id = "bb".repeat(32);
        let owner_state = GroupStateProjection::from_value(
            &group_state_json(group_id, 26, "after-hash", "shared-roster"),
            group_id,
        )
        .expect("owner projection");
        let (_data_dir, child, _calls, task) = spawn_scripted_child_api_with_states(
            &child_id,
            vec![Some(group_json(&child_id, 22, "active"))],
            vec![
                Some(group_state_json(
                    group_id,
                    25,
                    "before-hash",
                    "shared-roster",
                )),
                Some(group_state_json(
                    group_id,
                    26,
                    "after-hash",
                    "shared-roster",
                )),
            ],
        )
        .await;

        let observation = observe_child_group(
            &loopback_http_client().expect("HTTP client"),
            &child,
            &ChildGroupExpectation {
                group_path: &format!("/groups/{group_id}"),
                group_id,
                owner_state: &owner_state,
                forbidden_agent_id: None,
            },
        )
        .await
        .expect("observe changing child state");

        assert_eq!(observation, ChildGroupObservation::PresentStale);
        task.abort();
    }

    #[tokio::test]
    async fn group_404_with_visible_state_is_not_repairable_missing() {
        let group_id = "group";
        let child_id = "bb".repeat(32);
        let owner_state = GroupStateProjection::from_value(
            &group_state_json(group_id, 26, "shared-hash", "shared-roster"),
            group_id,
        )
        .expect("owner projection");
        let (_data_dir, child, _calls, task) = spawn_scripted_child_api(
            &child_id,
            vec![None],
            group_state_json(group_id, 26, "shared-hash", "shared-roster"),
        )
        .await;

        let observation = observe_child_group(
            &loopback_http_client().expect("HTTP client"),
            &child,
            &ChildGroupExpectation {
                group_path: &format!("/groups/{group_id}"),
                group_id,
                owner_state: &owner_state,
                forbidden_agent_id: None,
            },
        )
        .await
        .expect("observe inconsistent 404");

        assert_eq!(observation, ChildGroupObservation::PresentStale);
        task.abort();
    }

    #[test]
    fn owner_state_change_during_group_read_fails_closed() {
        let group_id = "group";
        let child_id = "bb".repeat(32);
        let before = GroupStateProjection::from_value(
            &group_state_json(group_id, 25, "before-hash", "shared-roster"),
            group_id,
        )
        .expect("before projection");
        let after = GroupStateProjection::from_value(
            &group_state_json(group_id, 26, "after-hash", "shared-roster"),
            group_id,
        )
        .expect("after projection");
        let group = group_json(&child_id, 22, "active");
        let error = validate_owner_group_snapshot(
            &before,
            &group,
            &after,
            &OwnerGroupExpectation {
                group_path: &format!("/groups/{group_id}"),
                group_id,
                roster_revision: 22,
                required_agent_id: &child_id,
                required_member_state: GroupMemberState::Active,
                forbidden_agent_id: None,
            },
        )
        .expect_err("owner head change must fail closed");

        assert!(error.contains("state changed"));
    }

    #[test]
    fn authoritative_projection_requires_every_signed_public_field() {
        let group_id = "group";
        let owner = GroupStateProjection::from_value(
            &group_state_json(group_id, 26, "owner-hash", "shared-roster"),
            group_id,
        )
        .expect("owner projection");
        let mut equivalent = GroupStateProjection::from_value(
            &group_state_json(group_id, 22, "child-hash", "shared-roster"),
            group_id,
        )
        .expect("child projection");
        assert!(owner.authoritative_projection_eq(&equivalent));

        equivalent.group_id = "other-group".to_string();
        assert!(!owner.authoritative_projection_eq(&equivalent));
        equivalent = owner.clone();
        equivalent.genesis["creation_nonce"] = json!("other-nonce");
        assert!(!owner.authoritative_projection_eq(&equivalent));
        equivalent = owner.clone();
        equivalent.roster_root = "other-roster".to_string();
        assert!(!owner.authoritative_projection_eq(&equivalent));
        equivalent = owner.clone();
        equivalent.policy_hash = "other-policy".to_string();
        assert!(!owner.authoritative_projection_eq(&equivalent));
        equivalent = owner.clone();
        equivalent.public_meta_hash = "other-meta".to_string();
        assert!(!owner.authoritative_projection_eq(&equivalent));
        equivalent = owner.clone();
        equivalent.security_binding = json!({ "epoch": 1 });
        assert!(!owner.authoritative_projection_eq(&equivalent));
        equivalent = owner.clone();
        equivalent.withdrawn = true;
        assert!(!owner.authoritative_projection_eq(&equivalent));
    }

    #[test]
    fn active_owner_mutation_is_allowed_only_for_persistent_missing() {
        assert_eq!(
            active_owner_action(ChildGroupObservation::Strict),
            ActiveOwnerAction::Keep
        );
        assert_eq!(
            active_owner_action(ChildGroupObservation::Equivalent),
            ActiveOwnerAction::Keep
        );
        assert_eq!(
            active_owner_action(ChildGroupObservation::PresentStale),
            ActiveOwnerAction::RejectStale
        );
        assert_eq!(
            active_owner_action(ChildGroupObservation::Missing),
            ActiveOwnerAction::RepairMissing
        );
    }

    #[tokio::test]
    async fn member_equality_cannot_hide_authoritative_projection_mismatch() {
        let group_id = "group";
        let child_id = "bb".repeat(32);
        let owner_state = GroupStateProjection::from_value(
            &group_state_json(group_id, 26, "owner-hash", "owner-roster"),
            group_id,
        )
        .expect("owner projection");
        let (_data_dir, child, _calls, task) = spawn_scripted_child_api(
            &child_id,
            vec![Some(group_json(&child_id, 22, "active"))],
            group_state_json(group_id, 22, "child-hash", "child-roster"),
        )
        .await;

        let observation = observe_child_group(
            &loopback_http_client().expect("HTTP client"),
            &child,
            &ChildGroupExpectation {
                group_path: &format!("/groups/{group_id}"),
                group_id,
                owner_state: &owner_state,
                forbidden_agent_id: None,
            },
        )
        .await
        .expect("observe mismatched group");

        assert_eq!(observation, ChildGroupObservation::PresentStale);
        task.abort();
    }

    #[tokio::test]
    async fn banned_child_state_fails_closed_before_equivalence() {
        let group_id = "group";
        let child_id = "bb".repeat(32);
        let owner_state = GroupStateProjection::from_value(
            &group_state_json(group_id, 22, "shared-hash", "shared-roster"),
            group_id,
        )
        .expect("owner projection");
        let (_data_dir, child, _calls, task) = spawn_scripted_child_api(
            &child_id,
            vec![Some(group_json(&child_id, 22, "banned"))],
            group_state_json(group_id, 22, "shared-hash", "shared-roster"),
        )
        .await;

        let error = observe_child_group(
            &loopback_http_client().expect("HTTP client"),
            &child,
            &ChildGroupExpectation {
                group_path: &format!("/groups/{group_id}"),
                group_id,
                owner_state: &owner_state,
                forbidden_agent_id: None,
            },
        )
        .await
        .expect_err("banned child must fail closed");

        assert!(error.contains("explicitly unban"));
        task.abort();
    }

    #[tokio::test]
    async fn withdrawn_child_state_cannot_converge_even_with_matching_head() {
        let group_id = "group";
        let child_id = "bb".repeat(32);
        let owner_state = GroupStateProjection::from_value(
            &group_state_json(group_id, 22, "shared-hash", "shared-roster"),
            group_id,
        )
        .expect("owner projection");
        let mut withdrawn_state = group_state_json(group_id, 22, "shared-hash", "shared-roster");
        withdrawn_state["withdrawn"] = json!(true);
        let (_data_dir, child, _calls, task) = spawn_scripted_child_api(
            &child_id,
            vec![Some(group_json(&child_id, 22, "active"))],
            withdrawn_state,
        )
        .await;

        let observation = observe_child_group(
            &loopback_http_client().expect("HTTP client"),
            &child,
            &ChildGroupExpectation {
                group_path: &format!("/groups/{group_id}"),
                group_id,
                owner_state: &owner_state,
                forbidden_agent_id: None,
            },
        )
        .await
        .expect("observe withdrawn child state");

        assert_eq!(observation, ChildGroupObservation::PresentStale);
        task.abort();
    }

    #[tokio::test]
    async fn active_legacy_identity_blocks_equivalent_convergence() {
        let group_id = "group";
        let child_id = "bb".repeat(32);
        let legacy_id = "aa".repeat(32);
        let owner_state = GroupStateProjection::from_value(
            &group_state_json(group_id, 26, "owner-hash", "shared-roster"),
            group_id,
        )
        .expect("owner projection");
        let group = json!({
            "policy": { "confidentiality": "signed_public" },
            "roster_revision": 22,
            "members": [
                { "agent_id": child_id, "state": "active" },
                { "agent_id": legacy_id, "state": "active" },
            ],
        });
        let (_data_dir, child, _calls, task) = spawn_scripted_child_api(
            &child_id,
            vec![Some(group)],
            group_state_json(group_id, 22, "child-hash", "shared-roster"),
        )
        .await;

        let observation = observe_child_group(
            &loopback_http_client().expect("HTTP client"),
            &child,
            &ChildGroupExpectation {
                group_path: &format!("/groups/{group_id}"),
                group_id,
                owner_state: &owner_state,
                forbidden_agent_id: Some(&legacy_id),
            },
        )
        .await
        .expect("observe legacy identity");

        assert_eq!(observation, ChildGroupObservation::PresentStale);
        task.abort();
    }
}
