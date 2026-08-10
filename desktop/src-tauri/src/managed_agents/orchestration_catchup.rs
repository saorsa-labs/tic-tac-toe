//! Authenticated child-only convergence for managed signed-public communities.

use super::*;

const STATE_COMMITS_APPLY_MAX: u64 = 500;

#[derive(Debug, Clone)]
pub(super) struct ValidatedOwnerCatchup {
    commits: Vec<Value>,
    child_revision: u64,
}

#[derive(Debug)]
struct ChildCatchupHead {
    state: GroupStateProjection,
    roster_revision: u64,
}

pub(super) async fn catch_up_child_from_owner_history(
    owner_client: &X0xClient,
    loopback_http: &reqwest::Client,
    child: &ManagedAgentChild,
    owner_expectation: &OwnerGroupExpectation<'_>,
    child_expectation: &ChildGroupExpectation<'_>,
) -> Result<(), String> {
    let child_snapshot = read_child_catchup_head(
        loopback_http,
        child,
        child_expectation,
        owner_expectation.roster_revision,
    )
    .await?;
    let child_head = &child_snapshot.state;
    let owner_head = child_expectation.owner_state;

    if !catchup_projection_compatible(child_head, owner_head) {
        return Err(format!(
            "managed-agent community {} has an incompatible child projection; retained catch-up is unsafe",
            child_expectation.group_id
        ));
    }
    if child_head.state_revision == owner_head.state_revision
        && child_head.state_hash == owner_head.state_hash
    {
        require_child_strict(
            loopback_http,
            child,
            child_expectation,
            owner_expectation.roster_revision,
        )
        .await?;
        require_owner_unchanged(owner_client, owner_expectation, owner_head).await?;
        return Ok(());
    }
    if child_head.state_revision >= owner_head.state_revision {
        return Err(format!(
            "managed-agent community {} child head is not an ancestor of the owner head",
            child_expectation.group_id
        ));
    }

    let next_revision = child_head.state_revision.checked_add(1).ok_or_else(|| {
        "managed-agent community catch-up cannot advance an exhausted revision".to_string()
    })?;
    let history_path = format!("{}/state/commits", child_expectation.group_path);
    let history: Value = owner_client
        .get_json(
            &history_path,
            &[
                ("from_revision".to_string(), next_revision.to_string()),
                ("limit".to_string(), STATE_COMMITS_APPLY_MAX.to_string()),
            ],
        )
        .await
        .map_err(|error| format!("owner retained community history lookup failed: {error}"))?;
    let batch = validate_owner_catchup_history(&history, child_head, owner_head)?;

    require_owner_unchanged(owner_client, owner_expectation, owner_head).await?;
    apply_validated_child_catchup(
        loopback_http,
        child,
        child_expectation,
        owner_expectation.roster_revision,
        &batch,
    )
    .await?;
    require_owner_unchanged(owner_client, owner_expectation, owner_head).await
}

fn catchup_projection_compatible(
    child: &GroupStateProjection,
    owner: &GroupStateProjection,
) -> bool {
    child.group_id == owner.group_id
        && child.genesis == owner.genesis
        && child.policy_hash == owner.policy_hash
        && child.public_meta_hash == owner.public_meta_hash
        && child.security_binding == owner.security_binding
        && child.withdrawn == owner.withdrawn
}

