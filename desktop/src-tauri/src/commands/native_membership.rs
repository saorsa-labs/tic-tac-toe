//! Native named-group membership command surface (M3 workspace cutover).
//!
//! Typed Tauri commands over the token-authenticated loopback [`X0xClient`]
//! that proxy the embedded `x0xd` daemon's named-group REST surface (`/groups`,
//! `/groups/:id`, `/groups/:id/members`, `/groups/:id/ban/:agent_id`,
//! `/groups/:id/policy`, `/groups/:id/requests`).
//!
//! These are the native transport path the M3 membership cutover binds to.
//! They emit **no** relay events and publish **no** Nostr kinds, and they
//! perform **no** authority reconstruction (ADR-0001): roster/policy state is
//! accepted only as token-authenticated loopback delegation (the daemon's
//! transient bearer token over the loopback socket) and re-serialized for the
//! frontend — it carries display data, not a verified secure-group guarantee.
//! Every call resolves the transient daemon token/port inside
//! [`X0xClient`](crate::x0x_client::X0xClient) and returns a human-readable
//! `String` error (never the token) on failure.
//!
//! The one-time invite mint (`POST /groups/:id/invite`) and join
//! (`POST /groups/join`) surfaces are intentionally NOT exposed here: the
//! opaque invite contract cannot authenticate, version, or canonically bind
//! the secure-group bootstrap, so the invite bootstrap is gated pending x0x
//! frontier review. Public group creation and roster reads remain.
//!
//! # serde convention
//! Daemon responses are snake_case; the frozen TS seam
//! (`tauriNativeX0x.ts`) exposes camelCase types to features. Raw structs
//! deserialize the daemon verbatim; output structs carry
//! `#[serde(rename_all = "camelCase")]`; a trivial `From` bridges them. The
//! wrappers where the TS seam does its own key conversion
//! (`x0x_list_groups`, `x0x_get_group_members`) return the exact raw shape
//! the seam unpacks.

use serde::{Deserialize, Serialize};
use tauri::State;

use crate::app_state::AppState;
use crate::commands::native_auxiliary::path_segment;
use crate::commands::native_social::validate_agent_id;
use crate::x0x_client::X0xClient;

// ── TS-facing output types (camelCase) ───────────────────────────────────────

/// One roster entry — mirrors `X0xGroupMember` in `tauriNativeX0x.ts`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GroupMember {
    pub agent_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,
    pub role: String,
    pub state: String,
    pub display_name: Option<String>,
    pub joined_at_ms: i64,
    pub updated_at_ms: i64,
    pub added_by: Option<String>,
    pub removed_by: Option<String>,
}

/// Full named-group detail — mirrors `X0xNamedGroup`. `members` is
/// `#[serde(default)]`: GET /groups/:id may omit the inline roster (the UI
/// fetches the roster via `x0x_get_group_members`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NamedGroup {
    pub group_id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub creator: Option<String>,
    #[serde(default)]
    pub created_at_ms: i64,
    #[serde(default)]
    pub updated_at_ms: i64,
    #[serde(default)]
    pub chat_topic: String,
    #[serde(default)]
    pub metadata_topic: String,
    #[serde(default)]
    pub policy_revision: i64,
    #[serde(default)]
    pub roster_revision: i64,
    #[serde(default)]
    pub member_count: i64,
    #[serde(default)]
    pub members: Vec<GroupMember>,
    /// `policy.confidentiality` from the daemon (`signed_public` for creatable
    /// groups). The channel projection omits any group that is not
    /// `signed_public` so a non-public group is never laundered as an open
    /// channel.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidentiality: Option<String>,
}

/// Lightweight list entry — mirrors `X0xNamedGroupSummary`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NamedGroupSummary {
    pub group_id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub member_count: i64,
}

