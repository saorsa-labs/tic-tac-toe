#!/usr/bin/env node
// No-relay invariant gate for the packaged desktop app.
//
// Enforces the M3 cutover contract: the packaged production graph has no
// relay/Nostr transport reachability — no relay URL resolution, no event
// signing/auth, no relay query/submit/admission, no native websocket plugin,
// and no Nostr identity dependence. The gate scans the COMPLETE production
// frontier (every frontend module under desktop/src and every Tauri source
// under desktop/src-tauri/src), not a handful of root files, so it goes red
// rather than false-greens on transitive relay/Nostr use (e.g. a hook mounted
// by AppShell that imports the relay client, or a commands/*.rs that resolves
// a relay URL via crate::relay).
//
// Reported violations fall into labelled classes:
//   • relay transport         — a live relay code path on the Rust wire
//     (crate::relay API, relay_admission gate, BUZZ_RELAY URL/env, relay
//     query/submit, NIP-98 HTTP auth, native websocket plugin).
//   • nostr identity/transport — production dependence on the external `nostr`
//     crate (Keys/Event/EventBuilder signing, NIP auth). Reported SEPARATELY
//     from relay transport so the two cutovers can be tracked independently;
//     both classes must reach zero before release.
//   • frontend transport / migration debt — relay transport reachability and
//     the relayClient cutover stub still mounted by production frontend code.
//   • dormant transport       — the relay transport LIBRARY itself (relay.rs,
//     relay/, relay_admission.rs) plus its mod declarations and the relay/nostr
//     manifest dependencies. A clean cutover deletes the dormant transport, not
//     merely its callers.
import { access, readFile, readdir } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const REPO_ROOT = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "..",
);

// ── Packaging / build config ────────────────────────────────────────────────

const PACKAGING_FILES = [
  "desktop/package.json",
  "desktop/src-tauri/Cargo.toml",
  "desktop/src-tauri/build.rs",
  "desktop/src-tauri/tauri.conf.json",
  "desktop/src-tauri/capabilities/default.json",
  "desktop/src-tauri/src/lib.rs",
  "scripts/stage-sidecars.sh",
  "justfile",
];

