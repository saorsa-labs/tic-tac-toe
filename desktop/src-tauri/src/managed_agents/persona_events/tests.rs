use super::*;
use std::collections::BTreeMap;

fn sample_persona() -> AgentDefinition {
    AgentDefinition {
        id: "test-persona".to_string(),
        display_name: "Test Persona".to_string(),
        avatar_url: Some("https://example.com/avatar.png".to_string()),
        system_prompt: "You are a test assistant.".to_string(),
        runtime: Some("goose".to_string()),
        model: Some("claude-opus-4".to_string()),
        provider: Some("anthropic".to_string()),
        name_pool: vec!["Alpha".to_string(), "Beta".to_string()],
        is_builtin: false,
        is_active: true,
        source_team: None,
        source_team_persona_slug: Some("test-slug".to_string()),
        env_vars: BTreeMap::from([("KEY".to_string(), "value".to_string())]),
        respond_to: None,
        respond_to_allowlist: Vec::new(),
        parallelism: None,
        created_at: "2025-01-01T00:00:00Z".to_string(),
        updated_at: "2025-01-01T00:00:00Z".to_string(),
    }
}

#[test]
fn d_tag_uses_slug_when_available() {
    let record = sample_persona();
    assert_eq!(persona_d_tag(&record), "test-slug");
}

#[test]
fn d_tag_falls_back_to_id() {
    let mut record = sample_persona();
    record.source_team_persona_slug = None;
    assert_eq!(persona_d_tag(&record), "test-persona");
}