// ── Daemon-facing raw types (snake_case in) ──────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
struct RawGroupMember {
    agent_id: String,
    #[serde(default)]
    user_id: Option<String>,
    role: String,
    #[serde(default)]
    state: String,
    #[serde(default)]
    display_name: Option<String>,
    /// Daemon emits `joined_at` (ms epoch); renamed at the serde edge.
    #[serde(default, rename = "joined_at")]
    joined_at_ms: i64,
    /// Daemon omits `updated_at` for never-edited members; the `From` impl
    /// falls back to `joined_at_ms` so `updatedAtMs` is never stale-zero.
    #[serde(default, rename = "updated_at")]
    updated_at_ms: Option<i64>,
    #[serde(default)]
    added_by: Option<String>,
    #[serde(default)]
    removed_by: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct RawMembershipMutationResponse {
    #[serde(default)]
    members: Vec<RawGroupMember>,
}

fn mutation_member(
    response: RawMembershipMutationResponse,
    target_agent_id: &str,
) -> Result<GroupMember, String> {
    response
        .members
        .into_iter()
        .find(|member| member.agent_id == target_agent_id)
        .map(Into::into)
        .ok_or_else(|| "daemon membership response omitted the target member".to_string())
}

#[derive(Debug, Clone, Deserialize)]
struct RawGroupPolicy {
    #[serde(default)]
    confidentiality: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct RawNamedGroup {
    group_id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    creator: Option<String>,
    #[serde(default)]
    created_at_ms: i64,
    #[serde(default)]
    updated_at_ms: i64,
    #[serde(default)]
    chat_topic: String,
    #[serde(default)]
    metadata_topic: String,
    #[serde(default)]
    policy_revision: i64,
    #[serde(default)]
    roster_revision: i64,
    #[serde(default)]
    member_count: i64,
    #[serde(default)]
    members: Vec<RawGroupMember>,
    #[serde(default)]
    policy: Option<RawGroupPolicy>,
}

#[derive(Debug, Clone, Deserialize)]
struct RawNamedGroupSummary {
    group_id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    member_count: i64,
}

impl From<RawGroupMember> for GroupMember {
    fn from(r: RawGroupMember) -> Self {
        GroupMember {
            agent_id: r.agent_id,
            user_id: r.user_id,
            role: r.role,
            state: r.state,
            display_name: r.display_name,
            joined_at_ms: r.joined_at_ms,
            updated_at_ms: r.updated_at_ms.unwrap_or(r.joined_at_ms),
            added_by: r.added_by,
            removed_by: r.removed_by,
        }
    }
}

impl From<RawNamedGroup> for NamedGroup {
    fn from(r: RawNamedGroup) -> Self {
        NamedGroup {
            group_id: r.group_id,
            name: r.name,
            description: r.description,
            creator: r.creator,
            created_at_ms: r.created_at_ms,
            updated_at_ms: r.updated_at_ms,
            chat_topic: r.chat_topic,
            metadata_topic: r.metadata_topic,
            policy_revision: r.policy_revision,
            roster_revision: r.roster_revision,
            member_count: r.member_count,
            members: r.members.into_iter().map(Into::into).collect(),
            confidentiality: r.policy.and_then(|p| p.confidentiality),
        }
    }
}

impl From<RawNamedGroupSummary> for NamedGroupSummary {
    fn from(r: RawNamedGroupSummary) -> Self {
        NamedGroupSummary {
            group_id: r.group_id,
            name: r.name,
            description: r.description,
            member_count: r.member_count,
        }
    }
}

// ── Wire wrappers the TS seam unpacks (raw key shape) ────────────────────────

#[derive(Debug, Serialize)]
pub struct GroupsList {
    pub groups: Vec<NamedGroupSummary>,
}

#[derive(Debug, Serialize)]
pub struct MembersList {
    pub members: Vec<GroupMember>,
}

/// POST /groups response holder. The surface returns at least `group_id`; a
/// full group object also carries it, so this extracts the id whether the
/// daemon returns a thin ack or the full record. We then fetch the
/// authoritative full group via GET /groups/:id so create returns the
/// complete `NamedGroup` the frozen seam types promise.
#[derive(Debug, Deserialize)]
struct GroupIdHolder {
    #[serde(default)]
    group_id: Option<String>,
}

// ── Request bodies ───────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
struct CreateGroupBody<'a> {
    name: &'a str,
    #[serde(skip_serializing_if = "str::is_empty")]
    description: &'a str,
    display_name: Option<String>,
    preset: Option<String>,
}

#[derive(Debug, Serialize)]
struct AddMemberBody<'a> {
    agent_id: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    display_name: Option<&'a str>,
}

