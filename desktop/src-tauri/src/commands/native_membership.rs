//! Native named-group membership command surface (M3 workspace cutover).
//!
//! Typed Tauri commands over the authenticated loopback [`X0xClient`] that
//! proxy the embedded `x0xd` daemon's named-group REST surface (`/groups`,
//! `/groups/:id`, `/groups/:id/members`, `/groups/:id/ban/:agent_id`,
//! `/groups/:id/invite`, `/groups/:id/policy`, `/groups/:id/requests`).
//!
//! These are the native transport path the M3 membership cutover binds to.
//! They emit **no** relay events and publish **no** Nostr kinds, and they
//! perform **no** authority reconstruction (ADR-0001): roster/policy state is
//! accepted only as the daemon's authenticated frontier and re-serialized for
//! the frontend. Every call resolves the transient daemon token/port inside
//! [`X0xClient`](crate::x0x_client::X0xClient) and returns a human-readable
//! `String` error (never the token) on failure.
//!
//! # serde convention
//! Daemon responses are snake_case; the frozen TS seam
//! (`tauriNativeX0x.ts`) exposes camelCase types to features. Raw structs
//! deserialize the daemon verbatim; output structs carry
//! `#[serde(rename_all = "camelCase")]`; a trivial `From` bridges them. The
//! two wrappers where the TS seam does its own key conversion
//! (`x0x_list_groups`, `x0x_get_group_members`, `x0x_list_group_join_requests`,
//! `x0x_mint_group_invite`) return the exact raw shape the seam unpacks.

use serde::{Deserialize, Serialize};
use tauri::State;

use crate::app_state::AppState;
use crate::x0x_client::X0xClient;

#[path = "native_social.rs"]
mod native_social;
pub use native_social::*;

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
/// fetches the authoritative frontier via `x0x_get_group_members`).
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

/// A request-to-join — mirrors `X0xJoinRequest`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct JoinRequest {
    pub request_id: String,
    pub group_id: String,
    pub requester_agent_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requester_user_id: Option<String>,
    pub requested_role: String,
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub treekem_key_package_b64: Option<String>,
    pub created_at_ms: i64,
    pub reviewed_at_ms: Option<i64>,
    pub reviewed_by: Option<String>,
    pub status: String,
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
    #[serde(default)]
    joined_at_ms: i64,
    #[serde(default)]
    updated_at_ms: i64,
    #[serde(default)]
    added_by: Option<String>,
    #[serde(default)]
    removed_by: Option<String>,
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

#[derive(Debug, Clone, Deserialize)]
struct RawJoinRequest {
    request_id: String,
    group_id: String,
    requester_agent_id: String,
    #[serde(default)]
    requester_user_id: Option<String>,
    #[serde(default)]
    requested_role: String,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    treekem_key_package_b64: Option<String>,
    #[serde(default)]
    created_at_ms: i64,
    #[serde(default)]
    reviewed_at_ms: Option<i64>,
    #[serde(default)]
    reviewed_by: Option<String>,
    #[serde(default)]
    status: String,
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
            updated_at_ms: r.updated_at_ms,
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

impl From<RawJoinRequest> for JoinRequest {
    fn from(r: RawJoinRequest) -> Self {
        JoinRequest {
            request_id: r.request_id,
            group_id: r.group_id,
            requester_agent_id: r.requester_agent_id,
            requester_user_id: r.requester_user_id,
            requested_role: r.requested_role,
            message: r.message,
            treekem_key_package_b64: r.treekem_key_package_b64,
            created_at_ms: r.created_at_ms,
            reviewed_at_ms: r.reviewed_at_ms,
            reviewed_by: r.reviewed_by,
            status: r.status,
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

#[derive(Debug, Serialize)]
pub struct JoinRequestsList {
    pub requests: Vec<JoinRequest>,
}

/// POST /groups / POST /groups/join response holder. Both surfaces return at
/// least `group_id`; a full group object also carries it, so this extracts the
/// id whether the daemon returns a thin ack or the full record. We then fetch
/// the authoritative full group via GET /groups/:id so create/join return the
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
struct JoinGroupBody<'a> {
    invite: &'a str,
    display_name: Option<String>,
}

#[derive(Debug, Serialize)]
struct AddMemberBody<'a> {
    agent_id: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    display_name: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    treekem_key_package_b64: Option<&'a str>,
}

#[derive(Debug, Serialize)]
struct SetRoleBody<'a> {
    role: &'a str,
}

#[derive(Debug, Serialize)]
struct MintInviteBody {
    /// `None` ⇒ daemon default (7 days); `Some(0)` ⇒ never.
    expiry_secs: Option<i64>,
}

#[derive(Debug, Serialize)]
struct RequestJoinBody<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    treekem_key_package_b64: Option<&'a str>,
}

