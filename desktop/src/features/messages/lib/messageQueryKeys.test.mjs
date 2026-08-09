import assert from "node:assert/strict";
import test, { beforeEach, afterEach } from "node:test";

import {
  channelMessagesKey,
  channelWindowKey,
  mergeTimelineHistoryMessages,
  normalizeTimelineMessages,
  threadRepliesKey,
} from "./messageQueryKeys.ts";
import {
  setResolvedHistoryScope,
  clearAllResolvedHistoryScopes,
} from "./nativeHistoryScopeStore.ts";

// The cache-key functions fold the resolved durable-history scope into every
// history key, so resolving/rotating a scope yields a fresh partition. Each
// key-partition test starts from a clean registry.
beforeEach(() => clearAllResolvedHistoryScopes());
afterEach(() => clearAllResolvedHistoryScopes());

const CHANNEL_ID = "timeline-window-test";
const PUBKEY = "a".repeat(64);

function event({ id, kind = 9, createdAt, tags, content = "" }) {
  return {
    id,
    pubkey: PUBKEY,
    created_at: createdAt,
    kind,
    tags: tags ?? [["h", CHANNEL_ID]],
    content,
    sig: "mocksig".repeat(20).slice(0, 128),
  };
}

function id(prefix, index) {
  return `${prefix}${String(index).padStart(64 - prefix.length, "0")}`;
}

test("normalizeTimelineMessages preserves the complete loaded window", () => {
  const messages = [];
  for (let index = 0; index < 2_100; index += 1) {
    messages.push(event({ id: id("row", index), createdAt: 1_000 + index }));
  }
  messages.push(
    event({
      id: id("aux", 0),
      kind: 7,
      createdAt: 4_000,
      tags: [
        ["h", CHANNEL_ID],
        ["e", id("row", 0)],
      ],
      content: "+",
    }),
  );

  const normalized = normalizeTimelineMessages(messages);

  assert.equal(normalized.filter((item) => item.kind === 9).length, 2_100);
  assert.equal(
    normalized.some((item) => item.id === id("row", 0)),
    true,
  );
  assert.equal(
    normalized.some((item) => item.id === id("aux", 0)),
    true,
  );
});

test("timeline history merge preserves freshly fetched older content roots", () => {
  const current = [];
  const olderPage = [];

  for (let index = 0; index < 2_000; index += 1) {
    current.push(event({ id: id("new", index), createdAt: 10_000 + index }));
  }
  for (let index = 0; index < 100; index += 1) {
    olderPage.push(event({ id: id("old", index), createdAt: 1_000 + index }));
  }

  const merged = mergeTimelineHistoryMessages(current, olderPage);
  const mergedContent = merged
    .filter((item) => item.kind === 9)
    .map((item) => item.id);

  assert.equal(mergedContent.length, 2_100);
  assert.equal(mergedContent[0], id("old", 0));
  assert.equal(mergedContent[99], id("old", 99));
  assert.equal(mergedContent[100], id("new", 0));
  assert.equal(mergedContent.at(-1), id("new", 1_999));
});

test("timeline history merge preserves the older window despite auxiliary events", () => {
  const seedMessages = [];
  const olderPage = [];

  for (let index = 0; index < 700; index += 1) {
    seedMessages.push(
      event({ id: id("new", index), createdAt: 10_000 + index }),
    );
  }
  for (let index = 0; index < 1_303; index += 1) {
    seedMessages.push(
      event({
        id: id("del", index),
        kind: 5,
        createdAt: 11_000 + index,
        tags: [
          ["h", CHANNEL_ID],
          ["e", id("zzz", index)],
        ],
      }),
    );
  }
  for (let index = 0; index < 231; index += 1) {
    seedMessages.push(
      event({
        id: id("rea", index),
        kind: 7,
        createdAt: 13_000 + index,
        tags: [
          ["h", CHANNEL_ID],
          ["e", id("yyy", index)],
        ],
        content: "+",
      }),
    );
  }
  for (let index = 0; index < 1_500; index += 1) {
    olderPage.push(event({ id: id("old", index), createdAt: 1_000 + index }));
  }

  const merged = mergeTimelineHistoryMessages(seedMessages, olderPage);
  const mergedContent = merged
    .filter((item) => item.kind === 9)
    .map((item) => item.id);

  assert.equal(mergedContent.length, 2_200);
  assert.equal(mergedContent[0], id("old", 0));
  assert.equal(mergedContent[1_499], id("old", 1_499));
  assert.equal(mergedContent[1_500], id("new", 0));
  assert.equal(mergedContent.at(-1), id("new", 699));
  assert.equal(merged.filter((item) => item.kind === 5).length, 1_303);
  assert.equal(merged.filter((item) => item.kind === 7).length, 231);
});

