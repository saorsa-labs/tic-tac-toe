import assert from "node:assert/strict";
import test from "node:test";

import {
  activityScopeKey,
  activityStorageKey,
  addThreadActivityItems,
  projectActivityForScope,
  readActivityFromStorage,
  writeActivityToStorage,
} from "./threadActivityStorage.ts";

// Mock window.localStorage with a simple in-memory store.
if (typeof globalThis.window === "undefined") {
  const storage = new Map();
  globalThis.window = {
    localStorage: {
      getItem: (key) => storage.get(key) ?? null,
      setItem: (key, value) => storage.set(key, value),
      removeItem: (key) => storage.delete(key),
    },
  };
}

// Helper to isolate each test's localStorage state.
function makeIsolatedStorage() {
  const store = new Map();
  const prev = globalThis.window.localStorage;
  globalThis.window.localStorage = {
    getItem: (key) => store.get(key) ?? null,
    setItem: (key, value) => store.set(key, value),
    removeItem: (key) => store.delete(key),
  };
  return {
    store,
    restore: () => {
      globalThis.window.localStorage = prev;
    },
  };
}

function makeItem(id, channelId = "channel-1", createdAt = 1) {
  return {
    id,
    kind: 9,
    pubkey: "author",
    content: "reply",
    createdAt,
    channelId,
    channelName: "general",
    tags: [
      ["h", channelId],
      ["e", "root-1", "", "root"],
    ],
  };
}

// ── activityStorageKey (production function) ─────────────────────────────────
//
// The scope dimension is the native x0x group id — a stable opaque identifier,
// NOT a URL. It is embedded verbatim (no normalization), so two distinct
// group ids always map to two distinct storage buckets.

test("activityStorageKey embeds the groupId verbatim", () => {
  assert.equal(
    activityStorageKey("pk1", "group-abc"),
    "buzz-thread-activity.v1:group-abc:pk1",
  );
});

test("activityStorageKey produces different keys for different groups", () => {
  const keyA = activityStorageKey("pk1", "group-a");
  const keyB = activityStorageKey("pk1", "group-b");
  assert.notEqual(keyA, keyB);
});

test("activityStorageKey produces different keys for different pubkeys", () => {
  const key1 = activityStorageKey("pk1", "group-a");
  const key2 = activityStorageKey("pk2", "group-a");
  assert.notEqual(key1, key2);
});

test("activityStorageKey differs from the legacy unscoped key", () => {
  // The legacy pubkey-only bucket must never collide with a group-scoped one,
  // so rows from an unknown group cannot be attributed to the current identity.
  const pubkey = "abc123";
  const groupId = "group-a";
  const legacyKey = `buzz-thread-activity.v1:${pubkey}`;
  const scopedKey = activityStorageKey(pubkey, groupId);
  assert.notEqual(legacyKey, scopedKey);
  assert.ok(
    scopedKey.includes(groupId),
    "scoped key should contain the group id",
  );
});

// ── write/read round-trip using production functions ─────────────────────────

test("round-trip: items written for group A are readable under group A", () => {
  const isolated = makeIsolatedStorage();
  try {
    const pubkey = "pk1";
    const groupA = "group-a";
    const items = [makeItem("reply-a1", "channel-1", 1)];

    writeActivityToStorage(pubkey, groupA, items);
    const read = readActivityFromStorage(pubkey, groupA);

    assert.equal(read.length, 1);
    assert.equal(read[0].id, "reply-a1");
  } finally {
    isolated.restore();
  }
});

test("round-trip: items written for group A are NOT readable under group B", () => {
  const isolated = makeIsolatedStorage();
  try {
    const pubkey = "pk1";
    const items = [makeItem("reply-a1", "channel-1", 1)];

    writeActivityToStorage(pubkey, "group-a", items);
    const read = readActivityFromStorage(pubkey, "group-b");

    assert.deepEqual(read, []);
  } finally {
    isolated.restore();
  }
});

