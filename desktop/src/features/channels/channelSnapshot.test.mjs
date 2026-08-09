import assert from "node:assert/strict";
import test from "node:test";

import {
  channelSnapshotKey,
  readChannelSnapshot,
  removeChannelSnapshotForGroup,
  writeChannelSnapshot,
} from "./channelSnapshot.ts";

// Per-test hermetic localStorage. Each test starts from an empty store so
// assertions never depend on ordering or leftover state from a sibling test.
const store = new Map();
globalThis.window = globalThis.window ?? {};
globalThis.window.localStorage = {
  getItem: (key) => store.get(key) ?? null,
  setItem: (key, value) => {
    store.set(key, value);
  },
  removeItem: (key) => {
    store.delete(key);
  },
};

function resetStorage() {
  store.clear();
}

function makeChannel(overrides = {}) {
  return {
    id: "chan-1",
    name: "General",
    channelType: "stream",
    visibility: "public",
    description: "",
    topic: null,
    purpose: null,
    memberCount: 3,
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

const GROUP_ID = "group-abc";

test("channelSnapshotKey embeds the opaque groupId verbatim", () => {
  // A group id is a stable x0x identifier, not a URL — it is NEVER normalized.
  assert.equal(channelSnapshotKey(GROUP_ID), "buzz-channels.v1:group-abc");
  assert.notEqual(
    channelSnapshotKey("group-abc"),
    channelSnapshotKey("group-xyz"),
  );
});

test("read after write returns the persisted channels", () => {
  resetStorage();
  const channels = [makeChannel(), makeChannel({ id: "chan-2", name: "Dev" })];
  writeChannelSnapshot(GROUP_ID, channels);
  assert.deepEqual(readChannelSnapshot(GROUP_ID), channels);
});

test("read for an unknown group returns null", () => {
  resetStorage();
  assert.equal(readChannelSnapshot("never-written"), null);
});

test("read returns null for malformed JSON", () => {
  resetStorage();
  window.localStorage.setItem(channelSnapshotKey(GROUP_ID), "not-json{{{");
  assert.equal(readChannelSnapshot(GROUP_ID), null);
});

test("read returns null for a wrong-version payload", () => {
  resetStorage();
  window.localStorage.setItem(
    channelSnapshotKey(GROUP_ID),
    JSON.stringify({ version: 2, channels: [makeChannel()] }),
  );
  assert.equal(readChannelSnapshot(GROUP_ID), null);
});

test("read returns null when channels is not an array", () => {
  resetStorage();
  window.localStorage.setItem(
    channelSnapshotKey(GROUP_ID),
    JSON.stringify({ version: 1, channels: "nope" }),
  );
  assert.equal(readChannelSnapshot(GROUP_ID), null);
});

test("removeChannelSnapshotForGroup clears only that group's snapshot", () => {
  resetStorage();
  writeChannelSnapshot(GROUP_ID, [makeChannel()]);
  writeChannelSnapshot("group-other", [makeChannel({ id: "chan-9" })]);

  removeChannelSnapshotForGroup(GROUP_ID);

  assert.equal(readChannelSnapshot(GROUP_ID), null);
  assert.notEqual(readChannelSnapshot("group-other"), null);
});

test("write is tolerant of storage failures", () => {
  resetStorage();
  const original = window.localStorage.setItem;
  window.localStorage.setItem = () => {
    throw new Error("quota exceeded");
  };
  try {
    assert.doesNotThrow(() => writeChannelSnapshot(GROUP_ID, [makeChannel()]));
  } finally {
    window.localStorage.setItem = original;
  }
});

test("write skips re-serializing an unchanged list", () => {
  resetStorage();
  const channels = [makeChannel()];
  writeChannelSnapshot(GROUP_ID, channels);

  let setCalls = 0;
  const original = window.localStorage.setItem;
  window.localStorage.setItem = () => {
    setCalls++;
  };
  try {
    // Same channels, same order → already-serialized, no setItem.
    writeChannelSnapshot(GROUP_ID, channels);
    assert.equal(setCalls, 0);
  } finally {
    window.localStorage.setItem = original;
  }
});