test("sortMessages tiebreaks same-second events on id, order-independent", () => {
  // Three events sharing one created_at, fed in two different input orders.
  // The (created_at, id) sort must produce the same sequence both ways, so a
  // history-then-live merge and a live-then-history merge can't shuffle a
  // same-second message to a different visible position.
  const a = event({ id: id("aaa", 1), createdAt: 5_000 });
  const b = event({ id: id("bbb", 1), createdAt: 5_000 });
  const c = event({ id: id("ccc", 1), createdAt: 5_000 });

  const forward = normalizeTimelineMessages([a, b, c]).map((m) => m.id);
  const reverse = normalizeTimelineMessages([c, b, a]).map((m) => m.id);

  assert.deepEqual(forward, reverse);
  assert.deepEqual(forward, [a.id, b.id, c.id]);
});

// ── Resolved-scope cache-key partition ──────────────────────────────────────
// Every history cache key carries the channel's resolved durable-history scope
// as its partition axis. A held group (unresolved -> null), a scope arrival
// (null -> stable), and a rotation (stable -> rotated) each yield a distinct
// partition, so prior/pending data is never displayed after the scope changes
// and the old partition is orphaned for GC. DM channel ids are never registered
// so they keep a stable null partition.

test("an unresolved group's keys carry a null scope partition (stable while held)", () => {
  assert.deepEqual(channelMessagesKey("g-1"), [
    "channel-messages",
    "g-1",
    null,
  ]);
  assert.deepEqual(channelWindowKey("g-1"), ["channel-window", "g-1", null]);
  assert.deepEqual(threadRepliesKey("g-1", "root-a"), [
    "thread-replies",
    "g-1",
    "root-a",
    null,
  ]);
});

test("scope arrival (null -> stable) yields a fresh partition for every history key", () => {
  const beforeMsg = channelMessagesKey("g-1");
  const beforeWin = channelWindowKey("g-1");
  const beforeThread = threadRepliesKey("g-1", "root-a");
  setResolvedHistoryScope("g-1", "group:stable-1");
  assert.notDeepEqual(channelMessagesKey("g-1"), beforeMsg);
  assert.notDeepEqual(channelWindowKey("g-1"), beforeWin);
  assert.notDeepEqual(threadRepliesKey("g-1", "root-a"), beforeThread);
  // The resolved scope is the partition value actually carried.
  assert.deepEqual(channelMessagesKey("g-1"), [
    "channel-messages",
    "g-1",
    "group:stable-1",
  ]);
});

test("scope rotation (stable -> rotated) yields a fresh partition, orphaning the old for GC", () => {
  setResolvedHistoryScope("g-1", "group:stable-1");
  const first = channelMessagesKey("g-1");
  setResolvedHistoryScope("g-1", "group:stable-1-rotated");
  assert.notDeepEqual(channelMessagesKey("g-1"), first);
  assert.deepEqual(channelMessagesKey("g-1"), [
    "channel-messages",
    "g-1",
    "group:stable-1-rotated",
  ]);
});

test("a DM channel id keeps a stable null partition (DMs are never registered)", () => {
  const dmPeer = "c".repeat(64);
  // DM scopes are deterministic and never enter the registry, so the DM key is
  // a stable null partition — it does not churn on any group scope change.
  assert.deepEqual(channelMessagesKey(dmPeer)[2], null);
  setResolvedHistoryScope("g-1", "group:stable-1");
  assert.deepEqual(channelMessagesKey(dmPeer)[2], null);
});

test("per-channel isolation: each channel's key carries its own scope, and rotating one never moves another", () => {
  setResolvedHistoryScope("g-a", "group:stable-a");
  setResolvedHistoryScope("g-b", "group:stable-b");
  assert.deepEqual(channelMessagesKey("g-a")[2], "group:stable-a");
  assert.deepEqual(channelMessagesKey("g-b")[2], "group:stable-b");
  assert.notDeepEqual(channelMessagesKey("g-a"), channelMessagesKey("g-b"));
  // Rotating g-a's scope does not touch g-b's partition.
  const bBefore = channelMessagesKey("g-b");
  setResolvedHistoryScope("g-a", "group:stable-a-rotated");
  assert.deepEqual(channelMessagesKey("g-b"), bBefore);
});

test("the three key families are distinct and cannot collide for one channel", () => {
  setResolvedHistoryScope("g-1", "group:stable-1");
  const msg = channelMessagesKey("g-1");
  const win = channelWindowKey("g-1");
  const thread = threadRepliesKey("g-1", "root-a");
  assert.notDeepEqual(msg, win);
  assert.notDeepEqual(msg, thread);
  assert.notDeepEqual(win, thread);
});

test("threadRepliesKey is partitioned by root id, independently of scope", () => {
  setResolvedHistoryScope("g-1", "group:stable-1");
  assert.notDeepEqual(
    threadRepliesKey("g-1", "root-a"),
    threadRepliesKey("g-1", "root-b"),
  );
});

test("one channel's scope rotation repartitions all three of its key families at once", () => {
  setResolvedHistoryScope("g-1", "group:stable-1");
  const msg = channelMessagesKey("g-1");
  const win = channelWindowKey("g-1");
  const thread = threadRepliesKey("g-1", "root-a");
  setResolvedHistoryScope("g-1", "group:rotated");
  assert.notDeepEqual(channelMessagesKey("g-1"), msg);
  assert.notDeepEqual(channelWindowKey("g-1"), win);
  assert.notDeepEqual(threadRepliesKey("g-1", "root-a"), thread);
});
