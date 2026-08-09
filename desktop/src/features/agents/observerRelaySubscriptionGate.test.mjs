import assert from "node:assert/strict";
import { test, describe, beforeEach } from "node:test";

import {
  shouldObserveManagedAgents,
  ingestArchivedObserverFrames,
  getArchivedChannelEvents,
  resetAgentObserverStore,
} from "./observerRelayStore.ts";

test("observer ingestion opens for a cold stopped managed agent", () => {
  assert.equal(
    shouldObserveManagedAgents([{ pubkey: "aa", status: "stopped" }]),
    true,
  );
});

test("observer ingestion stays closed when there are no owned agents", () => {
  assert.equal(shouldObserveManagedAgents([]), false);
});

describe("cold-load archive ingest (replay path)", () => {
  const PUBKEY = "ab".repeat(32);
  const CHANNEL = "ch-1";

  // Minimal decoded-frame shape (NativeObserverFrame). The archive ingest path
  // only inspects observerEvent.{seq,timestamp,channelId} for routing + dedupe.
  function frame(channelId, seq, timestamp) {
    return {
      agent: PUBKEY,
      frame: "telemetry",
      observerEvent: {
        seq,
        timestamp,
        kind: "tool_message",
        agentIndex: null,
        channelId,
        sessionId: null,
        turnId: null,
        payload: {},
      },
      msgId: null,
    };
  }

  beforeEach(() => resetAgentObserverStore());

  test("channel frames route to the channel archive window, sorted ascending", () => {
    const earlier = frame(CHANNEL, 1, "2026-01-01T00:00:00Z");
    const later = frame(CHANNEL, 2, "2026-01-01T00:00:01Z");
    // Feed newest-first, like x0x history pages; window must sort ascending.
    ingestArchivedObserverFrames(PUBKEY, [later, earlier]);

    const events = getArchivedChannelEvents(PUBKEY, CHANNEL);
    assert.deepEqual(
      events.map((e) => e.seq),
      [1, 2],
      "archive window is sorted ascending regardless of ingest order",
    );
  });

  test("canonical (seq, timestamp) dedupe skips an already-stored frame", () => {
    const ev = frame(CHANNEL, 5, "2026-01-01T00:00:05Z");
    ingestArchivedObserverFrames(PUBKEY, [ev]);
    ingestArchivedObserverFrames(PUBKEY, [ev]); // identical seq + timestamp

    assert.equal(
      getArchivedChannelEvents(PUBKEY, CHANNEL).length,
      1,
      "a frame with identical seq+timestamp is deduped, not appended",
    );
  });

  test("frames for different channels are isolated per channel", () => {
    ingestArchivedObserverFrames(PUBKEY, [
      frame(CHANNEL, 1, "2026-01-01T00:00:00Z"),
      frame("ch-other", 2, "2026-01-01T00:00:01Z"),
    ]);

    assert.equal(
      getArchivedChannelEvents(PUBKEY, CHANNEL).length,
      1,
      "CHANNEL window holds only its own frame",
    );
    assert.equal(
      getArchivedChannelEvents(PUBKEY, "ch-other").length,
      1,
      "the other channel has its own isolated window",
    );
  });

  test("an absent agent or channel reads back as [] without errors", () => {
    assert.deepEqual(getArchivedChannelEvents(null, CHANNEL), []);
    assert.deepEqual(getArchivedChannelEvents(PUBKEY, null), []);
  });
});