/// Mutable policy axes. All `Option`; omitted axes are unchanged daemon-side.
#[derive(Debug, Default, Serialize)]
struct PolicyUpdateBody {
    #[serde(skip_serializing_if = "Option::is_none")]
    preset: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    discoverability: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    admission: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    confidentiality: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    read_access: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    write_access: Option<String>,
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
/// community create/join cutover after `x0x_create_group` / `x0x_join_group`.
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

/// GET /groups/:id/members — the authenticated roster frontier (ADR-0001).
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

// ── Create / join ────────────────────────────────────────────────────────────

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
    let client = &state.x0x_client;
    let body = CreateGroupBody {
        name: &input.name,
        description: &input.description,
        display_name: input.display_name,
        preset: input.preset,
    };
    let holder: GroupIdHolder = client.post_json("/groups", &body).await?;
    let group_id = holder
        .group_id
        .ok_or_else(|| "daemon create response missing group_id".to_string())?;
    fetch_full_group(client, &group_id).await
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct JoinGroupRequest {
    invite: String,
    display_name: Option<String>,
}

/// POST /groups/join — join a named group via a one-time invite link. The
/// joiner self-adds; the inviter publishes the authoritative MemberAdded
/// commit. Returns the full record (fetched via GET after the ack).
#[tauri::command]
pub async fn x0x_join_group(
    input: JoinGroupRequest,
    state: State<'_, AppState>,
) -> Result<NamedGroup, String> {
    let client = &state.x0x_client;
    let body = JoinGroupBody {
        invite: &input.invite,
        display_name: input.display_name,
    };
    let holder: GroupIdHolder = client.post_json("/groups/join", &body).await?;
    let group_id = holder
        .group_id
        .ok_or_else(|| "daemon join response missing group_id".to_string())?;
    fetch_full_group(client, &group_id).await
}

// ── Membership mutations ─────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AddMemberRequest {
    group_id: String,
    agent_id: String,
    display_name: Option<String>,
    treekem_key_package_b64: Option<String>,
}

/// POST /groups/:id/members — add an agent (admin-or-higher). TreeKEM groups
/// require `treekem_key_package_b64` (the UI surfaces the daemon's rejection).
#[tauri::command]
pub async fn x0x_add_group_member(
    input: AddMemberRequest,
    state: State<'_, AppState>,
) -> Result<GroupMember, String> {
    let path = format!("/groups/{}/members", input.group_id);
    let body = AddMemberBody {
        agent_id: &input.agent_id,
        display_name: input.display_name.as_deref(),
        treekem_key_package_b64: input.treekem_key_package_b64.as_deref(),
    };
    let raw: RawGroupMember = state.x0x_client.post_json(&path, &body).await?;
    Ok(raw.into())
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
    let path = format!("/groups/{}/members/{}/role", input.group_id, input.agent_id);
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
    let path = format!("/groups/{}/members/{}", input.group_id, input.agent_id);
    state.x0x_client.delete(&path).await?;
    Ok(())
}

/// POST /groups/:id/ban/:agent_id — ban an agent (rekeys survivors; the crypto
/// frontier rotates the shared secret / advances epoch — the REST call exists).
#[tauri::command]
pub async fn x0x_ban_group_member(
    input: MemberTarget,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let path = format!("/groups/{}/ban/{}", input.group_id, input.agent_id);
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
    let path = format!("/groups/{}/ban/{}", input.group_id, input.agent_id);
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

// ── Invites ──────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MintInviteRequest {
    group_id: String,
    expiry_secs: Option<i64>,
}

/// Raw mint-invite response — returned verbatim (snake_case) because the TS
/// seam performs the camelCase conversion itself.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct MintInviteResponse {
    invite_link: String,
    group_id: String,
    group_name: String,
    #[serde(default)]
    expires_at: i64,
}

/// POST /groups/:id/invite — mint a one-time invite (admin-or-higher).
///
/// NOTE: the daemon exposes NO list-invite and NO revoke-invite endpoint.
/// Invites are one-time secrets that age out via `expires_at`; for an
/// already-joined member, ban is the only revocation path.
#[tauri::command]
pub async fn x0x_mint_group_invite(
    input: MintInviteRequest,
    state: State<'_, AppState>,
) -> Result<MintInviteResponse, String> {
    let path = format!("/groups/{}/invite", input.group_id);
    let body = MintInviteBody {
        expiry_secs: input.expiry_secs,
    };
    let raw: MintInviteResponse = state.x0x_client.post_json(&path, &body).await?;
    Ok(raw)
}

// ── Join requests (preset-gated groups) ──────────────────────────────────────

