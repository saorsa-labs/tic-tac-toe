import assert from "node:assert/strict";
import test from "node:test";

// Captured Tauri invocations + a per-call response slot, mirroring the
// existing nativeMessaging.test.mjs harness. Reset between tests.
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
  nativeDmRecipientAgentId,
  nativeScopeForChannel,
  sendNativeDirectMessage,
} = await import("./nativeMessaging.ts");
const { historyRowToRelayEvent, liveDirectMessageToRelayEvent } = await import(
  "@/shared/api/nativeMessageAdapter.ts"
);
const { openDm } = await import("@/shared/api/tauriChannels.ts");
const { projectDmChannel } = await import(
  "@/features/channels/nativeChannelProjection.ts"
);
const { reconcileIncomingMessage } = await import("./messageMerge.ts");
const { getThreadReference } = await import("./threading.ts");

function dmChannel(overrides = {}) {
  return {
    id: "c".repeat(64),
    name: "c".repeat(64),
    channelType: "dm",
    visibility: "private",
    description: "",
    topic: null,
    purpose: null,
    memberCount: 2,
    memberPubkeys: ["c".repeat(64)],
    lastMessageAt: null,
    archivedAt: null,
    participants: ["c".repeat(64)],
    participantPubkeys: ["c".repeat(64)],
    isMember: true,
    ttlSeconds: null,
    ttlDeadline: null,
    ...overrides,
  };
}

function identity(agentId = "a".repeat(64)) {
  return {
    agentId,
    displayName: "self",
  };
}

function payloadB64(text, clientId) {
  return btoa(JSON.stringify({ text, clientId, createdAt: 1_700_000_000_000 }));
}

function reset() {
  calls.length = 0;
  response = null;
}

// ── nativeDmRecipientAgentId ────────────────────────────────────────────────

test("nativeDmRecipientAgentId resolves the peer from the projected channel id", () => {
  reset();
  const peer = "c".repeat(64);
  assert.equal(
    nativeDmRecipientAgentId(dmChannel({ id: peer }), identity().agentId),
    peer,
  );
});

test("nativeDmRecipientAgentId excludes the local agent from participants", () => {
  reset();
  const own = "a".repeat(64);
  const peer = "b".repeat(64);
  // A compatibility channel carrying both ids in participants must resolve to
  // the single peer (the own id is filtered out).
  const channel = dmChannel({
    id: "legacy-dm",
    participants: [own, peer],
    participantPubkeys: [own, peer],
  });
  assert.equal(nativeDmRecipientAgentId(channel, own), peer);
});

test("nativeDmRecipientAgentId throws for an ambiguous multi-peer channel", () => {
  reset();
  const peer1 = "b".repeat(64);
  const peer2 = "c".repeat(64);
  const channel = dmChannel({
    id: "group-dm",
    participants: [peer1, peer2],
    participantPubkeys: [peer1, peer2],
  });
  assert.throws(
    () => nativeDmRecipientAgentId(channel, identity().agentId),
    /exactly one peer AgentId/,
  );
});

// ── sendNativeDirectMessage ─────────────────────────────────────────────────

test("sendNativeDirectMessage invokes x0x_send_direct_message with recipient + base64 payload", async () => {
  reset();
  response = {
    ok: true,
    path: "raw_quic_acked",
    retriesUsed: 0,
    requestId: "deadbeef",
  };
  const peer = "c".repeat(64);

  const result = await sendNativeDirectMessage({
    channel: dmChannel({ id: peer }),
    content: "hello dm",
    identity: identity(),
  });

  const send = calls.find((c) => c.cmd === "x0x_send_direct_message");
  assert.ok(send, "x0x_send_direct_message was invoked");
  assert.equal(send.args.input.agentId, peer);
  // payloadB64 decodes to the typed envelope carrying the text + clientId.
  const envelope = JSON.parse(atob(send.args.input.payloadB64));
  assert.equal(envelope.text, "hello dm");
  assert.equal(typeof envelope.clientId, "string");
  assert.equal(send.args.input.threadRoot, null);
  assert.equal(send.args.input.threadParent, null);
  // The returned clientId keys the optimistic row and reconciles with the
  // canonical (msgId-keyed) row when history rehydrates.
  assert.equal(result.clientId, envelope.clientId);
  assert.equal(typeof result.createdAt, "number");
});

