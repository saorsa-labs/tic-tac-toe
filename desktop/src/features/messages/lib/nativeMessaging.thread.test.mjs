/**
 * Thread-open (read) and capability-state behaviour for the native messaging
 * layer. Thread replies are resolved by canonical msgId ancestry, and the
 * unsupported write ops (edit/delete/react/thread-reply) are reflected as an
 * explicit capability state the UI hides controls against — not as submit-time
 * surprise blockers.
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

const {
  fetchNativeThreadReplies,
  nativeMessageCapabilities,
  NATIVE_EDIT_BLOCKER,
  NATIVE_DELETE_BLOCKER,
  NATIVE_REACTION_BLOCKER,
} = await import("@/features/messages/lib/nativeMessaging");

const { setResolvedHistoryScope, clearAllResolvedHistoryScopes } = await import(
  "@/features/messages/lib/nativeHistoryScopeStore.ts"
);

// The default channel ("group-1") has its daemon-resolved scope ready by
// default, so the thread-ancestry tests exercise a resolvable scope; tests that
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

function payload(text, clientId) {
  return btoa(JSON.stringify({ text, clientId, createdAt: 1_700_000_000_000 }));
}

// Raw snake_case wire shape (consumed by x0xHistoryList's fromRaw mappers).
function rawRow(overrides = {}) {
  return {
    id: 10,
    msg_id: "a".repeat(64),
    scope: "group:group-1",
    author_agent: "b".repeat(64),
    author_machine: null,
    sent_at_ms: 1_700_000_000_000,
    seen_at_ms: 1_700_000_001_000,
    direction: "Inbound",
    content_type: "text/plain",
    payload: payload("hi", "c-1"),
    signed: true,
    provenance: "VerifiedEnvelope",
    replace_key: null,
    thread_root: null,
    thread_parent: null,
    ...overrides,
  };
}

// ── Thread open: replies resolved by canonical msgId ancestry ──────────────

test("fetchNativeThreadReplies returns rows whose threadRoot === rootId, excluding the root itself", async () => {
  const ROOT = "1".repeat(64);
  const REPLY = "2".repeat(64);
  const OTHER_ROOT = "3".repeat(64);
  pages.length = 0;
  pages.push({
    rows: [
      rawRow({ id: 20, msg_id: ROOT, thread_root: ROOT, thread_parent: null }),
      rawRow({ id: 19, msg_id: REPLY, thread_root: ROOT, thread_parent: ROOT }),
      rawRow({
        id: 18,
        msg_id: OTHER_ROOT,
        thread_root: OTHER_ROOT,
        thread_parent: null,
      }),
    ],
    has_more: false,
  });
  calls.length = 0;

  const replies = await fetchNativeThreadReplies(channel(), ROOT);

  assert.equal(calls[0].cmd, "x0x_history_list");
  assert.equal(calls[0].args.scope, "group:group-1");
  assert.equal(replies.length, 1);
  assert.equal(replies[0].id, REPLY);
});

test("thread open on a canonical msgId root resolves ancestry (the live-msgId-fix scenario)", async () => {
  // Before the adapter fix a live-only row keyed RelayEvent.id by clientId, so
  // opening a thread passed a clientId as rootId while the daemon stores a
  // msgId — the match returned []. With canonical msgId identity the rootId
  // passed here is the same msgId the daemon stores under threadRoot.
  const LIVE_MSG_ID = "f".repeat(64);
  const REPLY_ID = "e".repeat(64);
  pages.length = 0;
  pages.push({
    rows: [
      rawRow({ id: 30, msg_id: LIVE_MSG_ID, thread_root: LIVE_MSG_ID }),
      rawRow({
        id: 29,
        msg_id: REPLY_ID,
        thread_root: LIVE_MSG_ID,
        thread_parent: LIVE_MSG_ID,
      }),
    ],
    has_more: false,
  });

  const replies = await fetchNativeThreadReplies(channel(), LIVE_MSG_ID);
  assert.equal(replies.length, 1);
  assert.equal(replies[0].id, REPLY_ID);
});

test("fetchNativeThreadReplies returns an empty list when no ancestry matches the root", async () => {
  pages.length = 0;
  pages.push({
    rows: [
      rawRow({ id: 5, msg_id: "9".repeat(64), thread_root: "9".repeat(64) }),
    ],
    has_more: false,
  });
  const replies = await fetchNativeThreadReplies(channel(), "1".repeat(64));
  assert.deepEqual(replies, []);
});

test("fetchNativeThreadReplies throws for an unresolved group instead of paging the transient id", async () => {
  // Distinct channel id with no resolved scope — a thread must not page history
  // against the transient REST id, and the consumer surfaces an honest failure.
  await assert.rejects(
    fetchNativeThreadReplies(channel({ id: "g-unresolved" }), "1".repeat(64)),
    /is not resolved/,
  );
  assert.equal(
    calls.length,
    0,
    "no history page is fetched while the scope is unresolved",
  );
});

// ── Capability state: unsupported ops are hidden, not submit-blocked ──────

test("nativeMessageCapabilities marks edit/delete/react unsupported but thread-reply supported", () => {
  assert.equal(nativeMessageCapabilities.canReplyInThread, true);
  assert.equal(nativeMessageCapabilities.canEditMessage, false);
  assert.equal(nativeMessageCapabilities.canDeleteMessage, false);
  assert.equal(nativeMessageCapabilities.canToggleReaction, false);
});

test("each still-unsupported capability has a matching blocker naming the missing daemon contract", () => {
  assert.match(NATIVE_EDIT_BLOCKER, /edit/);
  assert.match(NATIVE_DELETE_BLOCKER, /delete/);
  assert.match(NATIVE_REACTION_BLOCKER, /reaction/);
});
