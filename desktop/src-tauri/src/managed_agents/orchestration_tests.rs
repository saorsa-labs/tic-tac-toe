use super::*;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

use axum::{
    extract::State, http::StatusCode as AxumStatusCode, response::IntoResponse, routing::get, Json,
    Router,
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
    std::fs::write(data_dir.path().join("api-token"), "test-token").expect("write child api-token");

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

fn group_state_json(group_id: &str, revision: u64, state_hash: &str, roster_root: &str) -> Value {
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
fn startup_membership_eligibility_requires_exact_active_child() {
    let child_id = "bb".repeat(32);
    let other_id = "cc".repeat(32);

    assert!(owner_group_has_existing_active_member(
        &group_json(&child_id, 7, "active"),
        "group",
        &child_id,
    )
    .expect("parse active roster"));
    assert!(!owner_group_has_existing_active_member(
        &group_json(&other_id, 7, "active"),
        "group",
        &child_id,
    )
    .expect("parse absent child"));
    for state in ["invited", "banned", "removed"] {
        assert!(!owner_group_has_existing_active_member(
            &group_json(&child_id, 7, state),
            "group",
            &child_id,
        )
        .expect("parse non-active child"));
    }
}

#[test]
fn startup_membership_eligibility_fails_on_malformed_group_shape() {
    let child_id = "bb".repeat(32);
    let missing_members = json!({
        "policy": { "confidentiality": "signed_public" },
    });
    assert!(owner_group_has_existing_active_member(&missing_members, "group", &child_id).is_err());

    let malformed_member = json!({
        "policy": { "confidentiality": "signed_public" },
        "members": [{ "agent_id": child_id, "state": 7 }],
    });
    assert!(owner_group_has_existing_active_member(&malformed_member, "group", &child_id).is_err());

    let unsupported = json!({
        "policy": { "confidentiality": "secure" },
        "members": [],
    });
    assert!(owner_group_has_existing_active_member(&unsupported, "group", &child_id).is_err());
}

#[test]
fn existing_only_never_allows_owner_roster_mutation() {
    assert!(owner_roster_mutation_allowed(
        &GroupBindIntent::EnsureAttached
    ));
    assert!(!owner_roster_mutation_allowed(
        &GroupBindIntent::ExistingOnly {
            expected_child_agent_id: "bb".repeat(32),
        }
    ));
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
async fn retained_owner_ancestor_with_stale_roster_remains_a_safety_hold() {
    let group_id = "group";
    let child_id = "bb".repeat(32);
    // The raw observation does not itself prove or apply the retained suffix.
    // A different roster root must remain fail-closed until the catch-up path
    // validates the exact ancestry and the child reports the strict owner head.
    let owner_state = GroupStateProjection::from_value(
        &group_state_json(group_id, 26, "owner-hash", "current-roster"),
        group_id,
    )
    .expect("owner projection");
    let (_data_dir, child, _calls, task) = spawn_scripted_child_api(
        &child_id,
        vec![Some(group_json(&child_id, 21, "active"))],
        group_state_json(group_id, 21, "retained-ancestor-hash", "stale-roster"),
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
    .expect("observe signed ancestor with stale roster");

    assert_eq!(observation, ChildGroupObservation::PresentStale);
    assert_eq!(
        active_owner_action(observation),
        ActiveOwnerAction::RejectStale
    );
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
