import assert from "node:assert/strict";
import test from "node:test";

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
  NATIVE_THREAD_WRITE_BLOCKER,
  nativeHistoryPageToChannelWindowPage,
  nativeScopeForChannel,
  searchNativeMessages,
} = await import("./nativeMessaging.ts");

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

  const result = await searchNativeMessages("needle", [channel()], 12);
  assert.equal(calls[0].cmd, "x0x_history_search");
  assert.equal(calls[0].args.scope, "group:group-1");
  assert.equal(result.hits[0].eventId, "d".repeat(64));
  assert.equal(result.hits[0].content, "durable needle");
  assert.equal(result.hits[0].channelId, "group-1");
});

test("unsupported native mutations expose exact blockers instead of relay fallback", () => {
  for (const blocker of [
    NATIVE_THREAD_WRITE_BLOCKER,
    NATIVE_EDIT_BLOCKER,
    NATIVE_DELETE_BLOCKER,
    NATIVE_REACTION_BLOCKER,
  ]) {
    assert.match(blocker, /x0xd/);
  }
});
