// Focused invariant tests for the hardened Rust scan in no-relay-gate.mjs.
//
// Each case feeds a synthetic production source through findNoRelayViolations
// against an isolated temp tree (mirroring desktop/src-tauri/src layout) and
// asserts the exact category the gate must assign — or, for the allowlist
// cases, that it stays silent. These lock the newly-caught relay/Nostr
// patterns so a regression (a production caller sneaking back in) turns the
// gate red instead of false-greens.
import assert from "node:assert/strict";
import { after, before, describe, it } from "node:test";
import { mkdtemp, mkdir, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";

import { findNoRelayViolations } from "./no-relay-gate.mjs";

// Fixtures live under a production path the gate treats as live code, unless
// the case is specifically exercising an allowlist (relay lib / test basename).
const SRC = "desktop/src-tauri/src";

const FIXTURES = [
  // ── relay transport — newly caught callers ──────────────────────────────
  [
    `${SRC}/commands/agent_relay_url.rs`,
    `use crate::app_state::AppState;
pub fn resolve(record_relay: &str, state: &AppState) -> String {
    crate::relay::effective_agent_relay_url(
        record_relay,
        &crate::relay::relay_ws_url_with_override(state),
    )
}
`,
  ],
  [
    `${SRC}/commands/profile_sync.rs`,
    `pub async fn sync(state: &AppState, relay_url: &str) -> Result<(), String> {
    crate::relay::sync_managed_agent_profile(state, relay_url, &keys, "name", None).await
}
`,
  ],
  [
    `${SRC}/commands/media_http.rs`,
    `use crate::relay::{classify_request_error, relay_api_base_url_with_override};
pub async fn upload(state: &AppState) -> Result<(), String> {
    let base = relay_api_base_url_with_override(state);
    let _ = base;
    Ok(())
}
`,
  ],
  [
    `${SRC}/commands/project_workflow.rs`,
    `use crate::relay::submit_signed_event_with_keys;
use nostr::{EventBuilder, Keys, Kind};
pub async fn ship(state: &AppState) -> Result<(), String> {
    let event = EventBuilder::new(Kind::TextNote, "hi").sign_with_keys(&Keys::generate());
    submit_signed_event_with_keys(state, &event).await
}
`,
  ],
  [
    `${SRC}/commands/nip98_auth.rs`,
    `pub fn auth(keys: &Keys, method: &Method, url: &str, body: &[u8]) -> Result<String, String> {
    crate::relay::build_nip98_auth_header_for_keys(keys, method, url, body)
}
`,
  ],
  [
    `${SRC}/commands/admission_caller.rs`,
    `pub async fn send(state: &AppState) -> Result<(), String> {
    crate::relay_admission::wait_for_rate_limit().await;
    Ok(())
}
`,
  ],
  // ── nostr identity/transport — the separate class ──────────────────────
  [
    `${SRC}/commands/nostr_identity.rs`,
    `use nostr::Keys;
pub fn compat_signer() -> Keys {
    Keys::generate()
}
`,
  ],
  [
    `${SRC}/commands/nostr_inbound_verify.rs`,
    `pub fn verify(event_json: &str) -> Result<(), String> {
    let event = nostr::Event::from_json(event_json)?;
    event.verify();
    Ok(())
}
`,
  ],

  // ── allowlist: must NOT fire ────────────────────────────────────────────
  // Native x0x daemon WebSocket — explicitly allowed (no relay/nostr).
  [
    `${SRC}/commands/native_x0x_live.rs`,
    `/// Open one daemon WebSocket and remain attached for live frames.
pub async fn x0x_open_live(scope: &str) -> Result<u64, String> {
    let ws = connect_daemon_websocket(scope).await?;
    Ok(42)
}
`,
  ],
  // Doc-comment / prose references only — stripped, not reachability.
  [
    `${SRC}/commands/doc_only.rs`,
    `//! Historical: crate::relay::query_relay and nostr::Keys signing were
//! removed during the M3 cutover. This module is now native x0x only.
/* block: build_nip98_auth_header was vendored in relay.rs */
pub fn native() {}
`,
  ],
  // Test basename — non-production.
  [
    `${SRC}/commands/excluded_tests.rs`,
    `crate::relay::effective_agent_relay_url("a", "b");
nostr::Keys::generate();
`,
  ],
  // Relay transport LIBRARY — defines the to-be-deleted fns; callers flagged.
  [
    `${SRC}/relay.rs`,
    `pub fn relay_ws_url() -> String { "ws://localhost:3000".into() }
pub fn build_nip98_auth_header_for_keys() {}
`,
  ],
  [
    `${SRC}/relay_admission.rs`,
    `pub async fn wait_for_rate_limit() {}
pub const MAX_HINT_SECONDS: u64 = 600;
`,
  ],
  [
    `${SRC}/relay/submit.rs`,
    `pub async fn submit_event_at_with_keys() {}
`,
  ],
  // ── manifest dependencies (packaging class) ────────────────────────────
  // package.json: nostr-tools (devDep) + plugin-websocket (dep) must flag;
  // the "check:nostr-identity-ui" SCRIPT name contains "nostr" but must NOT.
  [
    "desktop/package.json",
    `{
  "scripts": { "check:nostr-identity-ui": "node ./scripts/check-nostr-identity-ui.mjs" },
  "dependencies": { "@tauri-apps/plugin-websocket": "^2.0.0" },
  "devDependencies": { "nostr-tools": "^2.23.3" }
}
`,
  ],
  // Cargo.toml: nostr crate + tauri-plugin-websocket must flag; tokio-tungstenite
  // (the proven native x0x daemon transport) must NOT.
  [
    "desktop/src-tauri/Cargo.toml",
    `[dependencies]
tokio-tungstenite = { version = "0.29", features = ["rustls-tls-webpki-roots"] }
nostr = { version = "0.44", features = ["nip44"] }
tauri-plugin-websocket = "2.0"
# prose comment mentioning relay must not fire
`,
  ],
  // lib.rs: mod relay;/mod relay_admission; must flag (dormant-transport);
  // mod nostr_bind; is out of scope (not relay transport) and must NOT.
  [
    `${SRC}/lib.rs`,
    `mod nostr_bind;
mod relay;
mod relay_admission;
`,
  ],
];

const has = (violations, basename, category) =>
  violations.some(
    (v) => v.path.endsWith(`/commands/${basename}`) && v.category === category,
  );
const hasLib = (violations, rel) => violations.some((v) => v.path === rel);
const hasCat = (violations, relPath, category) =>
  violations.some((v) => v.path === relPath && v.category === category);
const hasDetail = (violations, category, detailSubstring) =>
  violations.some(
    (v) => v.category === category && v.detail.includes(detailSubstring),
  );

describe("no-relay-gate — hardened Rust scan", () => {
  let root;
  let violations;

  before(async () => {
    root = await mkdtemp(path.join(tmpdir(), "no-relay-gate-"));
    for (const [relPath, content] of FIXTURES) {
      const full = path.join(root, relPath);
      await mkdir(path.dirname(full), { recursive: true });
      await writeFile(full, content);
    }
    violations = await findNoRelayViolations(root);
  });

  after(async () => {
    if (root) await rm(root, { recursive: true, force: true });
  });

  it("returns structured { path, category, detail } violations", () => {
    assert.ok(Array.isArray(violations));
    for (const v of violations) {
      assert.equal(typeof v.path, "string");
      assert.equal(typeof v.category, "string");
      assert.equal(typeof v.detail, "string");
    }
  });

  // ── relay transport — every concrete caller is caught ──────────────────
  it("flags effective_agent_relay_url as relay transport", () => {
    assert.equal(
      has(violations, "agent_relay_url.rs", "relay"),
      true,
      "effective_agent_relay_url caller must be relay transport",
    );
  });

  it("flags managed-agent profile sync as relay transport", () => {
    assert.equal(has(violations, "profile_sync.rs", "relay"), true);
  });

  it("flags media HTTP base URL as relay transport", () => {
    assert.equal(has(violations, "media_http.rs", "relay"), true);
  });

  it("flags NIP-98 relay HTTP auth signing as relay transport", () => {
    assert.equal(has(violations, "nip98_auth.rs", "relay"), true);
  });

  it("flags relay_admission gate usage as relay transport", () => {
    assert.equal(has(violations, "admission_caller.rs", "relay"), true);
  });

  it("flags project workflow submit in BOTH relay and nostr classes", () => {
    // crate::relay::submit_signed_event_with_keys → relay; nostr::EventBuilder → nostr.
    assert.equal(has(violations, "project_workflow.rs", "relay"), true);
    assert.equal(has(violations, "project_workflow.rs", "nostr"), true);
  });

  // ── nostr identity/transport — separate class ──────────────────────────
  it("flags compat-signer nostr::Keys as nostr identity/transport", () => {
    assert.equal(has(violations, "nostr_identity.rs", "nostr"), true);
  });

  it("flags inbound nostr::Event verify as nostr identity/transport", () => {
    assert.equal(has(violations, "nostr_inbound_verify.rs", "nostr"), true);
  });

  // ── allowlist — native x0x WebSocket stays allowed ─────────────────────
  it("does NOT flag native x0x daemon WebSocket transport", () => {
    assert.equal(
      has(violations, "native_x0x_live.rs", "relay"),
      false,
      "native x0x WebSocket must stay allowed",
    );
    assert.equal(has(violations, "native_x0x_live.rs", "nostr"), false);
  });

  it("does NOT false-positive on doc-comment / prose references", () => {
    assert.equal(has(violations, "doc_only.rs", "relay"), false);
    assert.equal(has(violations, "doc_only.rs", "nostr"), false);
  });

  it("does NOT flag _tests.rs basenames (non-production)", () => {
    assert.equal(has(violations, "excluded_tests.rs", "relay"), false);
    assert.equal(has(violations, "excluded_tests.rs", "nostr"), false);
  });

  // The dormant library's INTERNALS are content-allowlisted (no relay/nostr
  // noise from the to-be-deleted definitions) — but its EXISTENCE is flagged
  // as dormant-transport: a clean cutover deletes the transport, not just callers.
  it("does NOT content-scan the dormant relay library internals", () => {
    assert.equal(hasCat(violations, `${SRC}/relay.rs`, "relay"), false);
    assert.equal(hasCat(violations, `${SRC}/relay.rs`, "nostr"), false);
    assert.equal(hasCat(violations, `${SRC}/relay_admission.rs`, "relay"), false);
    assert.equal(hasCat(violations, `${SRC}/relay/submit.rs`, "relay"), false);
  });

  it("flags retained relay.rs / relay_admission.rs / relay/ as dormant transport", () => {
    assert.equal(
      hasCat(violations, `${SRC}/relay.rs`, "dormant-transport"),
      true,
    );
    assert.equal(
      hasCat(violations, `${SRC}/relay_admission.rs`, "dormant-transport"),
      true,
    );
    assert.equal(
      hasCat(violations, `${SRC}/relay/`, "dormant-transport"),
      true,
    );
  });

  it("flags mod relay; / mod relay_admission; declarations as dormant transport", () => {
    assert.equal(
      hasDetail(violations, "dormant-transport", "mod relay;"),
      true,
    );
    assert.equal(
      hasDetail(violations, "dormant-transport", "mod relay_admission;"),
      true,
    );
    // mod nostr_bind; is a nostr module, not relay transport — out of scope here.
    assert.equal(
      hasDetail(violations, "dormant-transport", "mod nostr_bind;"),
      false,
    );
  });

  // ── manifest dependencies (packaging class) ───────────────────────────
  it("flags nostr-tools and plugin-websocket in package.json", () => {
    assert.equal(
      hasDetail(violations, "packaging", "nostr-tools JS dependency"),
      true,
    );
    assert.equal(
      hasDetail(violations, "packaging", "websocket plugin JS dependency"),
      true,
    );
    // The "check:nostr-identity-ui" SCRIPT name contains "nostr" but is not a dep.
    assert.equal(
      violations.some(
        (v) =>
          v.category === "packaging" && v.path === "desktop/package.json" &&
          !v.detail.includes("dependency"),
      ),
      false,
      "script names must not be mistaken for dependencies",
    );
  });

  it("flags nostr and tauri-plugin-websocket crates in Cargo.toml", () => {
    assert.equal(
      hasDetail(violations, "packaging", "nostr crate dependency"),
      true,
    );
    assert.equal(
      hasDetail(violations, "packaging", "tauri-plugin-websocket crate dependency"),
      true,
    );
  });

  it("does NOT flag tokio-tungstenite (proven native x0x transport)", () => {
    assert.equal(
      violations.some(
        (v) =>
          v.category === "packaging" &&
          /tungstenite/i.test(v.detail),
      ),
      false,
      "tokio-tungstenite is the native x0x daemon transport — must stay allowed",
    );
  });
});