// Relay/Nostr manifest dependencies. Native x0x uses its own Rust transport
// (x0x_client.rs over tokio-tungstenite) — it does NOT need nostr-tools, the
// tauri websocket plugin, the `nostr` crate, or tauri-plugin-websocket, so
// these remaining declarations are cutover debt. tokio-tungstenite is
// intentionally NOT forbidden: it is the proven native-x0x daemon transport.
// Patterns target each manifest's declaration syntax (JSON key / TOML crate =
// line) so prose mentions in comments or script names do not false-fire.
const FORBIDDEN_PACKAGING = [
  ["compatibility sidecar", /x0x-nostr-bridge/i],
  ["relay URL environment", /BUZZ_RELAY_(?:URL|HTTP)/],
  [
    "relay reconnect build setting",
    /BUZZ_BUILD_(?:RELAY_RECONNECT_CMD|AUTO_CONNECT_DEFAULT_RELAY)/,
  ],
  [
    "native compatibility websocket permission",
    /websocket:default|plugin\(\s*["']websocket["']/,
  ],
  ["Nostr git credential sidecar", /git-credential-nostr/i],
  ["nostr-tools JS dependency", /"nostr-tools"\s*:/],
  [
    "tauri websocket plugin JS dependency",
    /"(@tauri-apps\/)?plugin-websocket"\s*:/,
  ],
  ["nostr crate dependency", /^\s*nostr\s*=/m],
  ["tauri-plugin-websocket crate dependency", /^\s*tauri-plugin-websocket\s*=/m],
];

// ── Tauri invoke_handler: removed relay commands must stay unregistered ─────

const FORBIDDEN_HANDLER_COMMANDS = [
  "get_default_relay_url",
  "get_relay_ws_url",
  "get_relay_http_url",
  "sign_event",
  "create_auth_event",
  "get_channels",
  "send_channel_message",
  "get_channel_window",
  "apply_workspace",
];

// ── Rust production sources ─────────────────────────────────────────────────

// Relay URL injection / defaults. (lib.rs is skipped here — it is
// packaging-checked above — to avoid duplicate reports.)
const RUST_ENV_PATTERNS = [
  ["BUZZ_RELAY_URL env injection", /\.env\(\s*["']BUZZ_RELAY_URL["']/],
  ["BUZZ_RELAY_HTTP env injection", /\.env\(\s*["']BUZZ_RELAY_HTTP["']/],
  ["BUZZ_RELAY_URL env read", /env::var\(\s*["']BUZZ_RELAY_URL["']/],
  ["BUZZ_RELAY_HTTP env read", /env::var\(\s*["']BUZZ_RELAY_HTTP["']/],
  [
    "BUZZ_RELAY_URL build-time read",
    /option_env!\(\s*["']BUZZ_DESKTOP_BUILD_RELAY_URL["']/,
  ],
  [
    "BUZZ_RELAY_HTTP build-time read",
    /option_env!\(\s*["']BUZZ_DESKTOP_BUILD_RELAY_HTTP["']/,
  ],
  ["hardcoded relay default URL", /ws:\/\/localhost:3000/],
];

// Relay transport reachability. The `crate::relay` catch-all is the primary
// net — it flags EVERY relay-module API caller (URL resolution such as
// effective_agent_relay_url / relay_ws_url_with_override / relay_http_base_url
// / relay_api_base_url_with_override, profile sync, media HTTP base, project
// submit, the NIP-98 header builder, error helpers). The named patterns
// document the high-signal transport entry points and stay defensive against
// re-exports that bypass the `crate::relay::` prefix. A file is reported once
// for this class regardless of how many patterns it trips.
const RUST_RELAY_CALL_PATTERNS = [
  ["crate::relay module reference", /\bcrate::relay\b/],
  ["relay_admission gate reference", /\bcrate::relay_admission\b/],
  ["relay query transport call", /\bquery_relay(?:_at(?:_with_keys)?)?\s*\(/],
  [
    "relay event submission call",
    /\bsubmit_(?:signed_)?event(?:_at(?:_with_keys)?|_with_keys)?\s*\(/,
  ],
  ["NIP-98 relay HTTP auth signing", /\bbuild_nip98_auth_header/],
  ["native websocket plugin call", /plugin:websocket/],
];

// Nostr identity/transport dependence — the external `nostr` crate (Keys,
// Event/EventBuilder signing, PublicKey, Timestamp, JsonUtil, nips::nip44,
// RelayUrl, AUTH). Reported in its OWN class so the Nostr cutover is visible
// apart from the relay transport cutover. NIP-98 HTTP auth lives in the relay
// class above because production callers reach it via
// crate::relay::build_nip98_auth_header.
const RUST_NOSTR_PATTERNS = [
  ["nostr crate identity/transport", /\buse\s+nostr\b|\bnostr::/],
];

// Allowlist — the dormant relay/Nostr transport LIBRARY that DEFINES the fns
// being removed wholesale. The scan flags production CALLERS, not these
// definitions: flagging their internals is pure noise (they cannot compile
// once any caller is cut over, and the contract deletes them outright).
//  • relay.rs (module root) + relay/ submodules (e.g. relay::submit)
//  • relay_admission.rs — the 429 admission gate library
//  • *_tests.rs / *_test.rs / test_*.rs / tests.rs — non-production
const RUST_ALLOW_BASENAMES = new Set(["tests.rs"]);
const isRelayLibFile = (relPath) =>
  relPath === "desktop/src-tauri/src/relay.rs" ||
  relPath === "desktop/src-tauri/src/relay_admission.rs" ||
  relPath.startsWith("desktop/src-tauri/src/relay/");
const isRustTestFile = (file) =>
  file.endsWith("_tests.rs") ||
  file.endsWith("_test.rs") ||
  file.startsWith("test_");

// ── Frontend production sources ─────────────────────────────────────────────

// Hard invariant: relay transport reachability (websocket / signing / URL /
// connect-capable session). Must be zero in a released build.
const FRONTEND_HARD_PATTERNS = [
  [
    "native websocket transport call",
    /plugin:websocket\|(?:connect|send)\b/,
  ],
  [
    "relay session value import (connect-capable module)",
    /import\s+(?!type\b)[^;]*\bfrom\s+["'][^"']*relayClientSession["']/,
  ],
  ["Nostr JavaScript dependency", /from\s+["']nostr-tools(?:\/[^"']*)?["']/],
  ["removed relay signing command call", /\bsignRelayEvent\s*\(/],
  ["removed relay auth-event command call", /\bcreateAuthEvent\s*\(/],
  [
    "removed relay URL command call",
    /\b(?:getRelayWsUrl|getDefaultRelayUrl|getRelayHttpUrl)\s*\(/,
  ],
];

// Migration debt: the relayClient cutover stub or a removed relay-backed
// command is still mounted by production code. The native projection must
// replace these before release.
const FRONTEND_MIGRATION_PATTERNS = [
  [
    "relayClient stub value import",
    /import\s+(?!type\b)[^;]*\bfrom\s+["'][^"']*\/relayClient["']/,
  ],
  [
    "removed relay-backed command invoke",
    /invokeTauri(?:<[^>]*>)?\(\s*["'](?:get_event|get_channels|send_channel_message|get_channel_window|apply_workspace)["']/,
  ],
];

// Allowlist (narrow, archived/test only):
//  • relayClientSession.ts — the archived connect-capable module. Its
//    unreachability is independently enforced by the session-import check
//    above, so its internal transport calls are dead code, not a violation.
//  • desktop/src/testing/ and desktop/tests/ — E2E mock-bridge infrastructure
//    that simulates the IPC boundary; not production code.
//  • *.test.* / *.spec.* — unit/E2E tests.
const FRONTEND_ALLOW_REL = new Set([
  "desktop/src/shared/api/relayClientSession.ts",
]);
const isFrontendTestFile = (file) =>
  file.endsWith(".test.mjs") ||
  file.endsWith(".test.ts") ||
  file.endsWith(".test.tsx") ||
  file.endsWith(".spec.ts") ||
  file.endsWith(".spec.tsx");

// ── Helpers ─────────────────────────────────────────────────────────────────

async function listFiles(dir, exts, ignore = new Set()) {
  const entries = await readdir(dir, { withFileTypes: true });
  const results = [];
  for (const entry of entries) {
    const full = path.join(dir, entry.name);
    if (ignore.has(full)) continue;
    if (entry.isDirectory()) {
      results.push(...(await listFiles(full, exts, ignore)));
    } else if (exts.some((ext) => entry.name.endsWith(ext))) {
      results.push(full);
    }
  }
  return results;
}

async function directoryExists(dir) {
  try {
    await readdir(dir);
    return true;
  } catch {
    return false;
  }
}
async function fileExists(file) {
  try {
    await access(file);
    return true;
  } catch {
    return false;
  }
}

// Strip Rust line (`//`, `//!`) and block (`/* */`) comments so doc-comment
// references to transport fns (e.g. events.rs documenting `submit_event()`)
// are not mistaken for production reachability.
function stripRustComments(src) {
  return src
    .replace(/\/\*[\s\S]*?\*\//g, "")
    .replace(/^\s*\/\/.*$/gm, "");
}

function rel(file, root = REPO_ROOT) {
  return path.relative(root, file);
}

function inFrontendAllow(file, root = REPO_ROOT) {
  const r = rel(file, root);
  if (FRONTEND_ALLOW_REL.has(r)) return true;
  return (
    r.startsWith("desktop/src/testing/") || r.startsWith("desktop/tests/")
  );
}

function escapeRegex(s) {
  return s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

// ── Core scan ───────────────────────────────────────────────────────────────

// Each violation: { path, category, detail }.
//   category ∈ packaging | handler | relay | nostr | dormant-transport |
//              frontend-transport | frontend-migration
// A production file is reported AT MOST once per category (first matching
// pattern wins) so the report stays one-line-per-offending-source.
export async function findNoRelayViolations(repoRoot = REPO_ROOT) {
  const violations = [];
  const relp = (file) => rel(file, repoRoot);

  // 1. Packaging / build config.
  for (const relativePath of PACKAGING_FILES) {
    let source;
    try {
      source = await readFile(path.join(repoRoot, relativePath), "utf8");
    } catch {
      continue; // absent packaging file — nothing to check
    }
    for (const [label, pattern] of FORBIDDEN_PACKAGING) {
      if (pattern.test(source)) {
        violations.push({
          path: relativePath,
          category: "packaging",
          detail: label,
        });
      }
    }
  }

  // 2. Tauri invoke_handler — removed relay commands must not be re-registered.
  try {
    const libRs = await readFile(
      path.join(repoRoot, "desktop/src-tauri/src/lib.rs"),
      "utf8",
    );
    const handlerMatch = libRs.match(/generate_handler!\[([\s\S]*?)\n\s*\]/);
    const handlerBlock = handlerMatch ? handlerMatch[1] : "";
    for (const cmd of FORBIDDEN_HANDLER_COMMANDS) {
      const re = new RegExp(`^\\s*${escapeRegex(cmd)}\\s*,?\\s*$`, "m");
      if (re.test(handlerBlock)) {
        violations.push({
          path: "desktop/src-tauri/src/lib.rs",
          category: "handler",
          detail: `removed relay command '${cmd}' still registered in invoke_handler`,
        });
      }
    }
    // mod declarations for the dormant relay transport library — must be
    // removed alongside the modules themselves (checked in step 5).
    const libRsCode = stripRustComments(libRs);
    for (const mod of ["relay", "relay_admission"]) {
      if (new RegExp(`^\\s*(?:pub\\s+)?mod\\s+${mod}\\s*;`, "m").test(libRsCode)) {
        violations.push({
          path: "desktop/src-tauri/src/lib.rs",
          category: "dormant-transport",
          detail: `mod ${mod}; declaration still present — delete on cutover`,
        });
      }
    }
  } catch {
    // lib.rs absent — skip the handler check.
  }

  // 3. Rust production sources — relay transport + Nostr identity/transport.
  const rustSrcDir = path.join(repoRoot, "desktop/src-tauri/src");
  if (await directoryExists(rustSrcDir)) {
    const rustFiles = await listFiles(rustSrcDir, [".rs"]);
    for (const file of rustFiles) {
      const r = relp(file);
      const basename = path.basename(file);
      if (
        isRelayLibFile(r) ||
        RUST_ALLOW_BASENAMES.has(basename) ||
        isRustTestFile(basename)
      ) {
        continue;
      }
      const code = stripRustComments(await readFile(file, "utf8"));
      // lib.rs is packaging-checked for BUZZ_RELAY; scan it only for refs/calls.
      const isLib = r === "desktop/src-tauri/src/lib.rs";
      const relayPatterns = isLib
        ? RUST_RELAY_CALL_PATTERNS
        : [...RUST_ENV_PATTERNS, ...RUST_RELAY_CALL_PATTERNS];
      for (const { category, patterns } of [
        { category: "relay", patterns: relayPatterns },
        { category: "nostr", patterns: RUST_NOSTR_PATTERNS },
      ]) {
        const hit = patterns.find(([, pattern]) => pattern.test(code));
        if (hit) {
          violations.push({ path: r, category, detail: hit[0] });
        }
      }
    }
  }

  // 4. Frontend production sources — transport reachability + migration debt.
  const frontendSrcDir = path.join(repoRoot, "desktop/src");
  if (await directoryExists(frontendSrcDir)) {
    const frontendFiles = await listFiles(frontendSrcDir, [".ts", ".tsx"]);
    for (const file of frontendFiles) {
      if (isFrontendTestFile(file) || inFrontendAllow(file, repoRoot)) continue;
      const r = relp(file);
      const source = await readFile(file, "utf8");
      for (const { category, patterns } of [
        { category: "frontend-transport", patterns: FRONTEND_HARD_PATTERNS },
        { category: "frontend-migration", patterns: FRONTEND_MIGRATION_PATTERNS },
      ]) {
        const hit = patterns.find(([, pattern]) => pattern.test(source));
        if (hit) {
          violations.push({ path: r, category, detail: hit[0] });
        }
      }
    }
  }

  // 5. Dormant relay transport library — a clean cutover DELETES the transport,
  // not merely its callers. Flag retained modules + their directory. (Their
  // internals are content-allowlisted in step 3 to avoid noise; this step is
  // the delete-on-cutover invariant.)
  const dormantModules = [
    "desktop/src-tauri/src/relay.rs",
    "desktop/src-tauri/src/relay_admission.rs",
  ];
  for (const relPath of dormantModules) {
    if (await fileExists(path.join(repoRoot, relPath))) {
      violations.push({
        path: relPath,
        category: "dormant-transport",
        detail: "dormant relay transport module still present — delete on cutover",
      });
    }
  }
  if (await directoryExists(path.join(repoRoot, "desktop/src-tauri/src/relay"))) {
    violations.push({
      path: "desktop/src-tauri/src/relay/",
      category: "dormant-transport",
      detail: "dormant relay transport directory still present — delete on cutover",
    });
  }

  return violations;
}

// Display order + label for each violation class in the CLI report.
const CATEGORY_LABELS = {
  packaging: "packaging / build config",
  handler: "removed relay command still registered",
  relay: "relay transport reachability",
  nostr: "nostr identity/transport",
  "dormant-transport": "dormant relay transport (delete on cutover)",
  "frontend-transport": "frontend transport reachability",
  "frontend-migration": "frontend migration debt",
};
const CATEGORY_ORDER = [
  "packaging",
  "handler",
  "relay",
  "nostr",
  "dormant-transport",
  "frontend-transport",
  "frontend-migration",
];

if (
  process.argv[1] &&
  path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)
) {
  const violations = await findNoRelayViolations();
  if (violations.length > 0) {
    console.error("No-relay invariant failed:");
    for (const category of CATEGORY_ORDER) {
      const group = violations.filter((v) => v.category === category);
      if (group.length === 0) continue;
      console.error(`\n  ${CATEGORY_LABELS[category]} (${group.length}):`);
      for (const v of group) console.error(`  - ${v.path}: ${v.detail}`);
    }
    process.exitCode = 1;
  } else {
    console.log("No-relay invariant passed");
  }
}