async fn read_child_catchup_head(
    loopback_http: &reqwest::Client,
    child: &ManagedAgentChild,
    expectation: &ChildGroupExpectation<'_>,
    owner_roster_revision: u64,
) -> Result<ChildCatchupHead, String> {
    let state_path = format!("{}/state", expectation.group_path);
    let before = read_child_group_state(loopback_http, child, &state_path, expectation.group_id)
        .await?
        .ok_or_else(|| "managed-agent community catch-up child state is missing".to_string())?;
    let group = child_request_json(
        loopback_http,
        &child.data_dir,
        Method::GET,
        expectation.group_path,
        None,
    )
    .await
    .map_err(|error| format!("managed-agent community catch-up lookup failed: {error}"))?;
    require_signed_public_group(&group, expectation.group_id)?;
    let child_state = group_member_state(&group, &child.agent_id);
    reject_banned_member(
        child_state,
        &child.agent_id,
        "managed-agent child community",
    )?;
    if child_state != GroupMemberState::Active {
        return Err(format!(
            "managed-agent child {} is not active in its local community snapshot",
            child.agent_id
        ));
    }
    if expectation
        .forbidden_agent_id
        .is_some_and(|id| group_member_state(&group, id) == GroupMemberState::Active)
    {
        return Err("managed-agent child retains forbidden active membership".to_string());
    }
    let child_roster_revision = roster_revision(&group)?;
    if child_roster_revision > owner_roster_revision {
        return Err("managed-agent child roster is ahead of the owner roster".to_string());
    }
    let after = read_child_group_state(loopback_http, child, &state_path, expectation.group_id)
        .await?
        .ok_or_else(|| "managed-agent community catch-up child state disappeared".to_string())?;
    if before != after {
        return Err("managed-agent child community changed during catch-up inspection".to_string());
    }
    if after.withdrawn {
        return Err("managed-agent child community is withdrawn".to_string());
    }
    Ok(ChildCatchupHead {
        state: after,
        roster_revision: child_roster_revision,
    })
}

pub(super) fn validate_owner_catchup_history(
    history: &Value,
    child: &GroupStateProjection,
    owner: &GroupStateProjection,
) -> Result<ValidatedOwnerCatchup, String> {
    if history.get("group_id").and_then(Value::as_str) != Some(owner.group_id.as_str())
        || history.get("state_revision").and_then(Value::as_u64) != Some(owner.state_revision)
        || history.get("withdrawn").and_then(Value::as_bool) != Some(false)
    {
        return Err("owner retained community history does not match its state head".to_string());
    }
    let next_revision = child.state_revision.checked_add(1).ok_or_else(|| {
        "managed-agent community catch-up cannot advance an exhausted revision".to_string()
    })?;
    let expected_count = owner
        .state_revision
        .checked_sub(child.state_revision)
        .filter(|count| *count > 0)
        .ok_or_else(|| "child state is not behind the owner state".to_string())?;
    if expected_count > STATE_COMMITS_APPLY_MAX {
        return Err("owner retained community history exceeds one safe catch-up batch".to_string());
    }
    let first_retained_revision = history
        .get("first_available_revision")
        .and_then(Value::as_u64)
        .ok_or_else(|| "owner retained community history has no lower bound".to_string())?;
    if history.get("from_revision").and_then(Value::as_u64) != Some(next_revision)
        || first_retained_revision > next_revision
        || history
            .get("latest_retained_revision")
            .and_then(Value::as_u64)
            != Some(owner.state_revision)
        || history.get("count").and_then(Value::as_u64) != Some(expected_count)
        || history.get("has_more").and_then(Value::as_bool) != Some(false)
    {
        return Err("owner retained community history is incomplete or non-contiguous".to_string());
    }
    let commits = history
        .get("commits")
        .and_then(Value::as_array)
        .filter(|entries| u64::try_from(entries.len()).ok() == Some(expected_count))
        .ok_or_else(|| {
            "owner retained community history has an unexpected batch size".to_string()
        })?;

    let mut expected_revision = next_revision;
    let mut predecessor = child.state_hash.as_str();
    for entry in commits {
        let commit = entry
            .get("commit")
            .filter(|value| value.is_object())
            .ok_or_else(|| "owner retained community history is missing a commit".to_string())?;
        if entry.get("roster").is_none_or(|value| !value.is_object())
            || entry.get("roster_root_verified").and_then(Value::as_bool) != Some(true)
            || commit.get("group_id").and_then(Value::as_str) != Some(owner.group_id.as_str())
            || commit.get("revision").and_then(Value::as_u64) != Some(expected_revision)
            || commit.get("prev_state_hash").and_then(Value::as_str) != Some(predecessor)
            || commit.get("withdrawn").and_then(Value::as_bool) != Some(false)
        {
            return Err(
                "owner retained community history contains a gap, fork, or invalid projection"
                    .to_string(),
            );
        }
        predecessor = commit
            .get("state_hash")
            .and_then(Value::as_str)
            .filter(|hash| !hash.is_empty())
            .ok_or_else(|| "owner retained community commit is missing state_hash".to_string())?;
        expected_revision = expected_revision
            .checked_add(1)
            .ok_or_else(|| "managed-agent community catch-up revision overflowed".to_string())?;
    }
    let last = commits
        .last()
        .and_then(|entry| entry.get("commit"))
        .ok_or_else(|| "owner retained community history is empty".to_string())?;
    if last.get("state_hash").and_then(Value::as_str) != Some(owner.state_hash.as_str())
        || last.get("roster_root").and_then(Value::as_str) != Some(owner.roster_root.as_str())
        || last.get("policy_hash").and_then(Value::as_str) != Some(owner.policy_hash.as_str())
        || last.get("public_meta_hash").and_then(Value::as_str)
            != Some(owner.public_meta_hash.as_str())
        || last.get("security_binding") != Some(&owner.security_binding)
    {
        return Err(
            "owner retained community history does not terminate at its state head".to_string(),
        );
    }
    Ok(ValidatedOwnerCatchup {
        commits: commits.clone(),
        child_revision: child.state_revision,
    })
}

