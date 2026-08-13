/**
 * M3 native data acceptance — durable-history seam (ADR-0023).
 *
 * Covers product acceptance test #2 (history survives): the cold workspace
 * backfill → live no-gap contract depends on an EXPLICIT, server-derived
 * keyset cursor and verbatim thread-root/parent ancestry. These controls prove
 * the frozen `tauriNativeX0x` seam — the single TS binding the desktop migrates
 * onto as it leaves the relay/Nostr dialect — preserves both, against the
 * native x0xd wire shape (source: ../x0x-tictactoe-threads history.rs `row_json`,
 * store.rs `HistoryQuery`, record.rs `HistoryRecord`).
 *
 * Replaces the relay-dialect paging/threading assertions (kind-39006 bounds
 * `has_more`/`next_cursor`, NIP-10 `e`/`reply` tags) with native frames and an
 * explicit `before_id` cursor. No relay kinds/tags appear here.
 *
 * Runs via: node --import ./test-loader.mjs --experimental-strip-types --test
 */
import assert from "node:assert/strict";
import test from "node:test";

// ── Tauri internals shim ────────────────────────────────────────────────────
// `invoke` (core.js:201) and `transformCallback` (core.js:69) both route through
// `window.__TAURI_INTERNALS__`. Intercepting it exercises the REAL production
// seam (command name + Tauri camelCase args + daemon-row mapping) without a
// browser or daemon.
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

const {
  x0xHistoryGet,
  x0xHistoryList,
  x0xHistorySearch,
  parseX0xScope,
  x0xScope,
} = await import("@/shared/api/tauriNativeX0x");

// ── Fixtures ────────────────────────────────────────────────────────────────
// Raw snake_case row as x0xd emits it (history.rs row_json). `msg_id` is the
// BLAKE3 dedupe key (hex). Thread columns are the M3 native threading surface.
function rawRow(overrides = {}) {
  return {
    id: 1,
    msg_id: "aa".repeat(32),
    scope: "topic:dev",
    author_agent: "bb".repeat(32),
    author_machine: "cc".repeat(32),
    sent_at_ms: 1_000,
    seen_at_ms: 1_100,
    direction: "Inbound",
    content_type: "text/plain",
    payload: "aGVsbG8=", // base64 "hello"
    signed: true,
    provenance: "VerifiedEnvelope",
    replace_key: null,
    thread_root: null,
    thread_parent: null,
    ...overrides,
  };
}

// ── Scope ───────────────────────────────────────────────────────────────────

test("parseX0xScope round-trips dm/group/topic and rejects bad kinds", () => {
  for (const [kind, id] of [
    ["dm", "dd".repeat(32)],
    ["group", "stable-group-id"],
    ["topic", "dev"],
  ]) {
    const scope = x0xScope(kind, id);
    assert.deepEqual(parseX0xScope(scope), { kind, id });
  }
  assert.throws(() => parseX0xScope("dm"), /invalid x0x scope/);
  assert.throws(() => parseX0xScope("relay:general"), /invalid x0x scope kind/);
  // `topic` is the native live-scope; the legacy relay channel id never appears.
  assert.throws(
    () => parseX0xScope("channel:9f28288a"),
    /invalid x0x scope kind/,
  );
});

// ── History list: wire contract + cursor derivation (AT#2) ───────────────────

test("x0x_history_list sends Tauri camelCase args and maps daemon rows", async () => {
  setResponse(() => ({ rows: [rawRow({ id: 7 })], has_more: false }));
  const page = await x0xHistoryList({ scope: "topic:dev", limit: 50 });

  assert.equal(calls[0].cmd, "x0x_history_list");
  assert.deepEqual(calls[0].args, {
    scope: "topic:dev",
    sinceMs: null,
    untilMs: null,
    limit: 50,
    beforeId: null,
  });
  // No relay vocabulary leaks onto the wire.
  assert.equal(calls[0].args.kinds, undefined);
  assert.equal(calls[0].args.until, undefined);

  const row = page.rows[0];
  assert.equal(row.msgId, "aa".repeat(32));
  assert.equal(row.authorAgent, "bb".repeat(32));
  assert.equal(row.seenAtMs, 1_100);
  assert.equal(row.contentType, "text/plain");
  assert.equal(row.provenance, "VerifiedEnvelope");
});

test("explicit cursor: nextCursor.beforeId is the last row's id (oldest of the page)", async () => {
  // newest-first page; the keyset cursor is the OLDEST retained row's rowid,
  // so the next (older) page starts strictly below it.
  setResponse(() => ({
    rows: [rawRow({ id: 30 }), rawRow({ id: 21 }), rawRow({ id: 12 })],
    has_more: true,
  }));
  const page = await x0xHistoryList({ scope: "topic:dev" });
  assert.equal(page.hasMore, true);
  assert.deepEqual(page.nextCursor, { beforeId: 12 });
});

test("cursor is null when the page is exhausted (has_more false)", async () => {
  setResponse(() => ({
    rows: [rawRow({ id: 2 }), rawRow({ id: 1 })],
    has_more: false,
  }));
  const page = await x0xHistoryList({ scope: "topic:dev" });
  assert.equal(page.hasMore, false);
  assert.equal(page.nextCursor, null);
});

test("cursor is null for an empty page even if has_more is true", async () => {
  // Defensive: a gap-closing read that returns zero rows must not synthesize a
  // cursor from a missing last row — that would silently strand the timeline.
  setResponse(() => ({ rows: [], has_more: true }));
  const page = await x0xHistoryList({ scope: "topic:dev" });
  assert.equal(page.nextCursor, null);
});

