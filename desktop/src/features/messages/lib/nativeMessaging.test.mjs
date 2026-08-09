import assert from "node:assert/strict";
import test, { beforeEach } from "node:test";

const calls = [];
let response = null;
globalThis.window = globalThis.window ?? {};
globalThis.window.__TAURI_INTERNALS__ = {
  invoke: async (cmd, args) => {
    calls.push({ cmd, args });
    return response;
  },
  transformCallback: () => 1,
  unregisterCallback: () => {},
};

const {
  NATIVE_DELETE_BLOCKER,
  NATIVE_EDIT_BLOCKER,
  NATIVE_REACTION_BLOCKER,
  fetchNativeChannelWindow,
  fetchNativeMessagesById,
  nativeHistoryPageToChannelWindowPage,
  nativeScopeForChannel,
  resolveNativeHistoryScope,
  searchNativeMessages,
  sendNativeMessage,
} = await import("./nativeMessaging.ts");
const {
  setResolvedHistoryScope,
  getResolvedHistoryScope,
  clearAllResolvedHistoryScopes,
} = await import("./nativeHistoryScopeStore.ts");

// Each test starts with a clean transport log and an empty scope registry so
// the resolved-scope precondition is explicit per test, never inherited.
beforeEach(() => {
  calls.length = 0;
  response = null;
  clearAllResolvedHistoryScopes();
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

function payload(text, clientId) {
  return btoa(JSON.stringify({ text, clientId, createdAt: 1_700_000_000_000 }));
}

function row(overrides = {}) {
  return {
    id: 10,
    msgId: "a".repeat(64),
    scope: "group:group-1",
    authorAgent: "b".repeat(64),
    authorMachine: null,
    sentAtMs: 1_700_000_000_000,
    seenAtMs: 1_700_000_001_000,
    direction: "Inbound",
    contentType: "text/plain",
    payload: payload("hello", "client-1"),
    signed: true,
    provenance: "VerifiedEnvelope",
    replaceKey: null,
    threadRoot: null,
    threadParent: null,
    ...overrides,
  };
}

/** Register a daemon-resolved durable scope for a group channel (default: the
 * transient group:<id>; pass a divergent scope to exercise the contract). */
function resolveGroup(channelId, scope = `group:${channelId}`) {
  setResolvedHistoryScope(channelId, scope);
}

test("native scopes map streams to groups and require an unambiguous DM AgentId", () => {
  assert.equal(nativeScopeForChannel(channel()), "group:group-1");
  const peer = "c".repeat(64);
  assert.equal(
    nativeScopeForChannel(
      channel({
        id: "legacy-dm",
        channelType: "dm",
        participantPubkeys: [peer],
      }),
    ),
    `dm:${peer}`,
  );
  assert.throws(
    () =>
      nativeScopeForChannel(channel({ id: "legacy-dm", channelType: "dm" })),
    /exactly one peer AgentId/,
  );
});

test("native history preserves msg_id ancestry and Buzz window shape", () => {
  const rootId = "1".repeat(64);
  const replyId = "2".repeat(64);
  const page = nativeHistoryPageToChannelWindowPage(
    {
      rows: [
        row({
          id: 20,
          msgId: rootId,
          threadRoot: rootId,
          payload: payload("root", "c-root"),
        }),
        row({
          id: 19,
          msgId: replyId,
          threadRoot: rootId,
          threadParent: rootId,
          payload: payload("reply", "c-reply"),
        }),
      ],
      hasMore: true,
      nextCursor: { beforeId: 19 },
    },
    channel(),
    null,
  );

  assert.equal(page.rows.length, 1, "replies stay out of the channel timeline");
  assert.equal(
    page.rows[0].event.id,
    rootId,
    "canonical msg_id is the UI event id",
  );
  assert.equal(page.rows[0].event.localKey, "c-root");
  assert.equal(page.rows[0].thread.replyCount, 1);
  assert.deepEqual(page.nextCursor, {
    createdAt: 1_700_000_001,
    eventId: replyId,
    beforeId: 19,
  });
});

test("native scoped search maps x0xd rows to the existing SearchHit result", async () => {
  calls.length = 0;
  response = {
    rows: [
      {
        id: 7,
        msg_id: "d".repeat(64),
        scope: "group:group-1",
        author_agent: "e".repeat(64),
        author_machine: null,
        sent_at_ms: 1_700_000_000_000,
        seen_at_ms: 1_700_000_002_000,
        direction: "Inbound",
        content_type: "text/plain",
        payload: payload("durable needle", "client-search"),
        signed: true,
        provenance: "VerifiedEnvelope",
        replace_key: null,
        thread_root: null,
        thread_parent: null,
      },
    ],
    has_more: false,
  };

  // The daemon resolved a stable scope that DIFFERS from the transient REST
  // id ("group-1"); search must target the daemon scope, not group:group-1.
  resolveGroup("group-1", "group:stable-search");
  const result = await searchNativeMessages("needle", [channel()], 12);
  assert.equal(calls[0].cmd, "x0x_history_search");
  assert.equal(calls[0].args.scope, "group:stable-search");
  assert.equal(result.hits[0].eventId, "d".repeat(64));
  assert.equal(result.hits[0].content, "durable needle");
  assert.equal(result.hits[0].channelId, "group-1");
});

test("unsupported native mutations expose exact blockers instead of relay fallback", () => {
  for (const blocker of [
    NATIVE_EDIT_BLOCKER,
    NATIVE_DELETE_BLOCKER,
    NATIVE_REACTION_BLOCKER,
  ]) {
    assert.match(blocker, /x0xd/);
  }
});

// ── Resolved durable-history scope contract ─────────────────────────────────
// Groups only query once the daemon-resolved stable scope arrives; an
// unresolved group never queries (and never falls back to the transient REST
// id). DMs resolve deterministically and never read the registry.

test("resolveNativeHistoryScope: a group returns the daemon-resolved scope, which may diverge from group:<id>, or null while unresolved", () => {
  // No registry entry → unresolved (consumers must hold / skip / throw).
  assert.equal(
    resolveNativeHistoryScope(channel({ id: "g-unresolved" })),
    null,
  );
  // A divergent stable id is returned verbatim, never normalized.
  resolveGroup("g-1", "group:stable-g1");
  const scope = resolveNativeHistoryScope(channel({ id: "g-1" }));
  assert.equal(scope, "group:stable-g1");
  assert.notEqual(scope, "group:g-1");
});

test("resolveNativeHistoryScope: a DM resolves its deterministic dm scope without consulting the registry", () => {
  const peer = "c".repeat(64);
  // Plant a group scope under the DM channel id to prove the DM path ignores it.
  setResolvedHistoryScope(peer, "group:planted");
  assert.equal(
    resolveNativeHistoryScope(channel({ id: peer, channelType: "dm" })),
    `dm:${peer}`,
  );
  // The planted entry remains in the registry untouched — DMs simply don't read it.
  assert.equal(getResolvedHistoryScope(peer), "group:planted");
});

test("fetchNativeChannelWindow targets the daemon-resolved scope, never the transient REST id", async () => {
  resolveGroup("g-1", "group:stable-g1");
  response = { rows: [], has_more: false };
  await fetchNativeChannelWindow(channel({ id: "g-1" }));
  assert.equal(calls[0].cmd, "x0x_history_list");
  assert.equal(calls[0].args.scope, "group:stable-g1");
});

test("fetchNativeChannelWindow throws for an unresolved group instead of querying the transient id", async () => {
  await assert.rejects(
    fetchNativeChannelWindow(channel({ id: "g-unresolved" })),
    /is not resolved/,
  );
  assert.equal(
    calls.length,
    0,
    "no history request is issued against an unresolved group",
  );
});

test("fetchNativeMessagesById throws for an unresolved group (no transient-id point lookups)", async () => {
  await assert.rejects(
    fetchNativeMessagesById(
      channel({ id: "g-unresolved" }),
      new Set(["a".repeat(64)]),
    ),
    /is not resolved/,
  );
  assert.equal(calls.length, 0);
});

test("searchNativeMessages skips unresolved groups and queries only resolved scopes", async () => {
  resolveGroup("g-resolved", "group:stable-resolved");
  response = { rows: [], has_more: false };
  await searchNativeMessages(
    "needle",
    [channel({ id: "g-unresolved" }), channel({ id: "g-resolved" })],
    10,
  );
  assert.deepEqual(
    calls.map((c) => c.args.scope),
    ["group:stable-resolved"],
  );
});

test("sendNativeMessage forwards ADR-0029 thread fields and uses the daemon msg_id as canonical id", async () => {
  const rootId = "a".repeat(64);
  const parentId = "b".repeat(64);
  const daemonMsgId = "c".repeat(64);
  const agentId = "d".repeat(64);

  // The daemon's POST /groups/:id/send response carries the ADR-0029 msg_id.
  response = daemonMsgId;

  const event = await sendNativeMessage({
    channel: channel({ id: "group-thread-test" }),
    content: "threaded reply",
    identity: { agentId },
    threadRoot: rootId,
    threadParent: parentId,
  });

  // The Tauri command was invoked with thread ancestry.
  const sendCall = calls.find((c) => c.cmd === "x0x_send_group_message");
  assert.ok(sendCall, "x0x_send_group_message was invoked");
  assert.equal(sendCall.args.input.threadRoot, rootId);
  assert.equal(sendCall.args.input.threadParent, parentId);
  assert.equal(sendCall.args.input.groupId, "group-thread-test");

  // The daemon-returned msg_id is the canonical event id — not the local
  // clientId fallback. This is the identity the history stream reconciles on.
  assert.equal(event.id, daemonMsgId);
  assert.notEqual(event.id, event.localKey);

  // Thread tags are present for UI rendering.
  const rootTag = event.tags.find((t) => t[0] === "e" && t[3] === "root");
  const replyTag = event.tags.find((t) => t[0] === "e" && t[3] === "reply");
  assert.ok(rootTag, "root thread tag present");
  assert.equal(rootTag[1], rootId);
  assert.ok(replyTag, "reply thread tag present");
  assert.equal(replyTag[1], parentId);
});

test("sendNativeMessage falls back to clientId when daemon returns no msg_id", async () => {
  response = null;

  const event = await sendNativeMessage({
    channel: channel({ id: "group-no-msgid" }),
    content: "root message",
    identity: { agentId: "e".repeat(64) },
  });

  // No daemon msg_id → optimistic row uses the local clientId.
  assert.equal(event.id, event.localKey);
  assert.ok(event.id, "event has a non-null id");

  // No thread tags when threadRoot is absent.
  assert.ok(
    !event.tags.some((t) => t[0] === "e"),
    "no thread tags without threadRoot",
  );
});
