use std::time::Duration;

use super::*;

#[test]
fn error_display_never_leaks_token() {
    // The token never flows into an error variant by construction; this
    // guards the contract against a future field addition that surfaces it.
    let cases = [
        X0xClientError::DaemonUnavailable("api-token missing"),
        X0xClientError::Transport("connection refused".into()),
        X0xClientError::Status(401, "unauthorized".into()),
        X0xClientError::Decode("EOF".into()),
    ];
    for e in cases {
        let s = e.to_string();
        assert!(!s.contains("Bearer"), "token prefix leaked: {s}");
    }
}

#[test]
fn history_row_thread_fields_default_null() {
    // A daemon predating the thread contract omits thread_root/parent; the
    // row must still parse with both null.
    let json = serde_json::json!({
        "id": 7i64,
        "msg_id": "deadbeef",
        "scope": "group:abc",
        "author_agent": null,
        "author_machine": null,
        "sent_at_ms": 0i64,
        "seen_at_ms": 0i64,
        "direction": "inbound",
        "content_type": "text/plain",
        "payload": "",
        "signed": false,
        "provenance": "verified_envelope"
    });
    let row: HistoryRow = serde_json::from_value(json).expect("legacy row must parse");
    assert_eq!(row.thread_root, None);
    assert_eq!(row.thread_parent, None);
}

#[test]
fn history_row_root_is_self_referential() {
    let root_id = "cafebabe";
    let json = serde_json::json!({
        "id": 1i64,
        "msg_id": root_id,
        "scope": "topic:m3",
        "author_agent": "00",
        "author_machine": null,
        "sent_at_ms": 1i64,
        "seen_at_ms": 1i64,
        "direction": "outbound",
        "content_type": "text/plain",
        "payload": "aGk=",
        "signed": true,
        "provenance": "verified_envelope",
        "thread_root": root_id,
        "thread_parent": null
    });
    let row: HistoryRow = serde_json::from_value(json).unwrap();
    assert_eq!(row.thread_root.as_deref(), Some(root_id));
    assert_eq!(row.thread_parent, None);
    // Self-referential root invariant: thread_root == msg_id.
    assert_eq!(row.thread_root.as_deref(), Some(row.msg_id.as_str()));
}

