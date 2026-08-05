/**
 * M3 native data acceptance — cold workspace backfill → live (ADR-0023 §7/§9).
 *
 * Covers product acceptance test #2 (history survives) at the live-transport
 * seam: on a cold workspace open, ONE backfill-then-live stream replays stored
 * rows oldest→newest, crosses an unconditional `{type:"live"}` boundary, then
 * streams live frames — with no gap and no duplicate across the marker.
 *
 * Source contract (durable-history-implementation.md §8 seam rule + §9 test):
 *   "the live broadcast tap is established *before* the backfill query runs,
 *    and events are deduped by msg_id across the marker — no gap, no duplicate
 *    (the seam bug this design exists to prevent)."
 *
 * These controls prove the frozen `subscribeX0xLive` seam (tauriNativeX0x.ts)
 * delivers that ordered stream over the Tauri Channel and that a correct client
 * merge over the delivered frames is gap-free and duplicate-free. This is the
 * native replacement for the relay-dialect EOSE/39006 "head" boundary.
 *
 * Runs via: node --import ./test-loader.mjs --experimental-strip-types --test
 */
import assert from "node:assert/strict";
import test from "node:test";

// ── Tauri internals shim with a real Channel callback registry ───────────────
// The Channel constructor (core.js:74) registers its raw handler via
// transformCallback and serializes as `__CHANNEL__:<id>` (toJSON). We capture
// that id so we can drive frames through the REAL Channel ordering machinery.
const calls = [];
const registry = new Map();
let nextCbId = 1;
let responseFor = null;

globalThis.window = globalThis.window ?? {};
globalThis.window.__TAURI_INTERNALS__ = {
  invoke: async (cmd, args) => {
    calls.push({ cmd, args });
    if (responseFor) return responseFor(cmd, args);
    return null;
  },
  transformCallback: (cb) => {
    const id = nextCbId++;
    registry.set(id, cb);
    return id;
  },
  unregisterCallback: (id) => registry.delete(id),
};

function setResponse(fn) {
  responseFor = fn;
  calls.length = 0;
}

/** Drive `frames` through the Channel passed in the subscribe call. */
function deliver(subscribeCall, frames) {
  // args.onFrame is the Channel *instance* (toJSON only fires over real IPC),
  // so read its callback id directly and replay frames through the registry.
  const id = subscribeCall.args.onFrame.id;
  const handler = registry.get(id);
  frames.forEach((frame, index) => {
    handler({ index, message: frame });
  });
}

const { subscribeX0xLive } = await import("@/shared/api/tauriNativeX0x");

// ── Wire contract ────────────────────────────────────────────────────────────

test("subscribeX0xLive invokes x0x_subscribe_live with scope + native backfill cursor", async () => {
  setResponse(() => ({ streamId: "s1" }));
  await subscribeX0xLive(
    { scope: "topic:dev", backfill: { limit: 50 } },
    () => {},
  );
  assert.equal(calls[0].cmd, "x0x_subscribe_live");
  assert.deepEqual(calls[0].args, {
    scope: "topic:dev",
    topics: null,
    backfill: { limit: 50, beforeId: null, sinceMs: null },
    onFrame: calls[0].args.onFrame, // opaque __CHANNEL__:<id>
  });
  assert.match(calls[0].args.onFrame.toJSON(), /^__CHANNEL__:\d+$/);
});

test("omitting backfill sends backfill:null (live-only, legacy behaviour)", async () => {
  setResponse(() => ({ streamId: "s1" }));
  await subscribeX0xLive({ scope: `dm:${"ff".repeat(32)}` }, () => {});
  assert.equal(calls[0].args.backfill, null);
});

test("close() tears the stream down via x0x_close_live with the returned streamId", async () => {
  setResponse(() => ({ streamId: "stream-xyz" }));
  const sub = await subscribeX0xLive({ scope: "topic:dev" }, () => {});
  await sub.close();
  const close = calls.find((c) => c.cmd === "x0x_close_live");
  assert.ok(close, "x0x_close_live was invoked");
  assert.equal(close.args.streamId, "stream-xyz");
});

// ── Backfill → live ordering: NO GAP (AT#2) ───────────────────────────────────

