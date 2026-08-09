//! Persona definition projections, drift hash, slug derivation, and the
//! pinned snapshot applied at agent create/restore.
//!
//! This was previously the kind:30175 relay-event serialization layer; the
//! relay transport is gone with the native x0x data cutover. What remains is
//! the nostr-free local projection (`PersonaEventContent`), the content drift
//! hash, the slug derivation that keys local records, and the snapshot logic.

use serde::{Deserialize, Serialize};

use super::{AgentDefinition, ManagedAgentRecord};

/// The projected content fields of a persona definition.
///
/// Field order is significant: `persona_content_hash` serializes this struct
/// verbatim to produce the drift digest, so a reorder changes every digest.
/// `skip_serializing_if` keeps pre-revision hashes stable across the
/// unified-model widening (a bare `Option` serializing `null` would flip
/// every digest).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersonaEventContent {
    pub display_name: String,
    /// Optional since the unified agent model (NIP-AP revision): a definition
    /// can be pure configuration. Writers emit `Some` whenever the record has
    /// a prompt (including the empty string) so pre-revision content bytes —
    /// and therefore `persona_content_hash` — are unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avatar_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub name_pool: Vec<String>,
    /// Definition-level defaults copied onto instances at creation
    /// (NIP-AP behavioral fields). Absent = defer to client defaults;
    /// `skip_serializing_if` keeps pre-revision hashes stable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub respond_to: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub respond_to_allowlist: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parallelism: Option<u32>,
}

/// Derive the d-tag (persona slug) from a `AgentDefinition`.
///
/// Uses `source_team_persona_slug` if available, otherwise falls back to
/// `id`, then normalizes to the slug grammar `^[a-z0-9][a-z0-9_-]{0,63}$`
/// via [`normalize_d_tag`]. Team pack slugs are `[a-zA-Z0-9_-]+` (mixed
/// case, may lead with `_`/`-`), so an un-normalized slug like
/// `CodeReviewer` or `_ops` is normalized before use. In-app personas use
/// a lowercase-hex UUID `id` that is already valid, so they are unaffected.
///
/// This is the local record match key; every caller routes through it so
/// the derived slug is consistent and cannot drift.
pub fn persona_d_tag(record: &AgentDefinition) -> String {
    let raw = record
        .source_team_persona_slug
        .as_deref()
        .unwrap_or(&record.id);
    normalize_d_tag(raw)
}