pub(super) async fn apply_validated_child_catchup(
    loopback_http: &reqwest::Client,
    child: &ManagedAgentChild,
    expectation: &ChildGroupExpectation<'_>,
    target_roster_revision: u64,
    batch: &ValidatedOwnerCatchup,
) -> Result<(), String> {
    let apply_path = format!("{}/state/commits/apply", expectation.group_path);
    let response = child_request_json(
        loopback_http,
        &child.data_dir,
        Method::POST,
        &apply_path,
        Some(&json!({
            "commits": batch.commits,
            "target_roster_revision": target_roster_revision,
        })),
    )
    .await
    .map_err(|error| format!("managed-agent retained community catch-up failed: {error}"))?;
    validate_child_apply_response(&response, expectation, target_roster_revision, batch)?;
    require_child_strict(loopback_http, child, expectation, target_roster_revision).await
}

fn validate_child_apply_response(
    response: &Value,
    expectation: &ChildGroupExpectation<'_>,
    target_roster_revision: u64,
    batch: &ValidatedOwnerCatchup,
) -> Result<(), String> {
    let expected_applied = u64::try_from(batch.commits.len())
        .map_err(|_| "managed-agent community catch-up batch length overflowed".to_string())?;
    let owner = expectation.owner_state;
    if response.get("ok").and_then(Value::as_bool) != Some(true)
        || response.get("group_id").and_then(Value::as_str) != Some(expectation.group_id)
        || response.get("applied").and_then(Value::as_u64) != Some(expected_applied)
        || response.get("from_revision").and_then(Value::as_u64) != Some(batch.child_revision)
        || response.get("state_revision").and_then(Value::as_u64) != Some(owner.state_revision)
        || response.get("roster_revision").and_then(Value::as_u64) != Some(target_roster_revision)
        || response.get("state_hash").and_then(Value::as_str) != Some(owner.state_hash.as_str())
        || response.get("roster_root").and_then(Value::as_str) != Some(owner.roster_root.as_str())
        || response.get("withdrawn").and_then(Value::as_bool) != Some(false)
    {
        return Err(
            "managed-agent retained community catch-up returned an inconsistent result".to_string(),
        );
    }
    Ok(())
}