/// GET /groups/:id/requests — pending/approved/rejected/cancelled requests.
#[tauri::command]
pub async fn x0x_list_group_join_requests(
    group_id: String,
    state: State<'_, AppState>,
) -> Result<JoinRequestsList, String> {
    let path = format!("/groups/{group_id}/requests");
    #[derive(Deserialize)]
    struct RawJoinRequestsList {
        #[serde(default)]
        requests: Vec<RawJoinRequest>,
    }
    let raw: RawJoinRequestsList = state.x0x_client.get_json(&path, &[]).await?;
    Ok(JoinRequestsList {
        requests: raw.requests.into_iter().map(Into::into).collect(),
    })
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RequestJoinInput {
    group_id: String,
    message: Option<String>,
    treekem_key_package_b64: Option<String>,
}

/// POST /groups/:id/requests — submit a request-to-join (requires non-member,
/// non-banned; used by `public_request_secure` admission).
#[tauri::command]
pub async fn x0x_request_group_join(
    input: RequestJoinInput,
    state: State<'_, AppState>,
) -> Result<JoinRequest, String> {
    let path = format!("/groups/{}/requests", input.group_id);
    let body = RequestJoinBody {
        message: input.message.as_deref(),
        treekem_key_package_b64: input.treekem_key_package_b64.as_deref(),
    };
    let raw: RawJoinRequest = state.x0x_client.post_json(&path, &body).await?;
    Ok(raw.into())
}

// ── Policy / metadata ────────────────────────────────────────────────────────

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PolicyUpdateRequest {
    group_id: String,
    preset: Option<String>,
    discoverability: Option<String>,
    admission: Option<String>,
    confidentiality: Option<String>,
    read_access: Option<String>,
    write_access: Option<String>,
}

/// PATCH /groups/:id/policy — mutate one or more policy axes. Any subset may
/// be supplied; omitted fields are unchanged. `preset` is the convenience name.
#[tauri::command]
pub async fn x0x_update_group_policy(
    input: PolicyUpdateRequest,
    state: State<'_, AppState>,
) -> Result<NamedGroup, String> {
    let path = format!("/groups/{}/policy", input.group_id);
    let body = PolicyUpdateBody {
        preset: input.preset,
        discoverability: input.discoverability,
        admission: input.admission,
        confidentiality: input.confidentiality,
        read_access: input.read_access,
        write_access: input.write_access,
    };
    let raw: RawNamedGroup = state.x0x_client.patch_json(&path, &body).await?;
    Ok(raw.into())
}

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
            updated_at_ms: 1_700_000_000_000,
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
    fn mint_invite_response_round_trips_snake_case() {
        // Daemon sends {ok, invite_link, group_id, group_name, expires_at};
        // serde ignores the extra `ok` and the command returns the raw shape
        // the TS seam converts.
        let daemon = serde_json::json!({
            "ok": true,
            "invite_link": "x0x://invite/abc",
            "group_id": "g1",
            "group_name": "Eng",
            "expires_at": 0
        });
        let resp: MintInviteResponse = serde_json::from_value(daemon).unwrap();
        assert_eq!(resp.invite_link, "x0x://invite/abc");
        assert_eq!(resp.group_id, "g1");
        assert_eq!(resp.group_name, "Eng");
        assert_eq!(resp.expires_at, 0);
    }

    #[test]
    fn join_request_raw_to_camel_case() {
        let raw = RawJoinRequest {
            request_id: "r1".to_string(),
            group_id: "g1".to_string(),
            requester_agent_id: "a".repeat(64),
            requester_user_id: None,
            requested_role: "member".to_string(),
            message: Some("let me in".to_string()),
            treekem_key_package_b64: None,
            created_at_ms: 1,
            reviewed_at_ms: None,
            reviewed_by: None,
            status: "Pending".to_string(),
        };
        let req: JoinRequest = raw.into();
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["requestId"], "r1");
        assert_eq!(json["requesterAgentId"], "a".repeat(64));
        assert_eq!(json["requestedRole"], "member");
        assert_eq!(json["status"], "Pending");
        assert_eq!(json["createdAtMs"], 1);
    }

    #[test]
    fn policy_update_body_omits_absent_axes() {
        let body = PolicyUpdateBody {
            preset: Some("public_open".to_string()),
            discoverability: None,
            admission: None,
            confidentiality: None,
            read_access: None,
            write_access: None,
        };
        let json = serde_json::to_value(&body).unwrap();
        assert_eq!(json["preset"], "public_open");
        // Absent axes are skipped so the daemon leaves them unchanged.
        for key in [
            "discoverability",
            "admission",
            "confidentiality",
            "read_access",
            "write_access",
        ] {
            assert!(json.get(key).is_none(), "{key} should be absent");
        }
    }
}