test("round-trip: A→B→A — A rows absent in B, A rows return on switch back", () => {
  const isolated = makeIsolatedStorage();
  try {
    const pubkey = "pk1";
    const groupA = "group-a";
    const groupB = "group-b";

    // Community A accumulates two activity rows.
    const itemsA = [
      makeItem("reply-a1", "channel-a1", 1),
      makeItem("reply-a2", "channel-a2", 2),
    ];
    writeActivityToStorage(pubkey, groupA, itemsA);

    // Community B has its own rows.
    const itemsB = [makeItem("reply-b1", "channel-b1", 3)];
    writeActivityToStorage(pubkey, groupB, itemsB);

    // While in community B, reading group B gives B's rows (not A's).
    const inB = readActivityFromStorage(pubkey, groupB);
    assert.equal(inB.length, 1);
    assert.equal(inB[0].id, "reply-b1");

    // A's rows must not appear in B.
    assert.ok(
      !inB.some((item) => item.id === "reply-a1" || item.id === "reply-a2"),
      "group A rows must not appear when reading group B bucket",
    );

    // Switch back to A — A's persisted rows return.
    const backInA = readActivityFromStorage(pubkey, groupA);
    assert.equal(backInA.length, 2);
    assert.ok(backInA.some((item) => item.id === "reply-a1"));
    assert.ok(backInA.some((item) => item.id === "reply-a2"));
  } finally {
    isolated.restore();
  }
});

test("round-trip: corrupt JSON in storage returns empty array without throwing", () => {
  const isolated = makeIsolatedStorage();
  try {
    const pubkey = "pk1";
    const groupId = "group-a";
    globalThis.window.localStorage.setItem(
      activityStorageKey(pubkey, groupId),
      "not-valid-json{{{",
    );
    const read = readActivityFromStorage(pubkey, groupId);
    assert.deepEqual(read, []);
  } finally {
    isolated.restore();
  }
});

test("round-trip: non-array JSON in storage returns empty array", () => {
  const isolated = makeIsolatedStorage();
  try {
    const pubkey = "pk1";
    const groupId = "group-a";
    globalThis.window.localStorage.setItem(
      activityStorageKey(pubkey, groupId),
      JSON.stringify({ not: "an array" }),
    );
    const read = readActivityFromStorage(pubkey, groupId);
    assert.deepEqual(read, []);
  } finally {
    isolated.restore();
  }
});

// ── addThreadActivityItems ───────────────────────────────────────────────────

test("addThreadActivityItems deduplicates by id", () => {
  const existing = [makeItem("a", "ch", 1)];
  const { didAdd, items } = addThreadActivityItems(existing, [
    makeItem("a", "ch", 1),
  ]);
  assert.equal(didAdd, false);
  assert.equal(items.length, 1);
});

test("addThreadActivityItems merges new items sorted by createdAt", () => {
  const existing = [makeItem("a", "ch", 1), makeItem("c", "ch", 3)];
  const { didAdd, items } = addThreadActivityItems(existing, [
    makeItem("b", "ch", 2),
  ]);
  assert.equal(didAdd, true);
  assert.deepEqual(
    items.map((item) => item.id),
    ["a", "b", "c"],
  );
});

test("addThreadActivityItems caps at MAX_ACTIVITY_ITEMS (100) keeping newest", () => {
  const existing = Array.from({ length: 99 }, (_, i) =>
    makeItem(`old-${i}`, "ch", i + 1),
  );
  const incoming = [
    makeItem("new-100", "ch", 200),
    makeItem("new-101", "ch", 201),
  ];
  const { didAdd, items } = addThreadActivityItems(existing, incoming);
  assert.equal(didAdd, true);
  assert.equal(items.length, 100);
  assert.ok(items.some((item) => item.id === "new-100"));
  assert.ok(items.some((item) => item.id === "new-101"));
});

