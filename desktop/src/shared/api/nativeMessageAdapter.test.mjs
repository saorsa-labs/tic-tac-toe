/**
 * Identity contract for the native ↔ RelayEvent adapter.
 *
 * Guards the critical M3 invariant: live and history messages share ONE
 * canonical id (the daemon-assigned BLAKE3 msgId), with the sender's clientId
 * carried as `localKey` so an optimistic/live/history row reconciles across
 * the window merge. Before this fix, live frames keyed RelayEvent.id by the
 * clientId UUID while history keyed it by msgId — breaking thread-open,
 * deep-link scroll-to, and search identity until a history refresh.
 */
import assert from "node:assert/strict";
import test from "node:test";

// The adapter performs no Tauri invoke at module load; provide a minimal
// window global so the @/ test loader's environment is satisfied.
globalThis.window = globalThis.window ?? {};
const { channelIdFromScope, historyRowToRelayEvent, liveMessageToRelayEvent } =
  await import("@/shared/api/nativeMessageAdapter");

const CHANNEL_ID = "group-1";
const CLIENT_ID = "11111111-1111-1111-1111-111111111111";
const MSG_ID = "a".repeat(64);
const AGENT = "b".repeat(64);

function payload(text, clientId = CLIENT_ID) {
  return btoa(JSON.stringify({ text, clientId, createdAt: 1_700_000_000_000 }));
}

function liveFrame(overrides = {}) {
  return {
    topic: "x0x.group.group-1.chat",
    payload: payload("hi"),
    origin: AGENT,
    ...overrides,
  };
}

function historyRow(overrides = {}) {
  return {
    id: 10,
    msgId: MSG_ID,
    scope: "group:group-1",
    authorAgent: AGENT,
    authorMachine: null,
    sentAtMs: 1_700_000_000_000,
    seenAtMs: 1_700_000_001_000,
    direction: "Inbound",
    contentType: "text/plain",
    payload: payload("hi"),
    signed: true,
    provenance: "VerifiedEnvelope",
    replaceKey: null,
    threadRoot: null,
    threadParent: null,
    ...overrides,
  };
}

// ── Canonical id: live frame carries msgId → RelayEvent.id = msgId ─────────

test("live frame with msgId keys RelayEvent.id by msgId and localKey by clientId", () => {
  const event = liveMessageToRelayEvent(
    liveFrame({ msgId: MSG_ID }),
    CHANNEL_ID,
  );
  assert.equal(event.id, MSG_ID);
  assert.equal(event.localKey, CLIENT_ID);
});

test("live frame without msgId falls back to clientId for id AND localKey", () => {
  const event = liveMessageToRelayEvent(liveFrame(), CHANNEL_ID);
  assert.equal(event.id, CLIENT_ID);
  assert.equal(event.localKey, CLIENT_ID);
});

test("live frame with empty or null msgId falls back to clientId", () => {
  assert.equal(
    liveMessageToRelayEvent(liveFrame({ msgId: "" }), CHANNEL_ID).id,
    CLIENT_ID,
  );
  assert.equal(
    liveMessageToRelayEvent(liveFrame({ msgId: null }), CHANNEL_ID).id,
    CLIENT_ID,
  );
});

// ── Live + history share ONE canonical id ─────────────────────────────────

test("live and history share the canonical msgId; localKey reconciles both", () => {
  const live = liveMessageToRelayEvent(
    liveFrame({ msgId: MSG_ID }),
    CHANNEL_ID,
  );
  const history = historyRowToRelayEvent(historyRow(), CHANNEL_ID);
  assert.equal(live.id, history.id);
  assert.equal(live.id, MSG_ID);
  assert.equal(history.id, MSG_ID);
  // Both carry the same clientId localKey → mergeLiveChannelWindowEvent /
  // reconcileIncomingMessage collapse the optimistic row into the canonical one.
  assert.equal(live.localKey, CLIENT_ID);
  assert.equal(history.localKey, CLIENT_ID);
});