test("sendNativeDirectMessage forwards native thread ancestry as 64-hex msg_ids", async () => {
  reset();
  response = { ok: true };
  const peer = "c".repeat(64);
  const root = "e".repeat(64);
  const parent = "f".repeat(64);

  await sendNativeDirectMessage({
    channel: dmChannel({ id: peer }),
    content: "a reply",
    identity: identity(),
    threadRoot: root,
    threadParent: parent,
  });

  const send = calls.find((c) => c.cmd === "x0x_send_direct_message");
  assert.equal(send.args.input.threadRoot, root);
  assert.equal(send.args.input.threadParent, parent);
});

// ── liveDirectMessageToRelayEvent ───────────────────────────────────────────

test("liveDirectMessageToRelayEvent maps sender to pubkey and reconciles by clientId", () => {
  const peer = "c".repeat(64);
  const clientId = "client-1";
  const frame = {
    type: "direct_message",
    msgId: "a".repeat(64),
    sender: peer,
    machineId: "0".repeat(64),
    payload: payloadB64("hi", clientId),
    receivedAt: 1_700_000_001_000,
    verified: true,
  };
  const event = liveDirectMessageToRelayEvent(frame, peer);
  assert.ok(event);
  // Canonical id is the daemon msg_id; localKey stays the clientId so the
  // optimistic outbound row reconciles with this inbound frame.
  assert.equal(event.id, "a".repeat(64));
  assert.equal(event.localKey, clientId);
  assert.equal(event.pubkey, peer);
  assert.equal(event.content, "hi");
});

test("liveDirectMessageToRelayEvent falls back to clientId when msgId is absent", () => {
  const peer = "c".repeat(64);
  const clientId = "client-2";
  const frame = {
    type: "direct_message",
    sender: peer,
    machineId: "0".repeat(64),
    payload: payloadB64("yo", clientId),
    receivedAt: 1_700_000_002_000,
    verified: false,
  };
  const event = liveDirectMessageToRelayEvent(frame, peer);
  assert.ok(event);
  assert.equal(event.id, clientId);
  assert.equal(event.localKey, clientId);
});

test("liveDirectMessageToRelayEvent returns null for a non-envelope payload", () => {
  const peer = "c".repeat(64);
  const frame = {
    type: "direct_message",
    sender: peer,
    machineId: "0".repeat(64),
    payload: btoa("not-json"),
    receivedAt: 1,
    verified: false,
  };
  assert.equal(liveDirectMessageToRelayEvent(frame, peer), null);
});

// ── openDm projection ───────────────────────────────────────────────────────

test("openDm projects a one-to-one DM channel keyed by the peer AgentId", async () => {
  reset();
  const peer = "d".repeat(64);
  const channel = await openDm({ pubkeys: [peer] });
  assert.equal(channel.id, peer);
  assert.equal(channel.channelType, "dm");
  assert.equal(channel.visibility, "private");
  assert.deepEqual(channel.participantPubkeys, [peer]);
  assert.equal(channel.isMember, true);
});

test("openDm rejects group-DMs (no native multi-recipient contract)", async () => {
  reset();
  const peer1 = "d".repeat(64);
  const peer2 = "e".repeat(64);
  await assert.rejects(
    () => openDm({ pubkeys: [peer1, peer2] }),
    /one-to-one conversations only/,
  );
});

test("openDm rejects a non-AgentId recipient", async () => {
  reset();
  await assert.rejects(
    () => openDm({ pubkeys: ["not-a-valid-agent-id"] }),
    /64-hex x0x AgentId/,
  );
});

test("projectDmChannel lowercases the peer id for a canonical scope", () => {
  const upper = "C".repeat(64);
  const channel = projectDmChannel(upper);
  assert.equal(channel.id, upper.toLowerCase());
  assert.equal(channel.channelType, "dm");
});

// ── Cohesive open → send → live/history reconciliation ────────────────────

// The native one-to-one DM lifecycle, traced end to end as a single contract:
// open projects dm:<peer>; send posts /direct/send and returns the clientId
// the optimistic outbound row is keyed by; the durable dm:<peer> history row
// (canonical msgId) carries the SAME clientId in its payload envelope, so the
// localKey dedupe reconciles the two into one canonical row. The live inbound
// path (a peer's direct_message frame) is peer-filtered and keyed by its own
// clientId/msgId, so it never collides with the outbound optimistic row.