#[derive(Debug, Serialize)]
struct SetRoleBody<'a> {
    role: &'a str,
}

#[derive(Debug, Default, Serialize)]
struct UpdateGroupBody {
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
}

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Fetch the authoritative full group record. Used by create/join to satisfy
/// the `NamedGroup` return type after the daemon's thin ack.
async fn fetch_full_group(client: &X0xClient, group_id: &str) -> Result<NamedGroup, String> {
    let path = format!("/groups/{group_id}");
    let raw: RawNamedGroup = client.get_json(&path, &[]).await?;
    Ok(raw.into())
}

/// Validate the AgentId and percent-encode both dynamic path segments for a
/// membership mutation. Group ids are opaque daemon strings (encoded so `/`,
/// `?`, `#` cannot re-route the loopback request); agent ids must be 64-char
/// lowercase hex (validated, then encoded for uniformity).
fn encode_member_path_segments(group_id: &str, agent_id: &str) -> Result<(String, String), String> {
    let agent = path_segment(&validate_agent_id(agent_id)?)?;
    let group = path_segment(group_id)?;
    Ok((group, agent))
}

// ── Active group resolution ──────────────────────────────────────────────────

/// The active workspace's bound native named-group id (opaque stable string).
/// Fail-closed when no group is bound so callers never synthesize a relay-scope
/// surrogate. Consumed by `ManagedAgentRuntimeKey.group_id` and group-scoped
/// history/membership resolution.
#[tauri::command]
pub async fn x0x_get_active_group_id(state: State<'_, AppState>) -> Result<String, String> {
    let guard = state.active_group_id.lock().map_err(|e| e.to_string())?;
    guard
        .clone()
        .ok_or_else(|| "no active native group is bound".to_string())
}

/// Bind the active workspace to a native named-group id. Called by the
/// community create cutover after `x0x_create_group`.
#[tauri::command]
pub async fn x0x_set_active_group_id(
    group_id: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let trimmed = group_id.trim();
    if trimmed.is_empty() {
        return Err("group_id must be non-empty".to_string());
    }
    let mut guard = state.active_group_id.lock().map_err(|e| e.to_string())?;
    *guard = Some(trimmed.to_string());
    Ok(())
}

// ── Reads ────────────────────────────────────────────────────────────────────

/// GET /groups — lightweight summaries of all groups this agent knows.
#[tauri::command]
pub async fn x0x_list_groups(state: State<'_, AppState>) -> Result<GroupsList, String> {
    let client = &state.x0x_client;
    #[derive(Deserialize)]
    struct RawGroupsList {
        #[serde(default)]
        groups: Vec<RawNamedGroupSummary>,
    }
    let raw: RawGroupsList = client.get_json("/groups", &[]).await?;
    Ok(GroupsList {
        groups: raw.groups.into_iter().map(Into::into).collect(),
    })
}

/// GET /groups/:id — full named-group detail.
#[tauri::command]
pub async fn x0x_get_group(
    group_id: String,
    state: State<'_, AppState>,
) -> Result<NamedGroup, String> {
    fetch_full_group(&state.x0x_client, &group_id).await
}

/// GET /groups/:id/members — the roster as token-authenticated loopback
/// delegation (ADR-0001: display data, not a verified secure-group guarantee).
#[tauri::command]
pub async fn x0x_get_group_members(
    group_id: String,
    state: State<'_, AppState>,
) -> Result<MembersList, String> {
    let path = format!("/groups/{group_id}/members");
    #[derive(Deserialize)]
    struct RawMembersList {
        #[serde(default)]
        members: Vec<RawGroupMember>,
    }
    let raw: RawMembersList = state.x0x_client.get_json(&path, &[]).await?;
    Ok(MembersList {
        members: raw.members.into_iter().map(Into::into).collect(),
    })
}

