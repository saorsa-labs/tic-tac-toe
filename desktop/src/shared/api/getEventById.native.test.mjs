/**
 * Native getEventById acceptance (M3 durable-history cutover).
 *
 * `getEventById` was the last relay/Nostr throw-wrapper. It now resolves a
 * message by canonical msg_id through the daemon's indexed `x0x_history_get`
 * point lookup (no relay, no network, no payload/history scan) and maps the
 * `HistoryRow` to the `RelayEvent` the rendering layer expects — deriving the
 * channel id from the row's own scope (no scope hint needed: msg_id is
 * globally unique in one store).
 *
 * These tests exercise the REAL production seam (Tauri command name + the
 * HistoryRow→RelayEvent adapter + channelIdFromScope) by intercepting
 * `window.__TAURI_INTERNALS__.invoke`, with no browser or daemon.
 *
 * Runs via: node --import ./test-loader.mjs --experimental-strip-types --test
 */
import assert from "node:assert/strict";
import test from "node:test";

// ── Tauri internals shim ────────────────────────────────────────────────────
const calls = [];
let responseFor = null; // (cmd, args) => value | throws

globalThis.window = globalThis.window ?? {};
globalThis.window.__TAURI_INTERNALS__ = {
  invoke: async (cmd, args) => {
    calls.push({ cmd, args });
    if (responseFor) return responseFor(cmd, args);
    return null;
  },
  transformCallback: () => 0,
  unregisterCallback: () => {},
};

function setResponse(fn) {
  responseFor = fn;
  calls.length = 0;
}

const { getEventById } = await import("@/shared/api/tauri");
const { historyRowToRelayEvent } = await import(
  "@/shared/api/nativeMessageAdapter"
);

// ── Fixtures ────────────────────────────────────────────────────────────────
const MSG_ID = "aa".repeat(32);
const AGENT = "bb".repeat(64);
const CLIENT_ID = "11111111-1111-1111-1111-111111111111";

function envelopePayload(text = "hello world") {
  return btoa(
    JSON.stringify({ text, clientId: CLIENT_ID, createdAt: 1_700_000_000_000 }),
  );
}

// Raw snake_case row as x0xd emits it (history.rs row_json).
function rawRow(overrides = {}) {
  return {
    id: 1,
    msg_id: MSG_ID,
    scope: "group:team-1",
    author_agent: AGENT,
    author_machine: null,
    sent_at_ms: 1_700_000_000_000,
    seen_at_ms: 1_700_000_001_000,
    direction: "Inbound",
    content_type: "text/plain",
    payload: envelopePayload(),
    signed: true,
    provenance: "VerifiedEnvelope",
    replace_key: null,
    thread_root: null,
    thread_parent: null,
    ...overrides,
  };
}

// ── Found: native lookup → RelayEvent ───────────────────────────────────────

test("getEventById resolves via x0x_history_get and maps to a RelayEvent", async () => {
  setResponse((cmd, args) => {
    assert.equal(cmd, "x0x_history_get");
    assert.deepEqual(args, { msgId: MSG_ID });
    return rawRow();
  });

  const event = await getEventById(MSG_ID);

  // Canonical identity is the daemon msg_id.
  assert.equal(event.id, MSG_ID);
  assert.equal(event.pubkey, AGENT);
  assert.equal(event.content, "hello world");
  // The channel id is derived from the row's scope — no hint was passed.
  assert.deepEqual(event.tags[0], ["h", "team-1"]);
  // No relay command leaked onto the wire.
  assert.equal(calls.length, 1);
  assert.equal(calls[0].cmd, "x0x_history_get");
});

test("getEventById carries thread ancestry verbatim from the stored row", async () => {
  const root = "11".repeat(32);
  setResponse(() =>
    rawRow({ msg_id: MSG_ID, thread_root: root, thread_parent: root }),
  );
  const event = await getEventById(MSG_ID);
  const eTags = event.tags.filter((t) => t[0] === "e");
  assert.deepEqual(eTags, [
    ["e", root, "", "root"],
    ["e", root, "", "reply"],
  ]);
});

test("getEventById maps the row identically to historyRowToRelayEvent with the derived channel", async () => {
  setResponse(() => rawRow({ scope: "dm:peer-9" }));
  const event = await getEventById(MSG_ID);
  // Re-derive the channel id the same way the wrapper does (scope id segment).
  const direct = historyRowToRelayEvent(
    {
      id: 1,
      msgId: MSG_ID,
      scope: "dm:peer-9",
      authorAgent: AGENT,
      authorMachine: null,
      sentAtMs: 1_700_000_000_000,
      seenAtMs: 1_700_000_001_000,
      direction: "Inbound",
      contentType: "text/plain",
      payload: envelopePayload(),
      signed: true,
      provenance: "VerifiedEnvelope",
      replaceKey: null,
      threadRoot: null,
      threadParent: null,
    },
    "peer-9",
  );
  assert.deepEqual(event, direct);
});

// ── Not found: null → throw (distinct from transport error) ─────────────────

test("getEventById throws when the daemon reports not-found (null row)", async () => {
  setResponse(() => null);
  await assert.rejects(() => getEventById("00".repeat(32)), /not found/);
});

// ── Non-renderable row → throw (callers treat like a miss) ──────────────────

test("getEventById throws for a stored row that is not a renderable channel message", async () => {
  // A binary content type is not a timeline row — the adapter maps it to null,
  // and the wrapper surfaces that as a miss.
  setResponse(() =>
    rawRow({ content_type: "application/octet-stream", payload: "AAAA" }),
  );
  await assert.rejects(() => getEventById(MSG_ID), /not a renderable/);
});
