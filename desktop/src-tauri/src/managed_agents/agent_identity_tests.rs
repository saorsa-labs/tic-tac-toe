use super::*;

#[test]
fn extracts_only_canonical_native_agent_ids() {
    let id = "ab".repeat(32);
    assert_eq!(
        extract_agent_id(&serde_json::json!({ "agent_id": id })),
        Some("ab".repeat(32))
    );
    let digit_id = "0123456789abcdef".repeat(4);
    assert_eq!(
        extract_agent_id(&serde_json::json!({ "data": { "agent_id": digit_id } })),
        Some("0123456789abcdef".repeat(4))
    );
    assert!(extract_agent_id(&serde_json::json!({ "agent_id": "ABC" })).is_none());
    assert!(extract_agent_id(&serde_json::json!({ "agent_id": "zz".repeat(32) })).is_none());
}

#[test]
fn child_config_keeps_an_explicit_isolated_directory() {
    let cfg = AgentChildConfig::for_test(
        PathBuf::from("/tmp/company-agent-a"),
        PathBuf::from("/tmp/x0xd"),
    );
    assert_eq!(cfg.data_dir, PathBuf::from("/tmp/company-agent-a"));
}

#[test]
fn handle_debug_never_contains_a_token_field() {
    let debug_fields = "agent_id base_url data_dir owned";
    assert!(!debug_fields.contains("api-token"));
}
