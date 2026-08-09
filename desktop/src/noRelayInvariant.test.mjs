import assert from "node:assert/strict";
import { test } from "node:test";

import { findNoRelayViolations } from "../../scripts/no-relay-gate.mjs";

// Static no-relay invariant for the packaged desktop app.
//
// The production graph must carry no relay/Nostr transport reachability (no
// relay URL resolution, event signing/auth, query/submit, or native websocket
// plugin) AND no transitional debt (the relayClient cutover stub or a removed
// relay-backed command still imported/invoked). This is the static counterpart
// to the native-smoke runtime deny-list.
//
// This is a STATIC reachability gate — it scans source, it does not boot x0xd.
// It proves the production code cannot reach a relay transport; it is not, on
// its own, evidence of real native-daemon behavior (see native-smoke.spec.ts
// for the mock-IPC runtime command-log side, and the verification note for why
// mock smoke is not daemon proof).
test("packaged native application has no relay reachability or relay stub debt", async () => {
  const violations = await findNoRelayViolations();
  const count = (category) =>
    violations.filter((v) => v.category === category).length;
  assert.equal(
    violations.length,
    0,
    `no-relay invariant failed: ${count("relay")} relay, ${count("nostr")} nostr, ${count("dormant-transport")} dormant-transport, ${count("packaging")} packaging, ${count("handler")} handler, ${count("frontend-transport")} frontend-transport, ${count("frontend-migration")} frontend-migration violation(s).
  Run \`node scripts/no-relay-gate.mjs\` for the full, labelled list.`,
  );
});
