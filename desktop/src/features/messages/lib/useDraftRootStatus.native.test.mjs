/**
 * Native draft-root resolution behaviour.
 *
 * The draft-root status lookup was relay `get_event`, which misfired for native
 * messages (identity is a BLAKE3 msgId, not a Nostr event id) and produced
 * false `deleted` flags. These tests pin the native replacement: a bounded
 * id-match history probe that never false-deletes and only reports `deleted`
 * when the probe window exhausts the scope.
 */
import assert from "node:assert/strict";
import test, { beforeEach } from "node:test";

const calls = [];
const pages = [];
globalThis.window = globalThis.window ?? {};
globalThis.window.__TAURI_INTERNALS__ = {
  invoke: async (cmd, args) => {
    calls.push({ cmd, args });
    if (cmd === "x0x_history_list") {
      return pages.shift() ?? { rows: [], has_more: false };
    }
    return null;
  },
  transformCallback: () => 1,
  unregisterCallback: () => {},
};

const { resolveNativeRootStatus } = await import(
  "@/features/messages/lib/useDraftRootStatus"
);

const { setResolvedHistoryScope, clearAllResolvedHistoryScopes } = await import(
  "@/features/messages/lib/nativeHistoryScopeStore.ts"
);

// The default channel ("group-1") has its daemon-resolved scope ready by
// default so the bounded-lookup tests exercise a resolvable group; tests that
// need an unresolved group use a distinct channel id. Each test starts from a
// clean registry + transport log.
beforeEach(() => {
  calls.length = 0;
  pages.length = 0;
  clearAllResolvedHistoryScopes();
  setResolvedHistoryScope("group-1", "group:group-1");
});

function channel(overrides = {}) {
  return {
    id: "group-1",
    name: "General",
    channelType: "stream",
    visibility: "private",
    description: "",
    topic: null,
    purpose: null,
    memberCount: 0,
    memberPubkeys: [],
    lastMessageAt: null,
    archivedAt: null,
    participants: [],
    participantPubkeys: [],
    isMember: true,
    ttlSeconds: null,
    ttlDeadline: null,
    ...overrides,
  };
}

// Raw snake_case wire shape (consumed by x0xHistoryList's fromRaw mappers).
function rawRow(msgId, overrides = {}) {
  return {
    id: 10,
    msg_id: msgId,
    scope: "group:group-1",
    author_agent: "b".repeat(64),
    author_machine: null,
    sent_at_ms: 1_700_000_000_000,
    seen_at_ms: 1_700_000_001_000,
    direction: "Inbound",
    content_type: "text/plain",
    payload: btoa(
      JSON.stringify({
        text: "x",
        clientId: "c1",
        createdAt: 1_700_000_000_000,
      }),
    ),
    signed: true,
    provenance: "VerifiedEnvelope",
    replace_key: null,
    thread_root: null,
    thread_parent: null,
    ...overrides,
  };
}

const ROOT = "1".repeat(64);

test("root present in the recent window resolves to available", async () => {
  pages.length = 0;
  pages.push({ rows: [rawRow(ROOT)], has_more: true });
  calls.length = 0;

  const status = await resolveNativeRootStatus(ROOT, channel());

  assert.equal(status, "available");
  assert.equal(calls[0].cmd, "x0x_history_list");
  assert.equal(calls[0].args.scope, "group:group-1");
});

test("root absent but history continues beyond the window stays optimistic (no false deleted)", async () => {
  pages.length = 0;
  pages.push({ rows: [rawRow("9".repeat(64))], has_more: true });
  const status = await resolveNativeRootStatus(ROOT, channel());
  assert.equal(status, "available");
});

test("root absent and window exhausted reports deleted", async () => {
  pages.length = 0;
  pages.push({ rows: [rawRow("9".repeat(64))], has_more: false });
  const status = await resolveNativeRootStatus(ROOT, channel());
  assert.equal(status, "deleted");
});

test("no channel (unknown scope) resolves optimistically without invoking", async () => {
  calls.length = 0;
  const status = await resolveNativeRootStatus(ROOT, null);
  assert.equal(status, "available");
  assert.equal(calls.length, 0);
});

test("unresolvable DM scope (ambiguous peer) resolves optimistically", async () => {
  calls.length = 0;
  const status = await resolveNativeRootStatus(
    ROOT,
    channel({ id: "legacy-dm", channelType: "dm" }),
  );
  assert.equal(status, "available");
  assert.equal(calls.length, 0);
});

test("lookup is an id match, never a payload scan: only one bounded page is fetched", async () => {
  pages.length = 0;
  pages.push({ rows: [rawRow(ROOT)], has_more: false });
  calls.length = 0;
  await resolveNativeRootStatus(ROOT, channel());
  assert.equal(calls.length, 1, "a single bounded page is requested");
});

// ── Resolved durable-history scope contract ─────────────────────────────────
// A thread-draft root can only be verified against a RESOLVED group scope. An
// unresolved group fails closed (throws) — the reply affordance is held in the
// pending state, never optimistically enabled against unverified history. The
// lookup re-evaluates the moment the daemon-resolved scope arrives.

test("an unresolved group throws instead of optimistically claiming the root is available", async () => {
  calls.length = 0;
  await assert.rejects(
    resolveNativeRootStatus(ROOT, channel({ id: "group-unresolved" })),
    /is not resolved/,
  );
  assert.equal(
    calls.length,
    0,
    "no history lookup while the group scope is unresolved",
  );
});

test("a resolved group probes its daemon-resolved stable scope, which may differ from group:<id>", async () => {
  setResolvedHistoryScope("group-1", "group:stable-g1");
  pages.length = 0;
  pages.push({ rows: [rawRow(ROOT)], has_more: true });
  calls.length = 0;
  const status = await resolveNativeRootStatus(ROOT, channel());
  assert.equal(status, "available");
  assert.equal(calls[0].args.scope, "group:stable-g1");
});

test("scope arrival re-enables the lookup: unresolved throws, then resolves once the stable scope lands", async () => {
  const ch = channel({ id: "group-arrives" });
  // Before the live subscription surfaces the stable scope: no query, throws.
  await assert.rejects(resolveNativeRootStatus(ROOT, ch), /is not resolved/);
  // The daemon-resolved stable scope arrives via the live subscription path.
  setResolvedHistoryScope("group-arrives", "group:stable-arrives");
  pages.length = 0;
  pages.push({ rows: [rawRow(ROOT)], has_more: true });
  calls.length = 0;
  const status = await resolveNativeRootStatus(ROOT, ch);
  assert.equal(status, "available");
  assert.equal(calls[0].args.scope, "group:stable-arrives");
});
