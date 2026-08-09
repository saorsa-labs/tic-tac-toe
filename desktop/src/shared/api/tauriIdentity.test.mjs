/**
 * M2 displayed-identity regressions for the frontend identity seam.
 *
 * The cutover split the boot surface into two commands:
 *   • `get_recovery_state` — ALWAYS succeeds, no daemon dependency, returns
 *     only { lost, locked, reset_failed }. Fetched FIRST; when any flag is
 *     true the UI routes to recovery and `get_identity` is never called.
 *   • `get_identity` — returns only { agent_id, identity_words } and stays
 *     STRICT: a malformed/absent AgentId or words fails hard rather than
 *     falling back to a partial identity.
 *
 * These tests pin both halves: the x0x AgentId is the ONLY identity — the
 * legacy Nostr relay signer was removed in the M3 cutover, so there is no
 * longer a separate signer namespace to carry. They mock
 * `window.__TAURI_INTERNALS__.invoke` (the transport `@tauri-apps/api` uses)
 * so the real `getIdentity`/`getRecoveryState` run end-to-end.
 */
import assert from "node:assert/strict";
import { afterEach, describe, it } from "node:test";

// ── Tauri IPC interceptor ───────────────────────────────────────────────────
// @tauri-apps/api/core reads window.__TAURI_INTERNALS__.invoke(cmd, args).
// Alias window→globalThis so the alias resolves, then route by command name.
globalThis.window = globalThis;

/** Records every command invoked, in order — proves which surface was hit. */
const invokedCommands = [];
const ipcHandlers = new Map();

globalThis.__TAURI_INTERNALS__ = {
  invoke: (cmd, args) => {
    invokedCommands.push(cmd);
    const handler = ipcHandlers.get(cmd);
    if (handler) return Promise.resolve(handler(args));
    return Promise.reject(new Error(`unmocked Tauri command: ${cmd}`));
  },
  transformCallback: () => 0,
};

function setIpc(cmd, fn) {
  ipcHandlers.set(cmd, fn);
}

afterEach(() => {
  ipcHandlers.clear();
  invokedCommands.length = 0;
});

// Production imports — the real seam under test.
const { getIdentity, getRecoveryState } = await import("./tauriIdentity.ts");

// ── Fixtures ────────────────────────────────────────────────────────────────

// Canonical x0x parity (BEN_AGENT_ID from the four-word-networking crate's
// network fixtures) and the words `x0x agent` actually prints for it.
const AGENT_ID =
  "dd6530452610619d468e4e82be82107e86384365c58efa6e3018d7762c7368da";
const PARITY_WORDS = ["bodily", "example", "dismiss", "galaxy"];

function rawIdentity(overrides = {}) {
  return {
    agent_id: AGENT_ID,
    identity_words: PARITY_WORDS,
    ...overrides,
  };
}

// ── get_identity: strict validation + canonical mapping ─────────────────────

describe("getIdentity: strict identity mapping", () => {
  it("maps a canonical raw payload to camelCase { agentId, identityWords }", async () => {
    setIpc("get_identity", () => rawIdentity());
    const identity = await getIdentity();

    assert.equal(identity.agentId, AGENT_ID);
    assert.deepEqual(identity.identityWords, PARITY_WORDS);
    // No recovery flags leak through the identity command (split contract).
    assert.ok(
      !("lost" in identity) &&
        !("locked" in identity) &&
        !("resetFailed" in identity),
      "identity must not carry recovery flags",
    );
  });

  it("rejects an uppercase (non-canonical) agent id — no silent folding", async () => {
    setIpc("get_identity", () =>
      rawIdentity({ agent_id: AGENT_ID.toUpperCase() }),
    );
    await assert.rejects(() => getIdentity(), /malformed agent id/);
  });

  it("rejects a wrong-length agent id", async () => {
    setIpc("get_identity", () =>
      rawIdentity({ agent_id: AGENT_ID.slice(0, 63) }),
    );
    await assert.rejects(() => getIdentity(), /malformed agent id/);
  });

  it("rejects a bech32/npub placeholder posing as an agent id", async () => {
    setIpc("get_identity", () =>
      rawIdentity({ agent_id: `npub1${"0".repeat(60)}` }),
    );
    await assert.rejects(() => getIdentity(), /malformed agent id/);
  });

  it("rejects missing identity words — never renders a wordless identity", async () => {
    setIpc("get_identity", () => rawIdentity({ identity_words: undefined }));
    await assert.rejects(() => getIdentity(), /missing identity words/);
  });

  it("rejects an empty identity-words array", async () => {
    setIpc("get_identity", () => rawIdentity({ identity_words: [] }));
    await assert.rejects(() => getIdentity(), /missing identity words/);
  });

  it("drops non-string and empty entries, fails when no word survives", async () => {
    // The filter keeps any string with length > 0 (it does NOT trim, so a
    // whitespace-only token survives) and drops everything else; if nothing
    // survives the identity fails closed rather than rendering wordlessly.
    setIpc("get_identity", () =>
      rawIdentity({
        identity_words: ["", "bodily", 123, "example", null, false],
      }),
    );
    const identity = await getIdentity();
    assert.deepEqual(identity.identityWords, ["bodily", "example"]);

    setIpc("get_identity", () => rawIdentity({ identity_words: ["", "", 9] }));
    await assert.rejects(() => getIdentity(), /missing identity words/);
  });
});

// ── get_recovery_state: daemon-independent, always succeeds ─────────────────

describe("getRecoveryState: daemon-independent recovery surface", () => {
  it("maps the snake_case raw to { lost, locked, resetFailed }", async () => {
    setIpc("get_recovery_state", () => ({
      lost: true,
      locked: false,
      reset_failed: true,
    }));
    const recovery = await getRecoveryState();
    assert.deepEqual(recovery, {
      lost: true,
      locked: false,
      resetFailed: true,
    });
  });

  it("coerces absent/non-boolean flags to false (defensive, never throws)", async () => {
    setIpc("get_recovery_state", () => ({}));
    const recovery = await getRecoveryState();
    assert.deepEqual(recovery, {
      lost: false,
      locked: false,
      resetFailed: false,
    });
  });

  it("hits the recovery command, never get_identity (daemon is skipped)", async () => {
    // The whole point of the split: recovery state comes from a daemon-free
    // command. Prove getRecoveryState does not touch the daemon-backed
    // get_identity surface even when recovery is active.
    setIpc("get_recovery_state", () => ({
      lost: true,
      locked: false,
      reset_failed: false,
    }));
    await getRecoveryState();
    assert.deepEqual(invokedCommands, ["get_recovery_state"]);
    assert.ok(
      !invokedCommands.includes("get_identity"),
      "recovery must not invoke the daemon-backed get_identity",
    );
  });
});