// ── Create ───────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CreateGroupRequest {
    name: String,
    #[serde(default)]
    description: String,
    display_name: Option<String>,
    preset: Option<String>,
}

/// POST /groups — create a named group. Creator is auto-added owner by the
/// daemon. Returns the full record (fetched via GET after the thin ack).
#[tauri::command]
pub async fn x0x_create_group(
    input: CreateGroupRequest,
    state: State<'_, AppState>,
) -> Result<NamedGroup, String> {
    // Only `public_open` groups are creatable: secure-group (MLS / GSS /
    // TreeKEM) crypto is NOT approved, so any other preset is refused at the
    // boundary with a visible error rather than reaching the daemon. A missing
    // preset defaults to `public_open` (the only creatable kind).
    let preset = match input.preset.as_deref() {
        None | Some("public_open") => "public_open",
        Some(other) => {
            return Err(format!(
                "Cannot create group with preset '{other}': only public_open groups are available (secure-group crypto is not approved)."
            ));
        }
    };
    let client = &state.x0x_client;
    let body = CreateGroupBody {
        name: &input.name,
        description: &input.description,
        display_name: input.display_name,
        preset: Some(preset.to_string()),
    };
    let holder: GroupIdHolder = client.post_json("/groups", &body).await?;
    let group_id = holder
        .group_id
        .ok_or_else(|| "daemon create response missing group_id".to_string())?;
    fetch_full_group(client, &group_id).await
}

// ── Membership mutations ─────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AddMemberRequest {
    group_id: String,
    agent_id: String,
    display_name: Option<String>,
}