test("addThreadActivityItems returns didAdd false when all items are duplicates", () => {
  const existing = [makeItem("x", "ch", 1), makeItem("y", "ch", 2)];
  const { didAdd, items } = addThreadActivityItems(existing, [
    makeItem("x", "ch", 1),
    makeItem("y", "ch", 2),
  ]);
  assert.equal(didAdd, false);
  assert.equal(items, existing); // same reference
});

// ── activityScopeKey (scope-ref identity helper) ─────────────────────────────

test("activityScopeKey returns empty string when pubkey is null", () => {
  assert.equal(activityScopeKey(null, "group-a"), "");
});

test("activityScopeKey returns empty string when groupId is empty", () => {
  assert.equal(activityScopeKey("pk1", ""), "");
});

test("activityScopeKey embeds the groupId verbatim", () => {
  assert.equal(activityScopeKey("pk1", "group-a"), "pk1:group-a");
});

test("activityScopeKey differs for different pubkeys", () => {
  const k1 = activityScopeKey("pk1", "group-a");
  const k2 = activityScopeKey("pk2", "group-a");
  assert.notEqual(k1, k2);
});

test("activityScopeKey differs for different groups", () => {
  const k1 = activityScopeKey("pk1", "group-a");
  const k2 = activityScopeKey("pk1", "group-b");
  assert.notEqual(k1, k2);
});

// ── scope-transition render fence (state-machine proof) ──────────────────────
//
// These tests model the hook's render fence without a React harness:
//   threadActivityScopeRef.current = scope at last effect commit
//   currentActivityScope            = scope derived this render
//   threadActivityRef.current       = in-memory items
//
// The fence: return threadActivityRef.current only when
//   threadActivityScopeRef.current === currentActivityScope
//
// This proves that A rows are hidden on the first B render (before the reset
// effect commits), and restored only when A is active again.

function makeScopeState({ pubkey, groupId, loadedItems = [] } = {}) {
  const loaded = loadedItems;
  // Simulate what the reset effect writes (executed after commit, not in render).
  const effectCommitScope = activityScopeKey(pubkey ?? null, groupId ?? "");
  return {
    threadActivityScopeRef: { current: effectCommitScope },
    threadActivityItems: loaded,
  };
}

function renderFence(
  threadActivityScopeRef,
  threadActivityItems,
  pubkey,
  groupId,
) {
  const currentScope = activityScopeKey(pubkey ?? null, groupId ?? "");
  return projectActivityForScope(
    threadActivityScopeRef.current,
    currentScope,
    threadActivityItems,
  );
}

test("scope-transition: A rows visible when scope matches A (steady state)", () => {
  const groupA = "group-a";
  const pubkey = "pk1";
  const itemsA = [{ id: "a1" }];
  const state = makeScopeState({
    pubkey,
    groupId: groupA,
    loadedItems: itemsA,
  });

  const visible = renderFence(
    state.threadActivityScopeRef,
    state.threadActivityItems,
    pubkey,
    groupA,
  );
  assert.deepEqual(visible, itemsA);
});

test("scope-transition: A rows hidden on first B render (scope mismatch before effect commits)", () => {
  const groupA = "group-a";
  const groupB = "group-b";
  const pubkey = "pk1";
  const itemsA = [{ id: "a1" }, { id: "a2" }];

  // State reflects A's committed scope (reset effect hasn't run for B yet).
  const state = makeScopeState({
    pubkey,
    groupId: groupA,
    loadedItems: itemsA,
  });

  // Render fires with B props before reset effect commits — fence returns [].
  const visible = renderFence(
    state.threadActivityScopeRef,
    state.threadActivityItems,
    pubkey,
    groupB,
  );
  assert.deepEqual(
    visible,
    [],
    "A rows must not be visible on the B transition render",
  );
});

test("scope-transition: B rows visible after B reset effect commits", () => {
  const groupB = "group-b";
  const pubkey = "pk1";
  const itemsB = [{ id: "b1" }];

  // Reset effect has now committed for B.
  const state = makeScopeState({
    pubkey,
    groupId: groupB,
    loadedItems: itemsB,
  });

  const visible = renderFence(
    state.threadActivityScopeRef,
    state.threadActivityItems,
    pubkey,
    groupB,
  );
  assert.deepEqual(visible, itemsB);
});