test("keyset paging threads the previous cursor forward with no gap", async () => {
  // Page 1 ends at id 12 → page 2 must request beforeId=12 and resume at 11.
  setResponse(() => ({ rows: [rawRow({ id: 12 })], has_more: true }));
  const first = await x0xHistoryList({ scope: "topic:dev" });
  const firstArgs = calls[0].args;
  setResponse(() => ({ rows: [rawRow({ id: 11 })], has_more: false }));
  const second = await x0xHistoryList({
    scope: "topic:dev",
    beforeId: first.nextCursor.beforeId,
  });
  assert.equal(firstArgs.beforeId, null);
  assert.equal(calls[0].args.beforeId, 12);
  assert.equal(second.rows[0].id, 11);
});

// ── Thread root/parent preservation (AT#2) ───────────────────────────────────

test("thread ancestry is preserved verbatim: root is self-referential, reply carries root+parent", async () => {
  const rootMsg = "11".repeat(32);
  const replyMsg = "22".repeat(32);
  setResponse(() => ({
    rows: [
      // A thread root: thread_root === own msg_id, thread_parent === null.
      rawRow({
        id: 1,
        msg_id: rootMsg,
        thread_root: rootMsg,
        thread_parent: null,
      }),
      // A reply in that thread: root is the ancestor, parent the direct parent.
      rawRow({
        id: 2,
        msg_id: replyMsg,
        thread_root: rootMsg,
        thread_parent: rootMsg,
      }),
      // A legacy / unthreaded row: both null ⟺ no threading metadata.
      rawRow({
        id: 3,
        msg_id: "33".repeat(32),
        thread_root: null,
        thread_parent: null,
      }),
    ],
    has_more: false,
  }));
  const page = await x0xHistoryList({ scope: "group:team" });

  const [root, reply, legacy] = page.rows;
  assert.equal(
    root.threadRoot,
    rootMsg,
    "root carries its own msgId as threadRoot",
  );
  assert.equal(root.threadParent, null, "root has no parent");
  assert.equal(reply.threadRoot, rootMsg, "reply preserves the thread root");
  assert.equal(
    reply.threadParent,
    rootMsg,
    "reply preserves the direct parent",
  );
  assert.equal(legacy.threadRoot, null);
  assert.equal(legacy.threadParent, null);
  // The whole thread is reachable by a single equality predicate on threadRoot.
  const thread = page.rows.filter((r) => r.threadRoot === rootMsg);
  assert.deepEqual(
    thread.map((r) => r.msgId),
    [rootMsg, replyMsg],
  );
});

// ── Search (AT#2: restart + FTS hit navigates back) ──────────────────────────

test("x0x_history_search sends the FTS needle + scope and maps the same row shape", async () => {
  const needle = "kickoff-2026";
  setResponse((_cmd, args) => {
    assert.equal(args.q, needle);
    return {
      rows: [rawRow({ id: 17, payload: "a2ljay1vZmY=" })],
      has_more: false,
    };
  });
  const page = await x0xHistorySearch({ scope: "topic:dev", q: needle });
  assert.equal(calls[0].cmd, "x0x_history_search");
  assert.equal(calls[0].args.q, needle);
  assert.equal(calls[0].args.scope, "topic:dev");
  // The hit carries its rowid + scope so navigation can deep-link back into
  // the durable timeline (the AT#2 "find message #17 after restart" path).
  assert.equal(page.rows[0].id, 17);
  assert.equal(page.rows[0].scope, "topic:dev");
});

// ── Single-row canonical lookup (x0x_history_get) ───────────────────────────

test("x0x_history_get sends the canonical msgId and maps the daemon row", async () => {
  const msgId = "aa".repeat(32);
  setResponse((cmd, args) => {
    assert.equal(cmd, "x0x_history_get");
    assert.deepEqual(args, { msgId });
    return rawRow({ id: 42, msg_id: msgId, scope: "group:team-1" });
  });
  const row = await x0xHistoryGet(msgId);
  assert.equal(row.id, 42);
  assert.equal(row.msgId, msgId);
  assert.equal(row.scope, "group:team-1");
});

test("x0x_history_get forwards group scope for canonical point lookup", async () => {
  const msgId = "aa".repeat(32);
  const scope = `group:${"cc".repeat(32)}`;
  setResponse((cmd, args) => {
    assert.equal(cmd, "x0x_history_get");
    assert.deepEqual(args, { msgId, scope });
    return rawRow({ id: 42, msg_id: msgId, scope });
  });
  const row = await x0xHistoryGet(msgId, scope);
  assert.equal(row.scope, scope);
});

test("x0x_history_get returns null when the daemon reports not-found (404)", async () => {
  // The client maps a 404 to null BEFORE it reaches TS; the wire therefore
  // delivers null, which the adapter surfaces as null — distinct from an
  // error (reject).
  setResponse(() => null);
  const row = await x0xHistoryGet("00".repeat(32));
  assert.equal(row, null);
});

test("x0x_history_get maps every daemon column to the camelCase seam", async () => {
  const root = "11".repeat(32);
  setResponse(() =>
    rawRow({
      msg_id: root,
      thread_root: root,
      thread_parent: null,
      content_type: "text/markdown",
      direction: "Outbound",
    }),
  );
  const row = await x0xHistoryGet(root);
  assert.equal(row.msgId, root);
  assert.equal(row.threadRoot, root, "self-referential root preserved");
  assert.equal(row.threadParent, null);
  assert.equal(row.contentType, "text/markdown");
  assert.equal(row.direction, "Outbound");
});