/// POST /groups/:id/members — add an agent (admin-or-higher). The daemon adds
/// the agent to the roster; no TreeKEM/secure-group key material is forwarded
/// (secure-group crypto is not approved — the boundary carries no key package).
#[tauri::command]
pub async fn x0x_add_group_member(
    input: AddMemberRequest,
    state: State<'_, AppState>,
) -> Result<GroupMember, String> {
    let group = path_segment(&input.group_id)?;
    let agent_id = validate_agent_id(&input.agent_id)?;
    let path = format!("/groups/{group}/members");
    let body = AddMemberBody {
        agent_id: &agent_id,
        display_name: input.display_name.as_deref(),
    };
    let raw: RawMembershipMutationResponse = state.x0x_client.post_json(&path, &body).await?;
    mutation_member(raw, &agent_id)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SetRoleRequest {
    group_id: String,
    agent_id: String,
    role: String,
}

/// PATCH /groups/:id/members/:agent_id/role — change role (admin-or-higher).
/// The daemon rejects owner assignment; assignable roles are admin/member.
#[tauri::command]
pub async fn x0x_set_group_member_role(
    input: SetRoleRequest,
    state: State<'_, AppState>,
) -> Result<GroupMember, String> {
    let (group, agent) = encode_member_path_segments(&input.group_id, &input.agent_id)?;
    let path = format!("/groups/{group}/members/{agent}/role");
    let body = SetRoleBody { role: &input.role };
    let raw: RawGroupMember = state.x0x_client.patch_json(&path, &body).await?;
    Ok(raw.into())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MemberTarget {
    group_id: String,
    agent_id: String,
}

/// DELETE /groups/:id/members/:agent_id — remove a member (admin) or self-leave.
#[tauri::command]
pub async fn x0x_remove_group_member(
    input: MemberTarget,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let (group, agent) = encode_member_path_segments(&input.group_id, &input.agent_id)?;
    let path = format!("/groups/{group}/members/{agent}");
    state.x0x_client.delete(&path).await?;
    Ok(())
}

/// POST /groups/:id/ban/:agent_id — ban an agent. The daemon advances the
/// group epoch server-side; this command only forwards the REST call and
/// asserts no local crypto / TreeKEM guarantee.
#[tauri::command]
pub async fn x0x_ban_group_member(
    input: MemberTarget,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let (group, agent) = encode_member_path_segments(&input.group_id, &input.agent_id)?;
    let path = format!("/groups/{group}/ban/{agent}");
    let _: serde_json::Value = state
        .x0x_client
        .post_json(&path, &serde_json::json!({}))
        .await?;
    Ok(())
}

/// DELETE /groups/:id/ban/:agent_id — lift a ban.
#[tauri::command]
pub async fn x0x_unban_group_member(
    input: MemberTarget,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let (group, agent) = encode_member_path_segments(&input.group_id, &input.agent_id)?;
    let path = format!("/groups/{group}/ban/{agent}");
    state.x0x_client.delete(&path).await?;
    Ok(())
}

/// DELETE /groups/:id — leave the calling agent's membership in a named group.
#[tauri::command]
pub async fn x0x_leave_group(group_id: String, state: State<'_, AppState>) -> Result<(), String> {
    let path = format!("/groups/{group_id}");
    state.x0x_client.delete(&path).await?;
    Ok(())
}

// ── Metadata (rename / redescribe) ───────────────────────────────────────────

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UpdateGroupRequest {
    group_id: String,
    name: Option<String>,
    description: Option<String>,
}

/// PATCH /groups/:id — rename / redescribe a named group.
#[tauri::command]
pub async fn x0x_update_group(
    input: UpdateGroupRequest,
    state: State<'_, AppState>,
) -> Result<NamedGroup, String> {
    let path = format!("/groups/{}", input.group_id);
    let body = UpdateGroupBody {
        name: input.name,
        description: input.description,
    };
    let raw: RawNamedGroup = state.x0x_client.patch_json(&path, &body).await?;
    Ok(raw.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn group_member_raw_to_camel_case() {
        let raw = RawGroupMember {
            agent_id: "a".repeat(64),
            user_id: None,
            role: "admin".to_string(),
            state: "active".to_string(),
            display_name: Some("Ada".to_string()),
            joined_at_ms: 1_700_000_000_000,
            updated_at_ms: Some(1_700_000_000_000),
            added_by: Some("b".repeat(64)),
            removed_by: None,
        };
        let member: GroupMember = raw.into();
        let json = serde_json::to_value(&member).unwrap();
        // camelCase keys the TS seam declares.
        assert_eq!(json["agentId"], "a".repeat(64));
        assert_eq!(json["role"], "admin");
        assert_eq!(json["state"], "active");
        assert_eq!(json["displayName"], "Ada");
        assert_eq!(json["joinedAtMs"], 1_700_000_000_000_i64);
        assert_eq!(json["addedBy"], "b".repeat(64));
        // Optional identity linkage is omitted; roster audit fields preserve
        // explicit null because the TS contract requires them.
        assert!(json["removedBy"].is_null());
        assert!(json.get("userId").is_none());
    }

    #[test]
    fn named_group_serializes_camel_case_and_tolerates_missing_fields() {
        // Daemon GET /groups/:id may omit inline roster + optional fields.
        let daemon_json = serde_json::json!({
            "group_id": "deadbeef",
            "name": "Engineering",
            "chat_topic": "x0x.group.deadbeef.chat",
            "metadata_topic": "x0x.group.deadbeef.meta",
            "policy_revision": 3,
            "roster_revision": 7,
            "member_count": 4
        });
        let raw: RawNamedGroup = serde_json::from_value(daemon_json).unwrap();
        let group: NamedGroup = raw.into();
        let json = serde_json::to_value(&group).unwrap();
        assert_eq!(json["groupId"], "deadbeef");
        assert_eq!(json["chatTopic"], "x0x.group.deadbeef.chat");
        assert_eq!(json["metadataTopic"], "x0x.group.deadbeef.meta");
        assert_eq!(json["policyRevision"], 3);
        assert_eq!(json["rosterRevision"], 7);
        assert_eq!(json["memberCount"], 4);
        // Defaults for absent fields.
        assert_eq!(json["description"], "");
        assert_eq!(json["members"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn create_response_holder_extracts_group_id_from_full_or_thin() {
        // Thin ack.
        let thin = serde_json::json!({ "ok": true, "group_id": "g1", "name": "X" });
        let h: GroupIdHolder = serde_json::from_value(thin).unwrap();
        assert_eq!(h.group_id.as_deref(), Some("g1"));

        // Full group object also carries group_id.
        let full = serde_json::json!({
            "group_id": "g2",
            "name": "Y",
            "chat_topic": "t",
            "members": []
        });
        let h: GroupIdHolder = serde_json::from_value(full).unwrap();
        assert_eq!(h.group_id.as_deref(), Some("g2"));
    }

    #[test]
    fn raw_group_member_reads_daemon_joined_at_and_falls_back_for_updated() {
        // Daemon member JSON uses `joined_at` and omits `updated_at`.
        let daemon = serde_json::json!({
            "agent_id": "a".repeat(64),
            "role": "member",
            "state": "active",
            "display_name": "Ada",
            "joined_at": 1_700_000_000_000_i64,
            "added_by": "b".repeat(64),
        });
        let member: GroupMember = serde_json::from_value::<RawGroupMember>(daemon)
            .unwrap()
            .into();
        let json = serde_json::to_value(&member).unwrap();
        assert_eq!(json["joinedAtMs"], 1_700_000_000_000_i64);
        // updated_at absent → falls back to joined_at, never stale zero.
        assert_eq!(json["updatedAtMs"], 1_700_000_000_000_i64);
    }

    #[test]
    fn add_member_response_selects_the_target_from_the_roster_wrapper() {
        let target = "b".repeat(64);
        let daemon = serde_json::json!({
            "ok": true,
            "group_id": "g1",
            "member_count": 2,
            "members": [
                {
                    "agent_id": "a".repeat(64),
                    "role": "admin",
                    "state": "active",
                    "joined_at": 1_i64
                },
                {
                    "agent_id": target,
                    "role": "member",
                    "state": "active",
                    "joined_at": 2_i64
                }
            ]
        });
        let raw: RawMembershipMutationResponse = serde_json::from_value(daemon).unwrap();
        let member = mutation_member(raw, &"b".repeat(64)).unwrap();
        assert_eq!(member.agent_id, "b".repeat(64));
        assert_eq!(member.role, "member");
        assert_eq!(member.joined_at_ms, 2);
    }

    #[test]
    fn raw_group_member_honors_explicit_daemon_updated_at() {
        let daemon = serde_json::json!({
            "agent_id": "a".repeat(64),
            "role": "member",
            "state": "active",
            "joined_at": 100_i64,
            "updated_at": 200_i64,
        });
        let member: GroupMember = serde_json::from_value::<RawGroupMember>(daemon)
            .unwrap()
            .into();
        let json = serde_json::to_value(&member).unwrap();
        assert_eq!(json["joinedAtMs"], 100_i64);
        assert_eq!(json["updatedAtMs"], 200_i64);
    }

    #[test]
    fn member_path_segments_validate_agent_and_encode_group() {
        // Valid 64-hex agent passes; opaque group id with route delimiters is encoded.
        let (group, agent) =
            encode_member_path_segments("group:opaque/7", &"ab".repeat(32)).unwrap();
        assert_eq!(group, "group%3Aopaque%2F7");
        assert_eq!(agent, "ab".repeat(32));

        // npub / too-short / slash-bearing agents are rejected pre-request.
        assert!(encode_member_path_segments("g", "npub1deadbeef").is_err());
        assert!(encode_member_path_segments("g", "deadbeef").is_err());
        let slash_agent = format!("{}{}{}", "a".repeat(31), "/", "b".repeat(32));
        assert_eq!(slash_agent.len(), 64);
        assert!(encode_member_path_segments("g", &slash_agent).is_err());
        // Empty group id is rejected.
        assert!(encode_member_path_segments("", &"ab".repeat(32)).is_err());
    }
    #[test]
    fn named_group_projection_preserves_signed_public_and_never_launders() {
        // `confidentiality` is the security-critical label a consumer branches
        // on. It MUST preserve an explicit `signed_public` (the only creatable
        // kind) and NEVER relabel an `mls_encrypted` or unresolved group as
        // `signed_public` — that would launder a non-public group onto the
        // open-channel path. An absent/null policy resolves to `None`, never a
        // fabricated public label.
        let signed = serde_json::json!({
            "group_id": "g1", "name": "Eng",
            "policy": { "confidentiality": "signed_public" }
        });
        let group: NamedGroup = serde_json::from_value::<RawNamedGroup>(signed)
            .unwrap()
            .into();
        assert_eq!(
            group.confidentiality.as_deref(),
            Some("signed_public"),
            "signed_public must be preserved verbatim"
        );

        let mls = serde_json::json!({
            "group_id": "g2", "name": "Secret",
            "policy": { "confidentiality": "mls_encrypted" }
        });
        let group: NamedGroup = serde_json::from_value::<RawNamedGroup>(mls).unwrap().into();
        // Never laundered to public: keeps its true label, never signed_public.
        assert_ne!(
            group.confidentiality.as_deref(),
            Some("signed_public"),
            "mls_encrypted must never be relabeled as signed_public"
        );
        assert_eq!(group.confidentiality.as_deref(), Some("mls_encrypted"));

        // Null confidentiality (policy present, axis unset) → None, not public.
        let null_axis = serde_json::json!({
            "group_id": "g3", "name": "Mystery",
            "policy": { "confidentiality": null }
        });
        let group: NamedGroup = serde_json::from_value::<RawNamedGroup>(null_axis)
            .unwrap()
            .into();
        assert_eq!(group.confidentiality, None);

        // No policy at all → None, never fabricated as public.
        let no_policy = serde_json::json!({ "group_id": "g4", "name": "Bare" });
        let group: NamedGroup = serde_json::from_value::<RawNamedGroup>(no_policy)
            .unwrap()
            .into();
        assert_eq!(group.confidentiality, None);
    }

    #[test]
    fn add_member_body_carries_no_treekem_or_key_package_field() {
        // The add-member wire body forwards only the roster addition;
        // secure-group (TreeKEM / MLS) crypto is NOT approved, so NO key-package
        // material may cross this boundary. Re-adding a `treekem_key_package_b64`
        // (or any key-package-shaped field) here would re-open the secure-group
        // ingress; this pins the body to exactly `{ agent_id, display_name? }`.
        let agent = "ab".repeat(32);
        let with_name = AddMemberBody {
            agent_id: &agent,
            display_name: Some("Ada"),
        };
        let v: serde_json::Value =
            serde_json::to_value(&with_name).expect("serialize add-member body");
        assert_eq!(v["agent_id"], agent);
        assert_eq!(v["display_name"], "Ada");
        assert!(
            v.get("treekem_key_package_b64").is_none(),
            "treekem key package must not cross the Tauri boundary: {v}"
        );
        let keys: Vec<&str> = v
            .as_object()
            .expect("add-member body is an object")
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            keys,
            vec!["agent_id", "display_name"],
            "exact add-member body keys: {v}"
        );

        // An omitted display_name is dropped; still no key material leaks.
        let no_name = AddMemberBody {
            agent_id: &agent,
            display_name: None,
        };
        let v: serde_json::Value =
            serde_json::to_value(&no_name).expect("serialize add-member body without name");
        assert!(v.get("display_name").is_none());
        assert!(v.get("treekem_key_package_b64").is_none());
        let keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(
            keys,
            vec!["agent_id"],
            "no display_name, no key material: {v}"
        );
    }
}