test("scope-transition: A→B→A — A rows return when A's reset effect commits again", () => {
  const groupA = "group-a";
  const groupB = "group-b";
  const pubkey = "pk1";
  const itemsA = [{ id: "a1" }, { id: "a2" }];
  const itemsB = [{ id: "b1" }];

  // Step 1: in A, scope and ref match A.
  const stateA = makeScopeState({
    pubkey,
    groupId: groupA,
    loadedItems: itemsA,
  });
  assert.deepEqual(
    renderFence(
      stateA.threadActivityScopeRef,
      stateA.threadActivityItems,
      pubkey,
      groupA,
    ),
    itemsA,
  );

  // Step 2: switch to B — render fires before effect commits (stale A scope in ref).
  assert.deepEqual(
    renderFence(
      stateA.threadActivityScopeRef,
      stateA.threadActivityItems,
      pubkey,
      groupB,
    ),
    [],
    "A rows must be hidden on B transition render",
  );

  // Step 3: B reset effect commits — now ref holds B scope and B items.
  const stateB = makeScopeState({
    pubkey,
    groupId: groupB,
    loadedItems: itemsB,
  });
  assert.deepEqual(
    renderFence(
      stateB.threadActivityScopeRef,
      stateB.threadActivityItems,
      pubkey,
      groupB,
    ),
    itemsB,
  );

  // Step 4: switch back to A — render fires before effect commits (stale B scope).
  assert.deepEqual(
    renderFence(
      stateB.threadActivityScopeRef,
      stateB.threadActivityItems,
      pubkey,
      groupA,
    ),
    [],
    "B rows must be hidden on A transition render",
  );

  // Step 5: A reset effect commits — A rows return.
  const stateA2 = makeScopeState({
    pubkey,
    groupId: groupA,
    loadedItems: itemsA,
  });
  assert.deepEqual(
    renderFence(
      stateA2.threadActivityScopeRef,
      stateA2.threadActivityItems,
      pubkey,
      groupA,
    ),
    itemsA,
    "A rows must return when A's reset effect commits again",
  );
});

test("scope-transition: empty pubkey produces empty fence regardless of group", () => {
  // When currentActivityScope is "" (no pubkey), the hook never loads items
  // into threadActivityRef — the reset effect guards on normalizedPubkey.
  // So scope "" with empty items correctly returns [].
  const state = {
    threadActivityScopeRef: { current: "" },
    threadActivityItems: [],
  };
  const visible = renderFence(
    state.threadActivityScopeRef,
    state.threadActivityItems,
    null,
    "group-a",
  );
  assert.deepEqual(visible, []);
});

// Direct projectActivityForScope tests — these call the production helper
// without going through renderFence, ensuring the helper itself is correct.

test("projectActivityForScope: rejects empty currentScope even if loadedScope is also empty", () => {
  // The ref initializes to "" and writers can fire before the first reset
  // effect commits. "" === "" must never expose items.
  // Use a non-empty items array so the test actually proves rejection
  // (an empty input array would stay green even if the helper returned it).
  const items = [{ id: "x1" }, { id: "x2" }];
  assert.deepEqual(
    projectActivityForScope("", "", items),
    [],
    "empty currentScope must always return [] regardless of items content",
  );
});

test("projectActivityForScope: returns items when both scopes are equal and non-empty", () => {
  const items = [{ id: "x1" }];
  assert.deepEqual(
    projectActivityForScope("pk1:group-a", "pk1:group-a", items),
    items,
  );
});

test("projectActivityForScope: returns [] on scope mismatch", () => {
  const items = [{ id: "x1" }];
  assert.deepEqual(
    projectActivityForScope("pk1:group-a", "pk1:group-b", items),
    [],
  );
});