test("DM lifecycle: open → send → durable history reconciles the optimistic row by clientId", async () => {
  reset();
  const own = "a".repeat(64);
  const peer = "d".repeat(64);

  // 1. OPEN: openDm projects a one-to-one channel keyed by the peer AgentId.
  const channel = await openDm({ pubkeys: [peer] });
  assert.equal(channel.id, peer);
  assert.equal(channel.channelType, "dm");
  assert.deepEqual(channel.participantPubkeys, [peer]);
  const scope = nativeScopeForChannel(channel);
  assert.equal(scope, `dm:${peer}`);

  // 2. SEND: sendNativeDirectMessage invokes x0x_send_direct_message with the
  //    peer recipient + a base64 typed envelope, and returns the clientId the
  //    optimistic row is keyed by.
  response = { ok: true, path: "loopback", requestId: "req-1" };
  const { clientId, createdAt } = await sendNativeDirectMessage({
    channel,
    content: "hello one-to-one",
    identity: identity(own),
  });
  const send = calls.find((c) => c.cmd === "x0x_send_direct_message");
  assert.ok(send, "x0x_send_direct_message was invoked");
  assert.equal(send.args.input.agentId, peer);
  assert.ok(clientId, "send returned a clientId correlation key");

  // 3. OPTIMISTIC ROW: the send mutation persists an outbound row keyed by
  //    clientId (mirrors useSendMessageMutation). /ws/direct does not echo the
  //    sender's own outbound, so this row is the only copy until history lands.
  const optimistic = {
    id: clientId,
    localKey: clientId,
    pubkey: own,
    created_at: Math.floor(createdAt / 1_000),
    kind: 9,
    tags: [
      ["h", channel.id],
      ["p", own],
    ],
    content: "hello one-to-one",
    sig: "",
  };

  // 4. HISTORY COLD-LOAD: x0x_history_list returns the durable row the daemon
  //    recorded under dm:<peer>. Its msgId is canonical (BLAKE3); its payload
  //    carries the SAME clientId, so historyRowToRelayEvent projects localKey
  //    = clientId. The send receipt never carried the canonical id — it
  //    surfaces only via history.
  const canonicalMsgId = "f".repeat(64);
  const durablePayload = btoa(
    JSON.stringify({ text: "hello one-to-one", clientId, createdAt }),
  );
  const historyRow = {
    id: 1,
    msgId: canonicalMsgId,
    scope,
    authorAgent: own,
    authorMachine: null,
    sentAtMs: createdAt,
    seenAtMs: createdAt,
    direction: "Outbound",
    contentType: "text/plain",
    payload: durablePayload,
    signed: false,
    provenance: "LocalSend",
    replaceKey: null,
    threadRoot: null,
    threadParent: null,
  };
  const durable = historyRowToRelayEvent(historyRow, channel.id);
  assert.ok(durable, "history row maps to a RelayEvent");
  assert.equal(durable.id, canonicalMsgId);
  assert.equal(durable.localKey, clientId);

  // 5. RECONCILE: the durable row supersedes the optimistic one by shared
  //    localKey (clientId). One row remains, keyed by the canonical msgId,
  //    retaining clientId as the render-local reconciliation key.
  const reconciled = reconcileIncomingMessage([optimistic], durable);
  assert.equal(
    reconciled.length,
    1,
    "optimistic + durable collapse to one row",
  );
  assert.equal(reconciled[0].id, canonicalMsgId);
  assert.equal(reconciled[0].localKey, clientId);
  assert.equal(reconciled[0].content, "hello one-to-one");
});