#[test]
fn ws_frame_round_trips() {
    // The frame union must deserialize the daemon's WsOutbound shapes.
    let live = r#"{"type":"live","topic":"topic:m3"}"#;
    let f: X0xFrame = serde_json::from_str(live).unwrap();
    assert!(matches!(f, X0xFrame::Live { ref topic } if topic == "topic:m3"));

    let msg = r#"{"type":"message","topic":"topic:m3","payload":"aGk=","origin":"00","thread_root":"cafebabe","thread_parent":"deadbeef"}"#;
    let f: X0xFrame = serde_json::from_str(msg).unwrap();
    match f {
        X0xFrame::Message {
            thread_root,
            thread_parent,
            ..
        } => {
            assert_eq!(thread_root.as_deref(), Some("cafebabe"));
            assert_eq!(thread_parent.as_deref(), Some("deadbeef"));
        }
        _ => panic!("wrong variant"),
    }
}
#[test]
fn ws_frame_serializes_camelcase_for_ts_channel() {
    // The Channel-facing frame MUST serialize camelCase (DesktopNativeApi TS
    // contract) while still deserializing the daemon's snake_case wire.
    let f = X0xFrame::Connected {
        session_id: "s1".into(),
        agent_id: "a1".into(),
    };
    let s = serde_json::to_string(&f).unwrap();
    assert!(s.contains(r#""type":"connected""#), "tag unchanged: {s}");
    assert!(
        s.contains(r#""sessionId":"s1""#),
        "camelCase sessionId: {s}"
    );
    assert!(s.contains(r#""agentId":"a1""#), "camelCase agentId: {s}");
    assert!(
        !s.contains("session_id"),
        "snake_case leaked into serialize: {s}"
    );

    let m = X0xFrame::Message {
        topic: "topic:m3".into(),
        payload: "aGk=".into(),
        origin: None,
        msg_id: None,
        thread_root: Some("cafebabe".into()),
        thread_parent: None,
    };
    let s = serde_json::to_string(&m).unwrap();
    assert!(
        s.contains(r#""threadRoot":"cafebabe""#),
        "camelCase threadRoot: {s}"
    );

    // Round-trip: the daemon's snake_case wire still deserializes.
    let back: X0xFrame = serde_json::from_str(
        r#"{"type":"message","topic":"t","payload":"","origin":null,"thread_root":"x","thread_parent":null}"#,
    )
    .unwrap();
    assert!(
        matches!(back, X0xFrame::Message { thread_root, .. } if thread_root.as_deref() == Some("x"))
    );
}

#[test]
fn direct_message_frame_round_trips() {
    // The daemon's WsOutbound::DirectMessage (snake_case) must deserialize,
    // and the Channel-facing serialization must be camelCase. `msg_id` is
    // absent on old daemons and always-present on current ones.
    let wire = r#"{"type":"direct_message","sender":"00aa","machine_id":"01bb","payload":"aGk=","received_at":42,"verified":true,"thread_root":"root","thread_parent":"par"}"#;
    let f: X0xFrame = serde_json::from_str(wire).unwrap();
    match f {
        X0xFrame::DirectMessage {
            sender,
            machine_id,
            payload,
            received_at,
            verified,
            msg_id,
            thread_root,
            thread_parent,
            trust_decision,
        } => {
            assert_eq!(sender, "00aa");
            assert_eq!(machine_id, "01bb");
            assert_eq!(payload, "aGk=");
            assert_eq!(received_at, 42);
            assert!(verified);
            assert_eq!(msg_id, None, "msg_id absent on old daemons");
            assert_eq!(thread_root.as_deref(), Some("root"));
            assert_eq!(thread_parent.as_deref(), Some("par"));
            assert_eq!(trust_decision, None);
        }
        _ => panic!("wrong variant"),
    }

    // Current daemon always sends msg_id; it must parse through.
    let with_id = r#"{"type":"direct_message","msg_id":"cafe","sender":"00aa","machine_id":"01bb","payload":"","received_at":1,"verified":false}"#;
    let f: X0xFrame = serde_json::from_str(with_id).unwrap();
    assert!(matches!(
        f,
        X0xFrame::DirectMessage { msg_id, .. } if msg_id.as_deref() == Some("cafe")
    ));

    // Channel-facing serialization is camelCase, tag stays direct_message.
    let dm = X0xFrame::DirectMessage {
        msg_id: Some("cafe".into()),
        sender: "00aa".into(),
        machine_id: "01bb".into(),
        payload: "aGk=".into(),
        received_at: 42,
        verified: true,
        trust_decision: None,
        thread_root: None,
        thread_parent: None,
    };
    let s = serde_json::to_string(&dm).unwrap();
    assert!(s.contains(r#""type":"direct_message""#), "tag: {s}");
    assert!(
        s.contains(r#""machineId":"01bb""#),
        "camelCase machineId: {s}"
    );
    assert!(
        s.contains(r#""receivedAt":42"#),
        "camelCase receivedAt: {s}"
    );
    assert!(s.contains(r#""msgId":"cafe""#), "camelCase msgId: {s}");
    assert!(!s.contains("machine_id"), "snake leaked: {s}");
}

#[test]
fn group_public_topic_matches_daemon_public_topic_for() {
    // Mirrors x0xd groups::public_topic_for(stable_id) verbatim — a
    // divergent string here means live SignedPublic chat never arrives.
    assert_eq!(group_public_topic("abc123"), "x0x.groups.public.abc123");
    assert_eq!(group_public_topic(""), "x0x.groups.public.");
}

#[test]
fn group_policy_wire_parses_confidentiality_snake_case() {
    // The daemon serializes the enum snake_case; any unknown/missing
    // value must leave confidentiality None. `resolve_group_transport`
    // fails closed on None/unknown (never defaults to MLS).
    let detail: GroupDetailWire = serde_json::from_value(
        serde_json::json!({"chat_topic":"t","policy":{"confidentiality":"signed_public"}}),
    )
    .unwrap();
    assert_eq!(detail.chat_topic, "t");
    assert_eq!(
        detail.policy.confidentiality.as_deref(),
        Some("signed_public")
    );

    let mls: GroupDetailWire = serde_json::from_value(
        serde_json::json!({"chat_topic":"x","policy":{"confidentiality":"mls_encrypted"}}),
    )
    .unwrap();
    assert_eq!(mls.policy.confidentiality.as_deref(), Some("mls_encrypted"));
}

#[test]
fn send_direct_body_omits_thread_fields_when_none() {
    // The daemon validates `agent_id` + `payload` and treats absent
    // thread fields as `ThreadMeta::NONE`. `skip_serializing_if` must
    // drop both so a plain one-to-one DM sends exactly two fields.
    let agent_id = "ab".repeat(32);
    let body = SendDirectBody {
        agent_id: &agent_id,
        payload: "aGk=",
        logical_id: None,
        thread_root: None,
        thread_parent: None,
    };
    let v: serde_json::Value = serde_json::to_value(&body).expect("serialize direct body");
    assert_eq!(v["agent_id"], "ab".repeat(32));
    assert_eq!(v["payload"], "aGk=");
    assert!(
        v.get("logical_id").is_none(),
        "logical_id must be absent when None: {v}"
    );
    assert!(
        v.get("thread_root").is_none(),
        "thread_root must be absent when None: {v}"
    );
    assert!(
        v.get("thread_parent").is_none(),
        "thread_parent must be absent when None: {v}"
    );
}

#[test]
fn send_direct_body_includes_thread_ancestry_when_present() {
    // Thread replies forward both 64-hex msg_ids verbatim (snake_case);
    // the daemon validates them to 32 bytes via ThreadMeta::from_hex.
    let root = "c".repeat(64);
    let parent = "d".repeat(64);
    let agent_id = "ab".repeat(32);
    let body = SendDirectBody {
        agent_id: &agent_id,
        payload: "aGk=",
        logical_id: Some("client-123"),
        thread_root: Some(root.as_str()),
        thread_parent: Some(parent.as_str()),
    };
    let v: serde_json::Value = serde_json::to_value(&body).expect("serialize threaded direct body");
    assert_eq!(v["thread_root"], root);
    assert_eq!(v["thread_parent"], parent);
    assert_eq!(v["logical_id"], "client-123");
    // Fields are snake_case on the wire (the daemon deserializes verbatim).
    assert!(v.get("threadRoot").is_none(), "camelCase leaked: {v}");
}

#[test]
fn direct_send_receipt_parses_full_daemon_response() {
    // The daemon's POST /direct/send success body (snake_case). The
    // canonical msg_id is NOT in this response — only the request_id;
    // msg_id is reconciled later via /history keyed by the clientId.
    let resp = r#"{"ok":true,"path":"raw_quic_acked","retries_used":1,"request_id":"deadbeef","require_ack":{"ok":true,"rtt_ms":42}}"#;
    let r: DirectSendReceipt = serde_json::from_str(resp).unwrap();
    assert!(r.ok);
    assert_eq!(r.path.as_deref(), Some("raw_quic_acked"));
    assert_eq!(r.retries_used, Some(1));
    assert_eq!(r.request_id.as_deref(), Some("deadbeef"));
}

#[test]
fn direct_send_receipt_parses_minimal_ok_body() {
    // A minimal acceptance body still parses (all fields #[serde(default)]).
    let r: DirectSendReceipt = serde_json::from_str(r#"{"ok":true}"#).unwrap();
    assert!(r.ok);
    assert_eq!(r.path, None);
    assert_eq!(r.retries_used, None);
    assert_eq!(r.request_id, None);
}

#[test]
fn direct_send_receipt_parses_error_body() {
    // A daemon error body (ok:false) still deserializes so the transport
    // surfaces the structured status rather than a decode failure.
    let r: DirectSendReceipt =
        serde_json::from_str(r#"{"ok":false,"path":null,"retries_used":0,"request_id":null}"#)
            .unwrap();
    assert!(!r.ok);
    assert_eq!(r.path, None);
}
#[test]
fn group_message_redelivery_receipt_requires_exact_causal_fields() {
    let group_id = "group-1";
    let msg_id = "a".repeat(64);
    let agent_id = "b".repeat(64);
    let value = serde_json::json!({
        "ok": true,
        "group_id": group_id,
        "msg_id": msg_id,
        "agent_id": agent_id,
        "outcome": "committed",
    });
    let receipt: GroupMessageRedeliveryReceipt =
        serde_json::from_value(value).expect("redelivery receipt parses");
    assert!(receipt.ok);
    assert_eq!(receipt.group_id, group_id);
    assert_eq!(receipt.msg_id, msg_id);
    assert_eq!(receipt.agent_id, agent_id);
    assert_eq!(receipt.outcome, "committed");
}

#[test]
fn direct_send_timeout_outlives_strict_daemon_retry_budget_only() {
    const STRICT_DAEMON_RETRY_BUDGET: Duration = Duration::from_secs(8 + 8 + 8);

    assert_eq!(REQUEST_TIMEOUT, Duration::from_secs(15));
    assert_eq!(DIRECT_SEND_REQUEST_TIMEOUT, Duration::from_secs(30));
    assert!(DIRECT_SEND_REQUEST_TIMEOUT > STRICT_DAEMON_RETRY_BUDGET);
}

#[test]
fn send_group_body_serializes_thread_ancestry() {
    // ADR-0029: SignedPublic send (`POST /groups/:id/send`) carries optional
    // `thread_root`/`thread_parent` (64-hex canonical msg_ids). Both use
    // `skip_serializing_if = "Option::is_none"` so a non-threaded v1 message
    // serializes to exactly `{ body, kind }` (byte-identical to pre-threading).
    let root = "a".repeat(64);

    // Non-threaded: exactly two keys, no ancestry.
    let plain = SendGroupBody {
        body: "hello",
        kind: "chat",
        thread_root: None,
        thread_parent: None,
    };
    let v: serde_json::Value = serde_json::to_value(&plain).expect("serialize group body");
    assert_eq!(v["body"], "hello");
    assert_eq!(v["kind"], "chat");
    assert!(
        v.get("thread_root").is_none(),
        "thread_root must not exist on a non-threaded send: {v}"
    );
    assert!(
        v.get("thread_parent").is_none(),
        "thread_parent must not exist on a non-threaded send: {v}"
    );

    // Threaded: carries thread_root and thread_parent.
    let threaded = SendGroupBody {
        body: "reply",
        kind: "chat",
        thread_root: Some(&root),
        thread_parent: Some(&root),
    };
    let v: serde_json::Value =
        serde_json::to_value(&threaded).expect("serialize threaded group body");
    assert_eq!(v["body"], "reply");
    assert_eq!(v["kind"], "chat");
    assert_eq!(v["thread_root"], root);
    assert_eq!(v["thread_parent"], root);
}