test("an optimistic clientId row is reconciled by a live msgId row via localKey", () => {
  // Optimistic send: id = clientId (no msgId yet), localKey = clientId.
  const optimistic = {
    id: CLIENT_ID,
    localKey: CLIENT_ID,
    pubkey: AGENT,
    created_at: 1_700_000_000,
    kind: liveMessageToRelayEvent(liveFrame(), CHANNEL_ID).kind,
    tags: [],
    content: "hi",
    sig: "",
  };
  // Live acknowledgement: id = msgId, localKey = clientId.
  const acknowledged = liveMessageToRelayEvent(
    liveFrame({ msgId: MSG_ID }),
    CHANNEL_ID,
  );
  // The merge's render key is localKey ?? id; matching localKey ⇒ same row.
  assert.equal(
    optimistic.localKey ?? optimistic.id,
    acknowledged.localKey ?? acknowledged.id,
  );
  assert.notEqual(optimistic.id, acknowledged.id);
});

// ── Thread ancestry is carried verbatim as e-tags ─────────────────────────

test("live thread ancestry is projected into adapter e-tags by msgId", () => {
  const ROOT = "c".repeat(64);
  const PARENT = "d".repeat(64);
  const event = liveMessageToRelayEvent(
    liveFrame({ msgId: MSG_ID, threadRoot: ROOT, threadParent: PARENT }),
    CHANNEL_ID,
  );
  const eTags = event.tags.filter((t) => t[0] === "e");
  assert.deepEqual(eTags, [
    ["e", ROOT, "", "root"],
    ["e", PARENT, "", "reply"],
  ]);
});

test("a history row that is its own root carries a single root e-tag", () => {
  const event = historyRowToRelayEvent(
    historyRow({ threadRoot: MSG_ID, threadParent: null }),
    CHANNEL_ID,
  );
  const eTags = event.tags.filter((t) => t[0] === "e");
  assert.deepEqual(eTags, [["e", MSG_ID, "", "root"]]);
});

// ── Non-renderable payloads map to null ───────────────────────────────────

test("non-text history content type and undecodable payload map to null", () => {
  assert.equal(
    historyRowToRelayEvent(
      historyRow({ contentType: "application/octet-stream" }),
      CHANNEL_ID,
    ),
    null,
  );
  assert.equal(
    historyRowToRelayEvent(
      historyRow({ payload: "%%%not-json%%%" }),
      CHANNEL_ID,
    ),
    null,
  );
});

test("literal x0xd text/plain history remains renderable after restart", () => {
  const marker = "cutest-769f-dm-b2a-20260811T2227BST";
  const msgId = "7".repeat(64);
  const event = historyRowToRelayEvent(
    historyRow({
      msgId,
      contentType: "text/plain",
      payload: btoa(marker),
    }),
    CHANNEL_ID,
  );

  assert.ok(event);
  assert.equal(event.id, msgId);
  assert.equal(event.localKey, msgId);
  assert.equal(event.content, marker);
});

test("non-envelope JSON control rows stay out of chat history", () => {
  const event = historyRowToRelayEvent(
    historyRow({
      contentType: "text/plain",
      payload: btoa(JSON.stringify({ type: "result", event: "member_added" })),
    }),
    CHANNEL_ID,
  );

  assert.equal(event, null);
});

test("typed JSON DM history is a renderable channel envelope", () => {
  const event = historyRowToRelayEvent(
    historyRow({ contentType: "application/json" }),
    CHANNEL_ID,
  );
  assert.equal(event.id, MSG_ID);
  assert.equal(event.localKey, CLIENT_ID);
  assert.equal(event.content, "hi");
});

test("channel tag and author are projected from the frame/row", () => {
  const live = liveMessageToRelayEvent(
    liveFrame({ msgId: MSG_ID }),
    CHANNEL_ID,
  );
  assert.deepEqual(live.tags[0], ["h", CHANNEL_ID]);
  assert.equal(live.pubkey, AGENT);
  const history = historyRowToRelayEvent(historyRow(), CHANNEL_ID);
  assert.deepEqual(history.tags[0], ["h", CHANNEL_ID]);
  assert.equal(history.pubkey, AGENT);
});

// ─── channelIdFromScope: scope → channel id (inverse of nativeScopeForChannel) ─

test("channelIdFromScope extracts the id segment for group/dm/topic scopes", () => {
  assert.equal(channelIdFromScope("group:team-1"), "team-1");
  assert.equal(channelIdFromScope(`dm:${"b".repeat(64)}`), "b".repeat(64));
  assert.equal(channelIdFromScope("topic:dev"), "dev");
});

test("channelIdFromScope only splits on the first colon", () => {
  // A topic name containing a colon must not be truncated past the first segment.
  assert.equal(channelIdFromScope("topic:foo:bar"), "foo:bar");
});