test("DM lifecycle: live inbound peer frame is peer-filtered and reconciles separately from the outbound row", async () => {
  reset();
  const own = "a".repeat(64);
  const peer = "d".repeat(64);
  const other = "e".repeat(64);

  const channel = await openDm({ pubkeys: [peer] });
  const scope = nativeScopeForChannel(channel);
  assert.equal(scope, `dm:${peer}`);

  // An inbound direct_message frame from the DM peer arrives on /ws/direct.
  // The live consumer (useChannelSubscription) peer-filters by sender === dmPeer.
  const peerClientId = "peer-client-uuid";
  const inboundFrame = {
    type: "direct_message",
    msgId: "b".repeat(64),
    sender: peer,
    machineId: "1".repeat(64),
    payload: btoa(
      JSON.stringify({
        text: "hi back",
        clientId: peerClientId,
        createdAt: 1_700_000_001_000,
      }),
    ),
    receivedAt: 1_700_000_001_000,
    verified: true,
    threadRoot: null,
    threadParent: null,
  };
  const inbound = liveDirectMessageToRelayEvent(inboundFrame, channel.id);
  assert.ok(inbound, "peer frame maps to a RelayEvent");
  assert.equal(inbound.id, "b".repeat(64));
  assert.equal(inbound.pubkey, peer);
  assert.equal(inbound.localKey, peerClientId);

  // A frame from an unrelated peer is dropped by the consumer's peer filter
  // (sender !== dmPeer) and never reaches the timeline.
  const otherFrame = { ...inboundFrame, sender: other };
  assert.notEqual(otherFrame.sender.toLowerCase(), peer);

  // The inbound row and a prior outbound optimistic row coexist (different
  // localKeys) — they are distinct messages, not collapsed.
  const outboundClientId = "own-client-uuid";
  const outbound = {
    id: outboundClientId,
    localKey: outboundClientId,
    pubkey: own,
    created_at: 1_700_000_000,
    kind: 9,
    tags: [
      ["h", channel.id],
      ["p", own],
    ],
    content: "hello one-to-one",
    sig: "",
  };
  const merged = reconcileIncomingMessage([outbound], inbound);
  assert.equal(merged.length, 2, "inbound + outbound are distinct rows");
});

test("DM lifecycle: a threaded reply carries thread ancestry through send, history, and reconcile", async () => {
  reset();
  const own = "a".repeat(64);
  const peer = "d".repeat(64);
  const rootMsgId = "c".repeat(64);

  const channel = await openDm({ pubkeys: [peer] });
  const scope = nativeScopeForChannel(channel);

  // SEND a reply: thread_root/thread_parent are forwarded as 64-hex msg_ids.
  response = { ok: true, path: "loopback", requestId: "req-2" };
  const { clientId, createdAt } = await sendNativeDirectMessage({
    channel,
    content: "threaded reply",
    identity: identity(own),
    threadRoot: rootMsgId,
    threadParent: rootMsgId,
  });
  const send = calls.find((c) => c.cmd === "x0x_send_direct_message");
  assert.equal(send.args.input.threadRoot, rootMsgId);
  assert.equal(send.args.input.threadParent, rootMsgId);

  // The durable history row the daemon records carries the same ancestry.
  const replyMsgId = "9".repeat(64);
  const historyRow = {
    id: 2,
    msgId: replyMsgId,
    scope,
    authorAgent: own,
    authorMachine: null,
    sentAtMs: createdAt,
    seenAtMs: createdAt,
    direction: "Outbound",
    contentType: "text/plain",
    payload: btoa(
      JSON.stringify({ text: "threaded reply", clientId, createdAt }),
    ),
    signed: false,
    provenance: "LocalSend",
    replaceKey: null,
    threadRoot: rootMsgId,
    threadParent: rootMsgId,
  };
  const durable = historyRowToRelayEvent(historyRow, channel.id);
  const durableThread = getThreadReference(durable.tags);
  assert.equal(durableThread.rootId, rootMsgId);
  assert.equal(durableThread.parentId, rootMsgId);

  // Reconcile still collapses by localKey (clientId), preserving ancestry.
  const optimistic = {
    id: clientId,
    localKey: clientId,
    pubkey: own,
    created_at: Math.floor(createdAt / 1_000),
    kind: 9,
    tags: [
      ["h", channel.id],
      ["p", own],
    ],
    content: "threaded reply",
    sig: "",
  };
  const reconciled = reconcileIncomingMessage([optimistic], durable);
  assert.equal(reconciled.length, 1);
  assert.equal(reconciled[0].id, replyMsgId);
  const reconciledThread = getThreadReference(reconciled[0].tags);
  assert.equal(reconciledThread.rootId, rootMsgId);
});