test("frames are delivered in contract order: connected → backfill oldest→newest → live marker → live", async () => {
  const sent = [
    { type: "connected", sessionId: "sess", agentId: "aa".repeat(32) },
    { type: "subscribed", topics: ["dev"] },
    // Backfill replay: oldest first (seenAtMs ascending).
    { type: "message", topic: "dev", payload: "b19", origin: null },
    { type: "message", topic: "dev", payload: "b20", origin: null },
    // The unconditional backfill→live boundary. Carries NO payload — it is the
    // marker, never a renderable row.
    { type: "live", topic: "dev" },
    // Live frames arrive strictly after the marker.
    {
      type: "message",
      topic: "dev",
      payload: "live21",
      origin: "ee".repeat(32),
    },
  ];
  setResponse(() => ({ streamId: "s1" }));

  const received = [];
  await subscribeX0xLive({ scope: "topic:dev", backfill: { limit: 50 } }, (f) =>
    received.push(f),
  );
  deliver(calls[0], sent);

  // The seam passes frames through verbatim and in order — nothing dropped or
  // reordered, so the cold-start window is gap-free at the transport boundary.
  assert.deepEqual(received, sent);

  // The marker is present exactly once and sits between backfill and live.
  const markerIndex = received.findIndex((f) => f.type === "live");
  assert.equal(markerIndex, 4);
  const beforeMarker = received
    .slice(0, markerIndex)
    .filter((f) => f.type === "message");
  const afterMarker = received
    .slice(markerIndex + 1)
    .filter((f) => f.type === "message");
  assert.deepEqual(
    beforeMarker.map((m) => m.payload),
    ["b19", "b20"],
    "backfill replays oldest→newest before the marker",
  );
  assert.deepEqual(
    afterMarker.map((m) => m.payload),
    ["live21"],
    "live frames follow the marker",
  );
});

test("the live marker is unconditional even when backfill was empty", async () => {
  // A fresh workspace with no stored rows still crosses the boundary — the
  // consumer must not block waiting for backfill rows that will never come.
  const sent = [
    { type: "connected", sessionId: "s", agentId: "aa".repeat(32) },
    { type: "live", topic: "dev" },
    { type: "message", topic: "dev", payload: "first-live", origin: null },
  ];
  setResponse(() => ({ streamId: "s1" }));
  const received = [];
  await subscribeX0xLive({ scope: "topic:dev", backfill: { limit: 50 } }, (f) =>
    received.push(f),
  );
  deliver(calls[0], sent);
  assert.ok(
    received.some((f) => f.type === "live"),
    "marker delivered even with an empty backfill window",
  );
});

// ── Dedupe across the marker: NO DUPLICATE (AT#2) ────────────────────────────
//
// The daemon dedupes by BLAKE3 msg_id before delivery (§8). The TS live frame
// carries payload (not msg_id), so a correct client merges backfill+live keyed
// by content and never renders the same message twice. This reference merge is
// the contract a migrated feature MUST satisfy — it is the seam bug §9 exists
// to prevent (a live re-delivery of a backfilled row double-rendering).
function mergeTimeline(frames) {
  const seen = new Set();
  const timeline = [];
  for (const f of frames) {
    if (f.type !== "message") continue;
    if (seen.has(f.payload)) continue; // dedupe across the marker
    seen.add(f.payload);
    timeline.push(f.payload);
  }
  return timeline;
}

test("backfill + delayed live merge is duplicate-free across the marker", async () => {
  // Cold open: backfill replays [m1, m2], then a DELAYED live frame m3 arrives.
  // m3 is new content — the merged timeline is [m1, m2, m3], no gap, no dup.
  const sent = [
    { type: "message", topic: "dev", payload: "m1", origin: null },
    { type: "message", topic: "dev", payload: "m2", origin: null },
    { type: "live", topic: "dev" },
    { type: "message", topic: "dev", payload: "m3", origin: null },
  ];
  setResponse(() => ({ streamId: "s1" }));
  const received = [];
  await subscribeX0xLive({ scope: "topic:dev", backfill: {} }, (f) =>
    received.push(f),
  );
  deliver(calls[0], sent);
  assert.deepEqual(mergeTimeline(received), ["m1", "m2", "m3"]);
});

test("a live re-delivery of a backfilled row is collapsed (the seam bug this design prevents)", async () => {
  // If the live tap were established AFTER the backfill query (the bug), a row
  // published during backfill would be re-delivered live. The daemon dedupes
  // by msg_id; a correct client keys by content so the duplicate never renders.
  const sent = [
    { type: "message", topic: "dev", payload: "racey", origin: null },
    { type: "live", topic: "dev" },
    { type: "message", topic: "dev", payload: "racey", origin: null }, // dup
    { type: "message", topic: "dev", payload: "fresh", origin: null },
  ];
  setResponse(() => ({ streamId: "s1" }));
  const received = [];
  await subscribeX0xLive({ scope: "topic:dev", backfill: {} }, (f) =>
    received.push(f),
  );
  deliver(calls[0], sent);
  assert.deepEqual(mergeTimeline(received), ["racey", "fresh"]);
});

test("live frames carry optional thread ancestry when the daemon sets it", async () => {
  // Thread metadata is optional on live frames (absent ⟺ null). A reply that
  // arrives live still preserves its root/parent for thread rendering.
  const root = "11".repeat(32);
  const sent = [
    { type: "live", topic: "dev" },
    {
      type: "message",
      topic: "dev",
      payload: "reply-body",
      origin: "ee".repeat(32),
      threadRoot: root,
      threadParent: root,
    },
  ];
  setResponse(() => ({ streamId: "s1" }));
  const received = [];
  await subscribeX0xLive({ scope: "topic:dev", backfill: {} }, (f) =>
    received.push(f),
  );
  deliver(calls[0], sent);
  const live = received.find((f) => f.type === "message");
  assert.equal(live.threadRoot, root);
  assert.equal(live.threadParent, root);
});