/// Mirror of the relay slug grammar (`ingest.rs:923` `^[a-z0-9][a-z0-9_-]{0,63}$`)
/// so the normalization tests assert what the relay actually enforces.
fn passes_relay_slug_grammar(d: &str) -> bool {
    let bytes = d.as_bytes();
    !d.is_empty()
        && d.len() <= 64
        && (bytes[0].is_ascii_lowercase() || bytes[0].is_ascii_digit())
        && bytes[1..]
            .iter()
            .all(|&b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
}

#[test]
fn d_tag_normalizes_pack_slug_to_relay_grammar() {
    // The cited failing cases: mixed-case and leading-underscore pack slugs
    // that the relay rejects un-normalized → pending forever.
    for (raw, expected) in [
        ("CodeReviewer", "codereviewer"),
        ("_ops", "a_ops"),
        ("Code-Reviewer", "code-reviewer"),
        ("UPPER_snake", "upper_snake"),
        ("-leading-dash", "a-leading-dash"),
    ] {
        let mut record = sample_persona();
        record.source_team_persona_slug = Some(raw.to_string());
        let d = persona_d_tag(&record);
        assert_eq!(d, expected, "normalization of {raw:?}");
        assert!(
            passes_relay_slug_grammar(&d),
            "normalized {raw:?} -> {d:?} still fails the relay grammar"
        );
    }
}

#[test]
fn d_tag_already_valid_slug_is_unchanged() {
    // In-app personas use a lowercase-hex UUID id — already valid, must pass
    // through untouched (no spurious coordinate change on existing data).
    let mut record = sample_persona();
    record.source_team_persona_slug = None;
    record.id = "11111111-2222-3333-4444-555555555555".to_string();
    let d = persona_d_tag(&record);
    assert_eq!(d, "11111111-2222-3333-4444-555555555555");
    assert!(passes_relay_slug_grammar(&d));
}

/// NIP-AP behavioral defaults are LIVE since B5 (create-path
/// unification): the wire fields are carried on AgentDefinition in wire
/// shape and re-emitted verbatim by the projection — a foreign
/// definition's behavioral values now survive a local
/// edit-and-republish cycle. This test replaces
/// `behavioral_defaults_are_staged_not_applied` (the staging lock),
/// whose deliberate removal was pinned in the B5 review gates.
#[test]
fn behavioral_defaults_survive_record_round_trip() {
    const FOREIGN: &str = r#"{"display_name":"F","system_prompt":"p","respond_to":"anyone","respond_to_allowlist":["deadbeef"],"parallelism":4}"#;
    let parsed: PersonaEventContent = serde_json::from_str(FOREIGN).unwrap();
    // Wire layer preserves the fields...
    assert_eq!(parsed.respond_to.as_deref(), Some("anyone"));
    assert_eq!(parsed.parallelism, Some(4));
    // ...and the record round-trip now carries them through.
    let record = persona_from_event_content_for_test(parsed);
    let reprojected = persona_event_content(&record);
    assert_eq!(reprojected.respond_to.as_deref(), Some("anyone"));
    assert_eq!(reprojected.respond_to_allowlist, vec!["deadbeef"]);
    assert_eq!(reprojected.parallelism, Some(4));
}

/// B5 hash row 1: a quad-absent definition's content bytes — and
/// therefore `persona_content_hash` — are identical before and after
/// quad activation. Pre-activation the projection hardcoded `None`;
/// post-activation it copies the record's (absent) quad. Both serialize
/// to the same bytes via `skip_serializing_if`, so no drift badge flips
/// and no republish wave fires for quad-absent definitions.
#[test]
fn quad_absent_definition_hash_stable_across_activation() {
    let record = AgentDefinition {
        id: "quad-absent".to_string(),
        display_name: "Test".to_string(),
        avatar_url: None,
        system_prompt: "Hello".to_string(),
        runtime: Some("goose".to_string()),
        model: Some("gpt-oss".to_string()),
        provider: None,
        name_pool: vec!["nib".to_string()],
        is_builtin: false,
        is_active: true,
        source_team: None,
        source_team_persona_slug: None,
        env_vars: BTreeMap::new(),
        respond_to: None,
        respond_to_allowlist: Vec::new(),
        parallelism: None,
        created_at: "2026-01-01T00:00:00Z".to_string(),
        updated_at: "2026-01-01T00:00:00Z".to_string(),
    };
    let live = persona_event_content(&record);
    // The reserved-era projection: identical fields, quad hardcoded off.
    let reserved_era = PersonaEventContent {
        respond_to: None,
        respond_to_allowlist: Vec::new(),
        parallelism: None,
        ..live.clone()
    };
    assert_eq!(
        serde_json::to_string(&live).unwrap(),
        serde_json::to_string(&reserved_era).unwrap(),
        "quad-absent projection must serialize byte-identically to the reserved era"
    );
    assert_eq!(
        persona_content_hash(&live),
        persona_content_hash(&reserved_era)
    );
}

/// Test-only bridge: build an AgentDefinition from parsed content fields,
/// mirroring the inbound content→record field mapping without a signed event.
fn persona_from_event_content_for_test(content: PersonaEventContent) -> AgentDefinition {
    AgentDefinition {
        id: "staged".to_string(),
        display_name: content.display_name,
        avatar_url: content.avatar_url,
        system_prompt: content.system_prompt.unwrap_or_default(),
        runtime: content.runtime,
        model: content.model,
        provider: content.provider,
        name_pool: content.name_pool,
        is_builtin: false,
        is_active: true,
        source_team: None,
        source_team_persona_slug: None,
        env_vars: BTreeMap::new(),
        respond_to: content.respond_to,
        respond_to_allowlist: content.respond_to_allowlist,
        parallelism: content.parallelism,
        created_at: "2026-01-01T00:00:00Z".to_string(),
        updated_at: "2026-01-01T00:00:00Z".to_string(),
    }
}

#[test]
fn persona_content_hash_is_deterministic() {
    let content = PersonaEventContent {
        display_name: "Test".to_string(),
        avatar_url: None,
        system_prompt: Some("Hello".to_string()),
        runtime: None,
        model: None,
        provider: None,
        name_pool: vec![],
        respond_to: None,
        respond_to_allowlist: Vec::new(),
        parallelism: None,
    };
    let hash1 = persona_content_hash(&content);
    let hash2 = persona_content_hash(&content);
    assert_eq!(hash1, hash2);
    assert_eq!(hash1.len(), 64); // SHA-256 hex
}

#[test]
fn persona_content_hash_changes_on_edit() {
    let content1 = PersonaEventContent {
        display_name: "Test".to_string(),
        avatar_url: None,
        system_prompt: Some("Hello".to_string()),
        runtime: None,
        model: None,
        provider: None,
        name_pool: vec![],
        respond_to: None,
        respond_to_allowlist: Vec::new(),
        parallelism: None,
    };
    let mut content2 = content1.clone();
    content2.system_prompt = Some("Goodbye".to_string());
    assert_ne!(
        persona_content_hash(&content1),
        persona_content_hash(&content2)
    );
}

// ── persona_field_with_record_fallback ────────────────────────────────────

#[test]
fn field_fallback_persona_present_wins() {
    assert_eq!(
        persona_field_with_record_fallback(Some("persona-model"), Some("record-model")),
        Some("persona-model".to_owned()),
    );
}

#[test]
fn field_fallback_persona_blank_uses_record() {
    assert_eq!(
        persona_field_with_record_fallback(None, Some("record-model")),
        Some("record-model".to_owned()),
    );
    assert_eq!(
        persona_field_with_record_fallback(Some("  "), Some("record-model")),
        Some("record-model".to_owned()),
    );
}

#[test]
fn field_fallback_both_blank_is_none() {
    assert_eq!(persona_field_with_record_fallback(None, None), None);
    assert_eq!(persona_field_with_record_fallback(Some(""), Some("")), None);
}

#[test]
fn field_fallback_record_blank_is_none() {
    assert_eq!(
        persona_field_with_record_fallback(None, Some("  ")),
        None,
        "whitespace-only record value must also be treated as blank"
    );
}

// ── PersonaSnapshot.runtime ───────────────────────────────────────────────

/// (b) The snapshot carries the persona's runtime VERBATIM — including None,
/// which clears a stale materialized value on the instance record. Unlike
/// model/provider, runtime does not fall back to the record's own value:
/// instances have no user-owned runtime, so the definition must stay
/// authoritative.
#[test]
fn snapshot_runtime_verbatim_from_persona() {
    let persona = sample_persona(); // runtime = Some("goose")
    let snap = persona_snapshot_with_agent_config_fallback(&persona, Some("gpt-4"), Some("openai"));
    assert_eq!(
        snap.runtime.as_deref(),
        Some("goose"),
        "persona runtime Some must be copied verbatim into snapshot"
    );

    let mut no_runtime = sample_persona();
    no_runtime.runtime = None;
    let snap =
        persona_snapshot_with_agent_config_fallback(&no_runtime, Some("gpt-4"), Some("openai"));
    assert_eq!(
        snap.runtime, None,
        "persona runtime None must produce None snapshot (clears stale materialized value)"
    );
}

// ── persona_snapshot_with_agent_config_fallback ────────────────────────────

/// Helper: a persona with no model/provider configured.
fn blank_model_persona() -> AgentDefinition {
    AgentDefinition {
        model: None,
        provider: None,
        ..sample_persona()
    }
}

/// (a) Persona leaves model/provider blank, agent record has values →
/// record values preserved AND source_version still updated to current hash.
#[test]
fn fallback_preserves_record_values_when_persona_blank() {
    let persona = blank_model_persona();
    let expected_version = persona_content_hash(&persona_event_content(&persona));

    let snapshot =
        persona_snapshot_with_agent_config_fallback(&persona, Some("gpt-4o"), Some("openai"));

    assert_eq!(
        snapshot.model.as_deref(),
        Some("gpt-4o"),
        "blank persona model must fall back to agent record value"
    );
    assert_eq!(
        snapshot.provider.as_deref(),
        Some("openai"),
        "blank persona provider must fall back to agent record value"
    );
    assert_eq!(
        snapshot.source_version, expected_version,
        "source_version must still reflect current persona hash"
    );
}

/// (b) Persona has model/provider set → persona wins over agent record.
#[test]
fn fallback_persona_wins_when_set() {
    let persona = sample_persona(); // has model=Some("claude-opus-4"), provider=Some("anthropic")

    let snapshot = persona_snapshot_with_agent_config_fallback(
        &persona,
        Some("gpt-4o"), // agent had a different model
        Some("openai"), // agent had a different provider
    );

    assert_eq!(
        snapshot.model.as_deref(),
        Some("claude-opus-4"),
        "persona model must win when persona has a value"
    );
    assert_eq!(
        snapshot.provider.as_deref(),
        Some("anthropic"),
        "persona provider must win when persona has a value"
    );
}

/// (c) Both blank → snapshot keeps None; a genuinely unconfigured agent
/// stays unconfigured (no fabricated values).
#[test]
fn fallback_both_blank_stays_none() {
    let persona = blank_model_persona();

    let snapshot = persona_snapshot_with_agent_config_fallback(
        &persona, None, // agent also has no model
        None, // agent also has no provider
    );

    assert!(
        snapshot.model.is_none(),
        "neither persona nor agent has model — snapshot must be None"
    );
    assert!(
        snapshot.provider.is_none(),
        "neither persona nor agent has provider — snapshot must be None"
    );
}

/// Whitespace-only values on the persona are treated as blank; agent
/// fallback applies.
#[test]
fn fallback_treats_whitespace_only_persona_value_as_blank() {
    let mut persona = sample_persona();
    persona.model = Some("  ".to_string());
    persona.provider = Some("\t".to_string());

    let snapshot = persona_snapshot_with_agent_config_fallback(
        &persona,
        Some("claude-opus-4"),
        Some("anthropic"),
    );

    assert_eq!(
        snapshot.model.as_deref(),
        Some("claude-opus-4"),
        "whitespace-only persona model must be treated as blank"
    );
    assert_eq!(
        snapshot.provider.as_deref(),
        Some("anthropic"),
        "whitespace-only persona provider must be treated as blank"
    );
}

/// Cross-field independence: persona sets model but not provider → model
/// comes from persona, provider falls back to the record.  This is the
/// practically common case (model-only personas).
#[test]
fn fallback_persona_model_set_provider_blank_uses_record_provider() {
    let mut persona = sample_persona(); // model=Some("claude-opus-4"), provider=Some("anthropic")
    persona.provider = None; // blank provider on persona

    let snapshot = persona_snapshot_with_agent_config_fallback(
        &persona,
        Some("gpt-4o"), // record model (should be overridden by persona)
        Some("openai"), // record provider (should be preserved)
    );

    assert_eq!(
        snapshot.model.as_deref(),
        Some("claude-opus-4"),
        "persona model must win when persona has a value"
    );
    assert_eq!(
        snapshot.provider.as_deref(),
        Some("openai"),
        "record provider must be used when persona provider is blank"
    );
}

/// Inverse: persona sets provider but not model → provider comes from
/// persona, model falls back to the record.
#[test]
fn fallback_persona_provider_set_model_blank_uses_record_model() {
    let mut persona = sample_persona(); // model=Some("claude-opus-4"), provider=Some("anthropic")
    persona.model = None; // blank model on persona

    let snapshot = persona_snapshot_with_agent_config_fallback(
        &persona,
        Some("gpt-4o"), // record model (should be preserved)
        Some("openai"), // record provider (should be overridden by persona)
    );

    assert_eq!(
        snapshot.model.as_deref(),
        Some("gpt-4o"),
        "record model must be used when persona model is blank"
    );
    assert_eq!(
        snapshot.provider.as_deref(),
        Some("anthropic"),
        "persona provider must win when persona has a value"
    );
}
