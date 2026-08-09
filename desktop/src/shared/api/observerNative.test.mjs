// Native observer transport acceptance — focused unit tests for the
// decode/filter boundary that protects the observer store:
//   - owner-auth (wrong owner rejected)
//   - wrong/unknown sender rejection
//   - no chat-timeline leakage (non-observer DMs never decode)
//   - cold↔live canonical id consistency
//
// Replaces the legacy relay-subscribe regression test (kind:24200 is gone).

import assert from "node:assert/strict";
import test from "node:test";

import {
  buildObserverEnvelope,
  encodeObserverEnvelope,
  parseObserverEnvelope,
} from "./observerEnvelope.ts";
import {
  decodeObserverFromHistory,
  decodeObserverFromLive,
} from "./observerNative.ts";

const CHILD = "ab".repeat(32);
const OTHER_CHILD = "cd".repeat(32);
const OWNER = "11".repeat(32);
const OTHER_OWNER = "22".repeat(32);
const MSG_ID = "fe".repeat(32);

// Known managed children — the set the store maintains live as agents register.
const KNOWN = new Set([CHILD]);

const EVENT = {
  seq: 7,
  timestamp: "2026-08-05T10:00:00Z",
  kind: "tool_call",
  agentIndex: 0,
  channelId: "ch-1",
  sessionId: "s-1",
  turnId: "t-1",
  payload: { title: "bash: ls" },
};

function b64(str) {
  return Buffer.from(str, "utf8").toString("base64");
}

function liveFrame(envelope, { sender = CHILD } = {}) {
  // The DM sender is the child that emitted the frame; the envelope's own
  // agent/owner are preserved verbatim (so owner-auth/child-auth are testable).
  return {
    type: "direct_message",
    sender,
    machineId: "9f".repeat(32),
    payload: b64(JSON.stringify(envelope)),
    receivedAt: 1,
    verified: true,
    msgId: MSG_ID,
  };
}

test("envelope round-trips through encode/parse", () => {
  const env = buildObserverEnvelope("telemetry", CHILD, OWNER, EVENT);
  const parsed = parseObserverEnvelope(encodeObserverEnvelope(env));
  assert.equal(parsed?.kind, "observer");
  assert.equal(parsed?.frame, "telemetry");
  assert.equal(parsed?.agent, CHILD);
  assert.equal(parsed?.owner, OWNER);
});

test("decodeObserverFromLive admits a telemetry frame from a known child to the owner", () => {
  const env = buildObserverEnvelope("telemetry", CHILD, OWNER, EVENT);
  const decoded = decodeObserverFromLive(liveFrame(env), OWNER, KNOWN);
  assert.equal(decoded?.agent, CHILD);
  assert.equal(decoded?.frame, "telemetry");
  assert.equal(decoded?.observerEvent.seq, 7);
  assert.equal(decoded?.msgId, MSG_ID);
});

test("no chat-timeline leakage: a non-observer DM (chat) is dropped", () => {
  const chat = {
    type: "direct_message",
    sender: CHILD,
    machineId: "9f".repeat(32),
    payload: b64(JSON.stringify({ type: "message", body: "hi" })),
    receivedAt: 1,
    verified: true,
    msgId: MSG_ID,
  };
  assert.equal(
    decodeObserverFromLive(chat, OWNER, KNOWN),
    null,
    "chat DM must never decode as an observer frame",
  );
});

test("owner-auth: an observer frame addressed to a different owner is rejected", () => {
  // Envelope claims a different owner (defense-in-depth even though sender is CHILD).
  const env = buildObserverEnvelope("telemetry", CHILD, OTHER_OWNER, EVENT);
  assert.equal(
    decodeObserverFromLive(liveFrame(env), OWNER, KNOWN),
    null,
    "frame not addressed to the local owner must be dropped",
  );
});

test("unknown-child rejection: a frame from a child we don't manage is dropped", () => {
  const env = buildObserverEnvelope("telemetry", OTHER_CHILD, OWNER, EVENT);
  const frame = {
    type: "direct_message",
    sender: OTHER_CHILD,
    machineId: "9f".repeat(32),
    payload: b64(JSON.stringify({ ...env, agent: OTHER_CHILD, owner: OWNER })),
    receivedAt: 1,
    verified: true,
    msgId: MSG_ID,
  };
  assert.equal(
    decodeObserverFromLive(frame, OWNER, KNOWN),
    null,
    "frame from an unmanaged child must be dropped",
  );
});

test("control_result frames are admitted (telemetry + control_result only)", () => {
  const controlResultEvent = {
    ...EVENT,
    kind: "control_result",
    payload: { type: "cancel_turn", status: "sent" },
  };
  const env = buildObserverEnvelope(
    "control_result",
    CHILD,
    OWNER,
    controlResultEvent,
  );
  const decoded = decodeObserverFromLive(liveFrame(env), OWNER, KNOWN);
  assert.equal(decoded?.frame, "control_result");
  assert.equal(decoded?.observerEvent.kind, "control_result");
});

test("control frames are not admitted inbound (control is owner→child)", () => {
  const env = buildObserverEnvelope("control", CHILD, OWNER, EVENT);
  assert.equal(
    decodeObserverFromLive(liveFrame(env), OWNER, KNOWN),
    null,
    "a control frame must not be ingested as inbound telemetry",
  );
});

test("cold↔live canonical id consistency: history row + live frame share msgId", () => {
  const env = buildObserverEnvelope("telemetry", CHILD, OWNER, EVENT);
  const payload = b64(JSON.stringify({ ...env, owner: OWNER }));
  const row = {
    id: 1,
    msgId: MSG_ID,
    scope: `dm:${CHILD}`,
    authorAgent: CHILD,
    authorMachine: "9f".repeat(32),
    sentAtMs: 1,
    seenAtMs: 1,
    direction: "inbound",
    contentType: "application/vnd.buzz.observer.v1+json",
    payload,
    signed: true,
    provenance: "loopback",
    replaceKey: null,
    threadRoot: null,
    threadParent: null,
  };
  const fromHistory = decodeObserverFromHistory(row, OWNER, KNOWN);
  const fromLive = decodeObserverFromLive(liveFrame(env), OWNER, KNOWN);
  assert.equal(fromHistory?.msgId, MSG_ID);
  assert.equal(fromLive?.msgId, MSG_ID);
  assert.deepEqual(fromHistory?.observerEvent, fromLive?.observerEvent);
});

test("history row: a non-observer content_type that isn't an observer payload is dropped", () => {
  const row = {
    id: 2,
    msgId: "00".repeat(32),
    scope: `dm:${CHILD}`,
    authorAgent: CHILD,
    authorMachine: "9f".repeat(32),
    sentAtMs: 1,
    seenAtMs: 1,
    direction: "inbound",
    contentType: "text/plain",
    payload: b64("hello chat"),
    signed: true,
    provenance: "loopback",
    replaceKey: null,
    threadRoot: null,
    threadParent: null,
  };
  assert.equal(
    decodeObserverFromHistory(row, OWNER, KNOWN),
    null,
    "a text/plain chat row must not decode as observer history",
  );
});