async fn require_child_strict(
    loopback_http: &reqwest::Client,
    child: &ManagedAgentChild,
    expectation: &ChildGroupExpectation<'_>,
    target_roster_revision: u64,
) -> Result<(), String> {
    let observation = wait_for_child_group_observation(
        loopback_http,
        child,
        expectation,
        GROUP_CONVERGENCE_TIMEOUT,
        GROUP_CONVERGENCE_POLL,
    )
    .await?;
    if observation != ChildGroupObservation::Strict {
        return Err(format!(
            "managed-agent retained community catch-up did not reach the exact owner head ({observation:?})"
        ));
    }
    let confirmed =
        read_child_catchup_head(loopback_http, child, expectation, target_roster_revision).await?;
    if confirmed.roster_revision != target_roster_revision
        || confirmed.state != *expectation.owner_state
    {
        return Err(
            "managed-agent retained community catch-up did not preserve the exact owner frontier"
                .to_string(),
        );
    }
    Ok(())
}

async fn require_owner_unchanged(
    owner_client: &X0xClient,
    expectation: &OwnerGroupExpectation<'_>,
    expected: &GroupStateProjection,
) -> Result<(), String> {
    let current = read_owner_group_state(owner_client, expectation).await?;
    if current != *expected {
        return Err(
            "owner community changed during child catch-up; retry without owner mutation"
                .to_string(),
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    };

    use axum::{
        extract::State, http::StatusCode as AxumStatusCode, response::IntoResponse, routing::get,
        routing::post, Json, Router,
    };

    #[derive(Clone)]
    struct ChildApplyApi {
        current: Arc<tokio::sync::RwLock<(Value, Value)>>,
        after: Arc<(Value, Value)>,
        response: Arc<Value>,
        reject: Option<AxumStatusCode>,
        calls: Arc<AtomicUsize>,
        bodies: Arc<Mutex<Vec<Value>>>,
    }

    async fn child_group(State(api): State<ChildApplyApi>) -> Json<Value> {
        Json(api.current.read().await.0.clone())
    }

    async fn child_state(State(api): State<ChildApplyApi>) -> Json<Value> {
        Json(api.current.read().await.1.clone())
    }

    async fn apply_commits(
        State(api): State<ChildApplyApi>,
        Json(body): Json<Value>,
    ) -> impl IntoResponse {
        api.calls.fetch_add(1, Ordering::SeqCst);
        api.bodies.lock().expect("record apply body").push(body);
        if let Some(status) = api.reject {
            return (status, Json(json!({ "error": "rejected" }))).into_response();
        }
        *api.current.write().await = api.after.as_ref().clone();
        (AxumStatusCode::OK, Json(api.response.as_ref().clone())).into_response()
    }

    async fn spawn_child_apply_api(
        child_id: &str,
        before: (Value, Value),
        after: (Value, Value),
        response: Value,
        reject: Option<AxumStatusCode>,
    ) -> (
        tempfile::TempDir,
        ManagedAgentChild,
        ChildApplyApi,
        tokio::task::JoinHandle<()>,
    ) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind child apply API");
        let address = listener.local_addr().expect("child apply API address");
        let data_dir = tempfile::tempdir().expect("child apply data dir");
        std::fs::write(
            data_dir.path().join("api.port"),
            format!("127.0.0.1:{}", address.port()),
        )
        .expect("write child api.port");
        std::fs::write(data_dir.path().join("api-token"), "test-token")
            .expect("write child api-token");
        let api = ChildApplyApi {
            current: Arc::new(tokio::sync::RwLock::new(before)),
            after: Arc::new(after),
            response: Arc::new(response),
            reject,
            calls: Arc::new(AtomicUsize::new(0)),
            bodies: Arc::new(Mutex::new(Vec::new())),
        };
        let router = Router::new()
            .route("/groups/{group_id}", get(child_group))
            .route("/groups/{group_id}/state", get(child_state))
            .route(
                "/groups/{group_id}/state/commits/apply",
                post(apply_commits),
            )
            .with_state(api.clone());
        let task = tokio::spawn(async move {
            axum::serve(listener, router)
                .await
                .expect("serve child apply API");
        });
        let child = ManagedAgentChild {
            agent_id: child_id.to_string(),
            data_dir: data_dir.path().to_path_buf(),
        };
        (data_dir, child, api, task)
    }

    fn state_json(group_id: &str, revision: u64, hash: &str, roster_root: &str) -> Value {
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
            "state_hash": hash,
            "state_revision": revision,
        })
    }

    fn group_json(child_id: &str, revision: u64, legacy: Option<&str>) -> Value {
        let mut members = vec![json!({ "agent_id": child_id, "state": "active" })];
        if let Some(legacy) = legacy {
            members.push(json!({ "agent_id": legacy, "state": "active" }));
        }
        json!({
            "policy": { "confidentiality": "signed_public" },
            "roster_revision": revision,
            "members": members,
        })
    }

    fn apply_response(
        group_id: &str,
        child_revision: u64,
        owner: &GroupStateProjection,
        roster_revision: u64,
        applied: u64,
    ) -> Value {
        json!({
            "ok": true,
            "group_id": group_id,
            "applied": applied,
            "from_revision": child_revision,
            "state_revision": owner.state_revision,
            "roster_revision": roster_revision,
            "state_hash": owner.state_hash,
            "roster_root": owner.roster_root,
            "withdrawn": false,
        })
    }

    fn batch(child_revision: u64, owner_revision: u64) -> ValidatedOwnerCatchup {
        ValidatedOwnerCatchup {
            commits: (child_revision + 1..=owner_revision)
                .map(|revision| json!({ "commit": { "revision": revision }, "roster": {} }))
                .collect(),
            child_revision,
        }
    }

    #[tokio::test]
    async fn stale_x_and_o_rosters_apply_removals_and_reobserve_strict() {
        let group_id = "group";
        let child_id = "bb".repeat(32);
        let owner = GroupStateProjection::from_value(
            &state_json(group_id, 26, "owner-hash", "current-roster"),
            group_id,
        )
        .expect("owner projection");
        for (child_revision, legacy, stale_root) in [
            (21, "b820-legacy", "x-stale-roster"),
            (15, "61fe-legacy", "o-stale-roster"),
        ] {
            let batch = batch(child_revision, owner.state_revision);
            let response = apply_response(
                group_id,
                child_revision,
                &owner,
                26,
                u64::try_from(batch.commits.len()).expect("batch length"),
            );
            let (_dir, child, api, task) = spawn_child_apply_api(
                &child_id,
                (
                    group_json(&child_id, child_revision, Some(legacy)),
                    state_json(group_id, child_revision, "ancestor-hash", stale_root),
                ),
                (
                    group_json(&child_id, 26, None),
                    state_json(group_id, 26, "owner-hash", "current-roster"),
                ),
                response,
                None,
            )
            .await;
            apply_validated_child_catchup(
                &loopback_http_client().expect("HTTP client"),
                &child,
                &ChildGroupExpectation {
                    group_path: "/groups/group",
                    group_id,
                    owner_state: &owner,
                    forbidden_agent_id: None,
                },
                26,
                &batch,
            )
            .await
            .expect("stale roster catches up");
            assert_eq!(api.calls.load(Ordering::SeqCst), 1);
            let current = api.current.read().await;
            assert_eq!(
                group_member_state(&current.0, legacy),
                GroupMemberState::Absent
            );
            task.abort();
        }
    }

    #[tokio::test]
    async fn equivalent_but_behind_child_catches_up_to_exact_owner_head() {
        let group_id = "group";
        let child_id = "bb".repeat(32);
        let owner = GroupStateProjection::from_value(
            &state_json(group_id, 26, "owner-hash", "shared-roster"),
            group_id,
        )
        .expect("owner projection");
        let batch = batch(22, 26);
        let response = apply_response(group_id, 22, &owner, 26, 4);
        let (_dir, child, api, task) = spawn_child_apply_api(
            &child_id,
            (
                group_json(&child_id, 22, None),
                state_json(group_id, 22, "guide-hash", "shared-roster"),
            ),
            (
                group_json(&child_id, 26, None),
                state_json(group_id, 26, "owner-hash", "shared-roster"),
            ),
            response,
            None,
        )
        .await;
        apply_validated_child_catchup(
            &loopback_http_client().expect("HTTP client"),
            &child,
            &ChildGroupExpectation {
                group_path: "/groups/group",
                group_id,
                owner_state: &owner,
                forbidden_agent_id: None,
            },
            26,
            &batch,
        )
        .await
        .expect("equivalent ancestor catches up");
        assert_eq!(api.calls.load(Ordering::SeqCst), 1);
        assert_eq!(api.current.read().await.1["state_revision"], 26);
        task.abort();
    }

    #[tokio::test]
    async fn rejected_child_apply_never_falls_back_to_owner_roster_repair() {
        let group_id = "group";
        let child_id = "bb".repeat(32);
        let owner = GroupStateProjection::from_value(
            &state_json(group_id, 26, "owner-hash", "current-roster"),
            group_id,
        )
        .expect("owner projection");
        let before_state = state_json(group_id, 21, "fork-hash", "stale-roster");
        let batch = batch(21, 26);
        let (_dir, child, api, task) = spawn_child_apply_api(
            &child_id,
            (
                group_json(&child_id, 21, Some("legacy")),
                before_state.clone(),
            ),
            (
                group_json(&child_id, 26, None),
                state_json(group_id, 26, "owner-hash", "current-roster"),
            ),
            apply_response(group_id, 21, &owner, 26, 5),
            Some(AxumStatusCode::CONFLICT),
        )
        .await;
        let error = apply_validated_child_catchup(
            &loopback_http_client().expect("HTTP client"),
            &child,
            &ChildGroupExpectation {
                group_path: "/groups/group",
                group_id,
                owner_state: &owner,
                forbidden_agent_id: None,
            },
            26,
            &batch,
        )
        .await
        .expect_err("child rejection fails closed");
        assert!(error.contains("HTTP 409"));
        assert_eq!(api.current.read().await.1, before_state);
        assert_eq!(
            active_owner_action(ChildGroupObservation::PresentStale),
            ActiveOwnerAction::RejectStale
        );
        task.abort();
    }

    #[test]
    fn older_retention_window_still_requires_exact_ancestor_and_compatible_projection() {
        let group_id = "group";
        let child = GroupStateProjection::from_value(
            &state_json(group_id, 21, "child-hash", "stale-roster"),
            group_id,
        )
        .expect("child projection");
        let owner = GroupStateProjection::from_value(
            &state_json(group_id, 22, "owner-hash", "owner-roster"),
            group_id,
        )
        .expect("owner projection");
        let history = json!({
            "group_id": group_id,
            "state_revision": 22,
            "withdrawn": false,
            "from_revision": 22,
            "first_available_revision": 1,
            "latest_retained_revision": 22,
            "count": 1,
            "has_more": false,
            "commits": [{
                "commit": {
                    "group_id": group_id,
                    "revision": 22,
                    "prev_state_hash": "child-hash",
                    "state_hash": "owner-hash",
                    "roster_root": "owner-roster",
                    "policy_hash": "policy-hash",
                    "public_meta_hash": "public-meta-hash",
                    "security_binding": null,
                    "withdrawn": false,
                },
                "roster": {},
                "roster_root_verified": true,
            }],
        });
        validate_owner_catchup_history(&history, &child, &owner)
            .expect("exact retained ancestor is accepted");

        let mut fork = history;
        fork["commits"][0]["commit"]["prev_state_hash"] = json!("other-head");
        assert!(validate_owner_catchup_history(&fork, &child, &owner).is_err());
        let mut incompatible = child;
        incompatible.policy_hash = "other-policy".to_string();
        assert!(!catchup_projection_compatible(&incompatible, &owner));
    }
}