/// Normalize a raw slug to the grammar `^[a-z0-9][a-z0-9_-]{0,63}$`.
///
/// - ASCII-lowercase every char (pack slugs are `[a-zA-Z0-9_-]+`, so this is
///   the only transform uppercase slugs need).
/// - Map any char outside `[a-z0-9_-]` to `-` (defensive; pack slugs never
///   contain such chars, but `id` fallbacks and future inputs might).
/// - If the first char is not `[a-z0-9]` (i.e. a leading `_`/`-`), prepend `a`
///   rather than trimming — trimming `_ops`→`ops` would collide with a real
///   `ops` pack, whereas the prefix keeps distinct inputs distinct.
/// - Truncate to 64 bytes (the grammar's max).
///
/// The transform is deterministic. It is NOT globally injective (`A-b` and
/// `a_b` both contain only safe chars and stay distinct, but two slugs
/// differing only in case — e.g. `Ops` and `ops` — collapse to the same
/// slug). That case-fold collision is inherent to the lowercase grammar and
/// is the desired behavior: same logical persona, one key.
fn normalize_d_tag(raw: &str) -> String {
    let mut out: String = raw
        .chars()
        .map(|c| {
            let c = c.to_ascii_lowercase();
            if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();
    if !out
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphanumeric())
    {
        out.insert(0, 'a');
    }
    out.truncate(64);
    out
}

/// SHA-256 (lowercase hex) of a persona's canonical content JSON.
///
/// The drift indicator compares this digest, not event timestamps, to decide
/// whether an agent's persona snapshot is stale — timestamps are fragile across
/// clock skew and export/import round-trips. `PersonaEventContent` field order
/// is fixed by the struct definition, so `serde_json` produces a stable
/// canonical encoding.
pub fn persona_content_hash(content: &PersonaEventContent) -> String {
    use sha2::{Digest, Sha256};
    let json = serde_json::to_vec(content).unwrap_or_default();
    let digest = Sha256::digest(&json);
    hex::encode(digest)
}

/// Project a `AgentDefinition` onto the content fields published in persona
/// events and engrams. Centralizes the field mapping so a new persona field is
/// added in exactly one place.
pub fn persona_event_content(record: &AgentDefinition) -> PersonaEventContent {
    PersonaEventContent {
        display_name: record.display_name.clone(),
        avatar_url: record.avatar_url.clone(),
        // Always Some — including for an empty prompt — so pre-revision
        // records serialize byte-identically and persona_content_hash is
        // stable across the upgrade (drift badges must not flip).
        system_prompt: Some(record.system_prompt.clone()),
        runtime: record.runtime.clone(),
        model: record.model.clone(),
        provider: record.provider.clone(),
        name_pool: record.name_pool.clone(),
        // NIP-AP behavioral defaults: live since the create-path unification
        // (B5) — carried on AgentDefinition in wire shape and copied verbatim.
        // Quad-absent records serialize identically to the reserved era, so
        // persona_content_hash is stable across the activation (guarded by
        // `quad_absent_definition_hash_stable_across_activation`).
        respond_to: record.respond_to.clone(),
        respond_to_allowlist: record.respond_to_allowlist.clone(),
        parallelism: record.parallelism,
    }
}

/// A persona's spawn-relevant config, pinned onto a `ManagedAgentRecord` at
/// create time. After the snapshot, spawn and deploy read these fields off the
/// record and never the live persona, so an agent stays pinned to the config
/// it was created with — restart reuses the snapshot, delete+respawn rewrites
/// it.
pub struct PersonaSnapshot {
    pub system_prompt: Option<String>,
    pub model: Option<String>,
    pub provider: Option<String>,
    /// Preferred ACP runtime ID, copied verbatim from the persona (including
    /// `None`). Unlike `model`/`provider`, there is no record-fallback: the
    /// materialized instance `runtime` must mirror the definition so that
    /// definition edits propagate on the next spawn rather than being silently
    /// shadowed by the stale materialized value.
    pub runtime: Option<String>,
    /// `persona_content_hash` of the persona at snapshot time; the drift basis.
    pub source_version: String,
}

/// Apply persona-wins-when-set precedence for a single optional string field.
///
/// Returns the persona's value when it is non-`None` and non-whitespace-only;
/// otherwise falls back to the record's value with the same blank filter applied.
/// Returns `None` only when both are blank — a genuinely unconfigured field stays
/// unconfigured.
///
/// This is the single source of truth for the precedence rule used by
/// `persona_snapshot_with_agent_config_fallback`, `build_deploy_payload`, and
/// `resolve_effective_prompt_model_provider` so the three paths cannot drift.
pub fn persona_field_with_record_fallback(
    persona_value: Option<&str>,
    record_value: Option<&str>,
) -> Option<String> {
    let non_blank = |v: Option<&str>| v.filter(|s| !s.trim().is_empty()).map(str::to_owned);
    non_blank(persona_value).or_else(|| non_blank(record_value))
}

/// Build the pinned snapshot for an agent created from `persona`.
///
/// The persona's `system_prompt` is always present, so it is wrapped in
/// `Some`. Env vars are deliberately absent: `record.env_vars` holds agent
/// overrides only, and the live persona env is merged underneath at read
/// time (spawn / readiness / deploy) — never snapshotted.
pub fn persona_snapshot(persona: &AgentDefinition) -> PersonaSnapshot {
    PersonaSnapshot {
        system_prompt: Some(persona.system_prompt.clone()),
        model: persona.model.clone(),
        provider: persona.provider.clone(),
        runtime: persona.runtime.clone(),
        source_version: persona_content_hash(&persona_event_content(persona)),
    }
}

/// Build the pinned snapshot for an **existing** agent record being re-snapshotted
/// from its linked persona (on spawn or app-launch restore).
///
/// Precedence rule: when the persona sets `model` or `provider` (non-`None`, non-empty),
/// the persona wins — this is the expected inheritance. When the persona leaves
/// these fields blank (`None` or empty string), the agent record's own values are
/// preserved instead. This prevents a persona with no configured model/provider from
/// clobbering a value the user already set on the agent, which would trap the agent
/// in a permanent "needs configuration" loop that users cannot escape.
///
/// `source_version` is always updated to the current persona content hash so the
/// drift badge clears correctly even when model/provider are not touched.
///
/// Env vars are not part of the snapshot: `record.env_vars` (agent overrides)
/// is left untouched and the live persona env is merged underneath at read time.
///
/// The two fields (`model`, `provider`) are independent: a persona that sets only
/// `model` wins on `model` while the agent's `provider` is preserved, and vice versa.
pub fn persona_snapshot_with_agent_config_fallback(
    persona: &AgentDefinition,
    current_agent_model: Option<&str>,
    current_agent_provider: Option<&str>,
) -> PersonaSnapshot {
    // Delegate system_prompt and source_version to persona_snapshot so future
    // PersonaSnapshot field additions stay automatically consistent.
    let base = persona_snapshot(persona);

    // Apply the shared precedence rule: persona wins when non-blank, else
    // the agent record's value is preserved so a configured agent stays configured.
    let model = persona_field_with_record_fallback(base.model.as_deref(), current_agent_model);
    let provider =
        persona_field_with_record_fallback(base.provider.as_deref(), current_agent_provider);

    PersonaSnapshot {
        model,
        provider,
        ..base
    }
}

/// Re-pin `record` to `persona`: build the snapshot (via
/// [`persona_snapshot_with_agent_config_fallback`], so blank persona
/// `model`/`provider` preserve the record's own values) and mirror it onto the
/// record — the definition quad (`system_prompt`/`model`/`provider`/`runtime`),
/// the env-override self-heal, and the `persona_source_version` drift basis.
///
/// This is the single apply used by every snapshot-apply site: the spawn
/// re-pin (`start_local_agent_with_preflight`), the launch backfill and
/// restore re-snapshot (`restore.rs`), and the prospective re-snapshot inside
/// `spawn_config_hash` — so a future `PersonaSnapshot` field addition
/// propagates to all of them at once.
///
/// Deliberately does NOT touch `updated_at`: persistence stamps are the
/// caller's concern, and `spawn_config_hash` (which applies this to a clone)
/// must stay pure.
pub fn apply_persona_snapshot(record: &mut ManagedAgentRecord, persona: &AgentDefinition) {
    let snapshot = persona_snapshot_with_agent_config_fallback(
        persona,
        record.model.as_deref(),    // fallback: record.model
        record.provider.as_deref(), // fallback: record.provider
    );
    if let Some(prompt) = snapshot.system_prompt {
        record.system_prompt = Some(prompt);
    }
    record.model = snapshot.model;
    record.provider = snapshot.provider;
    record.runtime = snapshot.runtime;
    // Drop a stale create-time harness pin when the definition names a
    // different known runtime; custom commands stay pinned.
    if let Some(def_runtime) = persona
        .runtime
        .as_deref()
        .map(str::trim)
        .filter(|r| !r.is_empty())
        .and_then(crate::managed_agents::known_acp_runtime_exact)
    {
        if let Some(pin_runtime) = record
            .agent_command_override
            .as_deref()
            .and_then(crate::managed_agents::known_acp_runtime)
        {
            if !std::ptr::eq(pin_runtime, def_runtime) {
                record.agent_command_override = None;
            }
        }
    }
    // env_vars stay overrides-only. Self-heal records written before the env
    // refresh: persona env used to be baked into `record.env_vars`, turning
    // inherited values into pseudo-overrides that shadow later persona edits.
    // An override equal to the persona's current value is indistinguishable
    // from inheritance, so drop it and let the live merge supply it.
    record
        .env_vars
        .retain(|k, v| persona.env_vars.get(k) != Some(v));
    record.persona_source_version = Some(snapshot.source_version);
}
#[cfg(test)]
mod tests;
