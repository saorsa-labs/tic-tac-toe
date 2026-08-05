#!/usr/bin/env node
import { readFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const REPO_ROOT = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "..",
);

const PACKAGING_FILES = [
  "desktop/src-tauri/build.rs",
  "desktop/src-tauri/tauri.conf.json",
  "desktop/src-tauri/capabilities/default.json",
  "desktop/src-tauri/src/lib.rs",
  "scripts/stage-sidecars.sh",
  "justfile",
];

const REACHABLE_NATIVE_ROOTS = [
  "desktop/src/main.tsx",
  "desktop/src/app/AppShell.tsx",
  "desktop/src/app/useAppShellLifecycleEffects.ts",
  "desktop/src/shared/api/nativeMessageAdapter.ts",
  "desktop/src/shared/api/tauriNativeX0x.ts",
  "desktop/src-tauri/src/local_stack.rs",
];

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
];

const FORBIDDEN_REACHABLE = [
  ["Nostr JavaScript dependency", /from\s+["']nostr-tools(?:\/[^"']*)?["']/],
  [
    "relay client import",
    /from\s+["'][^"']*(?:relayClient|readOnlyRelayClient|observerRelay)[^"']*["']/i,
  ],
  ["Nostr identity UI", /NostrBindConsentDialog|npub1|nsec1/],
  ["native compatibility websocket call", /plugin:websocket/],
  ["compatibility sidecar", /x0x-nostr-bridge/i],
  ["relay URL environment", /BUZZ_RELAY_(?:URL|HTTP)/],
];

export async function findNoRelayViolations(repoRoot = REPO_ROOT) {
  const violations = [];
  for (const relativePath of PACKAGING_FILES) {
    const source = await readFile(path.join(repoRoot, relativePath), "utf8");
    for (const [label, pattern] of FORBIDDEN_PACKAGING) {
      if (pattern.test(source)) violations.push(`${relativePath}: ${label}`);
    }
  }
  for (const relativePath of REACHABLE_NATIVE_ROOTS) {
    const source = await readFile(path.join(repoRoot, relativePath), "utf8");
    for (const [label, pattern] of FORBIDDEN_REACHABLE) {
      if (pattern.test(source)) violations.push(`${relativePath}: ${label}`);
    }
  }
  return violations;
}

if (
  process.argv[1] &&
  path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)
) {
  const violations = await findNoRelayViolations();
  if (violations.length > 0) {
    console.error(
      "No-relay invariant failed:\n" +
        violations.map((item) => `- ${item}`).join("\n"),
    );
    process.exitCode = 1;
  } else {
    console.log("No-relay invariant passed");
  }
}
