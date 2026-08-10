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
fn managed_pubkey_maps_to_valid_stable_x0xd_instance_name() {
    let pubkey = "AB".repeat(32);
    let bounded = bounded_agent_child_instance(&pubkey);
    let different_pubkey = format!("{}ac", "AB".repeat(31));
    let expected_dir_name = format!("x0x-managed-{bounded}");

    assert_eq!(bounded.len(), MANAGED_INSTANCE_KEY_LEN);
    assert!(bounded.bytes().all(|byte| byte.is_ascii_hexdigit()));
    assert_eq!(bounded, bounded_agent_child_instance(&pubkey));
    assert_ne!(bounded, bounded_agent_child_instance(&different_pubkey));
    assert_eq!(format!("managed-{bounded}").len(), 64);
    assert_eq!(
        agent_child_data_dir(&bounded)
            .and_then(|path| path.file_name().map(|name| name.to_owned()))
            .and_then(|name| name.into_string().ok())
            .as_deref(),
        Some(expected_dir_name.as_str())
    );
}

#[test]
fn handle_debug_never_contains_a_token_field() {
    let debug_fields = "agent_id base_url data_dir owned";
    assert!(!debug_fields.contains("api-token"));
}
