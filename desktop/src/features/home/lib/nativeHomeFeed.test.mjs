/**
 * Native x0x Home-feed derivation (M3 cutover).
 *
 * Exercises `buildNativeHomeFeed` end-to-end through the REAL production
 * pipeline — `nativeScopeForChannel`, `x0xHistoryList`, and
 * `historyRowToRelayEvent` all run unmodified; only the Tauri transport
 * boundary (`window.__TAURI_INTERNALS__.invoke`) is intercepted, the same seam
 * the sibling `tauriNativeX0x.*.test.mjs` suites use. This keeps the tests
 * black-box over the public function while still driving the real native
 * decode/scope/classification logic.
 *
 * Contracts defended:
 * - mention-vs-activity classification (envelope `mentions` + authorship)
 * - archived/DM scope handling (projection skips, never native errors)
 * - bounded native history queries (one per channel, recency-ordered, capped)
 * - native error propagation (daemon rejection surfaces, no relay fallback)
 * - honest empty buckets (needsAction/agentActivity always empty; caps newest-first)
 */
import assert from "node:assert/strict";
import test, { beforeEach } from "node:test";

// ── Tauri transport shim ────────────────────────────────────────────────────
// `invokeTauri` routes through `window.__TAURI_INTERNALS__.invoke`; intercepting
// it exercises the real `x0xHistoryList` binding (command + camelCase args +
// daemon-row mapping) without a browser or daemon.
const calls = [];
let responseFor = null; // (cmd, args) => value | throws

globalThis.window = globalThis.window ?? {};
globalThis.window.__TAURI_INTERNALS__ = {
  invoke: async (cmd, args) => {
    calls.push({ cmd, args });
    if (responseFor) return responseFor(cmd, args);
    return null;
  },
  transformCallback: () => 0,
  unregisterCallback: () => {},
};

function setResponse(fn) {
  responseFor = fn;
  calls.length = 0;
}

const { buildNativeHomeFeed } = await import("./nativeHomeFeed.ts");

const { setResolvedHistoryScope, clearAllResolvedHistoryScopes } = await import(
  "@/features/messages/lib/nativeHistoryScopeStore.ts"
);

// A group's durable history is only queried once its daemon-resolved stable
// scope is known. The default fixture channel ("group-1") has its scope
// resolved by default; tests that use other channel ids resolve them
// explicitly. Every test starts from a clean registry.
beforeEach(() => {
  clearAllResolvedHistoryScopes();
  setResolvedHistoryScope("group-1", "group:group-1");
});

/** Register a daemon-resolved group scope (default: the transient group:<id>). */
function resolveGroup(channelId, scope = `group:${channelId}`) {
  setResolvedHistoryScope(channelId, scope);
}

// ── Fixtures ────────────────────────────────────────────────────────────────

const ME = "ab".repeat(32); // current agent AgentId
const OTHER = "cd".repeat(32);

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
    // Distinct ISO timestamps make recency ordering deterministic.
    lastMessageAt: "2024-01-01T00:00:00.000Z",
    archivedAt: null,
    participants: [],
    participantPubkeys: [],
    isMember: true,
    ttlSeconds: null,
    ttlDeadline: null,
    ...overrides,
  };
}

/** base64 envelope payload the daemon stores / the adapter decodes. */
function payload(
  text,
  { clientId = "client-1", createdAt = 1_700_000_000_000, mentions } = {},
) {
  const envelope = { text, clientId, createdAt };
  if (mentions) envelope.mentions = mentions;
  return btoa(JSON.stringify(envelope));
}

/** Raw snake_case row as x0xd emits it (history.rs row_json). */
function rawRow(overrides = {}) {
  return {
    id: 1,
    msg_id: "aa".repeat(32),
    scope: "group:group-1",
    author_agent: OTHER,
    author_machine: null,
    sent_at_ms: 1_700_000_000_000,
    seen_at_ms: 1_700_000_001_000,
    direction: "Inbound",
    content_type: "text/plain",
    payload: payload("hello"),
    signed: true,
    provenance: "VerifiedEnvelope",
    replace_key: null,
    thread_root: null,
    thread_parent: null,
    ...overrides,
  };
}

function emptyPage() {
  return { rows: [], has_more: false };
}

// ── Mention-vs-activity classification ──────────────────────────────────────

test("a row that mentions the current agent is classified as a mention", async () => {
  setResponse(() => ({
    rows: [rawRow({ payload: payload("hi @me", { mentions: [ME] }) })],
    has_more: false,
  }));

  const result = await buildNativeHomeFeed({
    channels: [channel()],
    currentAgentId: ME,
  });

  assert.deepEqual(
    result.feed.mentions.map((item) => item.category),
    ["mention"],
  );
  assert.equal(result.feed.activity.length, 0);
});

test("a row with no mention of the current agent is classified as activity", async () => {
  setResponse(() => ({ rows: [rawRow()], has_more: false }));

  const result = await buildNativeHomeFeed({
    channels: [channel()],
    currentAgentId: ME,
  });

  assert.equal(result.feed.mentions.length, 0);
  assert.deepEqual(
    result.feed.activity.map((item) => item.category),
    ["activity"],
  );
});

test("a row authored by the current agent is never a self-mention", async () => {
  // The author lists itself in mentions; classification must still be activity.
  setResponse(() => ({
    rows: [
      rawRow({
        author_agent: ME,
        payload: payload("note to self", { mentions: [ME] }),
      }),
    ],
    has_more: false,
  }));

  const result = await buildNativeHomeFeed({
    channels: [channel()],
    currentAgentId: ME,
  });

  assert.equal(result.feed.mentions.length, 0);
  assert.equal(result.feed.activity.length, 1);
});

test("mention matching is case-insensitive on the agent id", async () => {
  setResponse(() => ({
    rows: [
      rawRow({ payload: payload("hi", { mentions: [ME.toUpperCase()] }) }),
    ],
    has_more: false,
  }));

  const result = await buildNativeHomeFeed({
    channels: [channel()],
    currentAgentId: ME.toLowerCase(),
  });

  assert.equal(result.feed.mentions.length, 1);
});

test("no current agent means nothing is ever classified as a mention", async () => {
  setResponse(() => ({ rows: [rawRow()], has_more: false }));

  const result = await buildNativeHomeFeed({ channels: [channel()] });

  assert.equal(result.feed.mentions.length, 0);
  assert.equal(result.feed.activity.length, 1);
});

// ── Archived / DM scope handling ────────────────────────────────────────────

test("archived channels are skipped: no history request, no rows", async () => {
  setResponse(() => ({
    rows: [rawRow({ payload: payload("ghost") })],
    has_more: false,
  }));

  const result = await buildNativeHomeFeed({
    channels: [
      channel({ id: "archived-1", archivedAt: "2024-01-02T00:00:00.000Z" }),
    ],
    currentAgentId: ME,
  });

  assert.equal(
    calls.length,
    0,
    "archived channel must not trigger a native history request",
  );
  assert.equal(result.feed.mentions.length, 0);
  assert.equal(result.feed.activity.length, 0);
});

test("a DM with an unambiguous peer AgentId is queried on its dm scope", async () => {
  const peer = "ef".repeat(32);
  setResponse((_cmd, args) => ({
    rows: [rawRow({ scope: args.scope, payload: payload("dm reply") })],
    has_more: false,
  }));

  const result = await buildNativeHomeFeed({
    channels: [channel({ id: peer, channelType: "dm" })],
    currentAgentId: ME,
  });

  const listCall = calls[0];
  assert.equal(listCall.args.scope, `dm:${peer}`);
  assert.equal(result.feed.activity.length, 1);
});

test("a DM with no resolvable peer AgentId is skipped silently, not an error", async () => {
  // channel id is not an AgentId and there are zero/multiple participant ids.
  setResponse(() => emptyPage());

  const result = await buildNativeHomeFeed({
    channels: [
      channel({ id: "legacy-dm", channelType: "dm", participantPubkeys: [] }),
    ],
    currentAgentId: ME,
  });

  assert.equal(calls.length, 0);
  // No throw — an unresolvable scope is a projection skip, not a native error.
  assert.equal(result.meta.total, 0);
});

// ── Bounded native history queries ──────────────────────────────────────────

test("one bounded request per eligible channel, scoped, with the configured limit", async () => {
  setResponse(() => emptyPage());
  resolveGroup("g-a");
  resolveGroup("g-b");

  await buildNativeHomeFeed({
    channels: [channel({ id: "g-a" }), channel({ id: "g-b" })],
    currentAgentId: ME,
    perChannelLimit: 7,
  });

  // buildNativeHomeFeed issues one history request per channel and nothing else,
  // so `calls` is exactly the set of native history requests it made.
  const listCalls = calls;
  assert.equal(listCalls.length, 2);
  assert.deepEqual(
    listCalls.map((call) => call.args.limit),
    [7, 7],
  );
  assert.ok(
    listCalls.every(
      (call) => call.args.sinceMs === null && call.args.beforeId === null,
    ),
  );
  assert.deepEqual(listCalls.map((call) => call.args.scope).sort(), [
    "group:g-a",
    "group:g-b",
  ]);
});

test("channels are queried most-recently-active first", async () => {
  setResponse(() => emptyPage());
  resolveGroup("oldest");
  resolveGroup("newest");
  resolveGroup("middle");

  await buildNativeHomeFeed({
    channels: [
      channel({ id: "oldest", lastMessageAt: "2024-01-01T00:00:00.000Z" }),
      channel({ id: "newest", lastMessageAt: "2024-03-01T00:00:00.000Z" }),
      channel({ id: "middle", lastMessageAt: "2024-02-01T00:00:00.000Z" }),
    ],
    currentAgentId: ME,
  });

  const scopes = calls.map((call) => call.args.scope);
  assert.deepEqual(scopes, ["group:newest", "group:middle", "group:oldest"]);
});

test("the channel cap drops the stale tail beyond MAX_FEED_CHANNELS (50)", async () => {
  setResponse(() => emptyPage());
  // Resolve each channel's daemon scope so all 51 are eligible; the cap must
  // still drop the least-recently-active one.
  for (let index = 0; index <= 50; index += 1) resolveGroup(`g-${index}`);

  // 51 channels with strictly decreasing recency so g-50 is unambiguously oldest.
  const channels = Array.from({ length: 51 }, (_, index) =>
    channel({
      id: `g-${index}`,
      lastMessageAt: new Date(1_700_000_000_000 - index * 1000).toISOString(),
    }),
  );

  await buildNativeHomeFeed({ channels, currentAgentId: ME });

  const listCalls = calls;
  assert.equal(listCalls.length, 50, "at most 50 channels are queried");
  const queried = new Set(listCalls.map((call) => call.args.scope));
  assert.equal(
    queried.has("group:g-50"),
    false,
    "the least-recently-active channel is dropped by the cap",
  );
});

// ── Native error propagation ────────────────────────────────────────────────

test("a rejected native history request propagates instead of falling back", async () => {
  setResponse(() => {
    throw new Error("daemon history store unavailable");
  });

  await assert.rejects(
    buildNativeHomeFeed({ channels: [channel()], currentAgentId: ME }),
    /daemon history store unavailable/,
  );
});

// ── Honest empty buckets ────────────────────────────────────────────────────

test("needsAction and agentActivity are always empty, even with data present", async () => {
  setResponse(() => ({
    rows: [
      rawRow({ id: 1, payload: payload("hi @me", { mentions: [ME] }) }),
      rawRow({ id: 2, payload: payload("plain") }),
    ],
    has_more: false,
  }));

  const result = await buildNativeHomeFeed({
    channels: [channel()],
    currentAgentId: ME,
  });

  assert.deepEqual(result.feed.needsAction, []);
  assert.deepEqual(result.feed.agentActivity, []);
  assert.ok(result.feed.mentions.length + result.feed.activity.length > 0);
});

test("no eligible channels yields an honest empty feed with total 0 and since 0", async () => {
  setResponse(() => emptyPage());

  const result = await buildNativeHomeFeed({
    channels: [],
    currentAgentId: ME,
  });

  assert.deepEqual(result.feed, {
    mentions: [],
    needsAction: [],
    activity: [],
    agentActivity: [],
  });
  assert.equal(result.meta.total, 0);
  assert.equal(result.meta.since, 0);
});

test("undecodable rows are dropped, leaving empty buckets without throwing", async () => {
  setResponse(() => ({
    rows: [
      rawRow({
        id: 1,
        content_type: "application/json",
        payload: "not-valid-base64{{{",
      }),
      rawRow({ id: 2, payload: btoa("not json") }),
    ],
    has_more: false,
  }));

  const result = await buildNativeHomeFeed({
    channels: [channel()],
    currentAgentId: ME,
  });

  assert.equal(result.feed.mentions.length, 0);
  assert.equal(result.feed.activity.length, 0);
  assert.equal(result.meta.total, 0);
});

test("mention bucket is capped at 30, newest-first", async () => {
  // 40 distinct mention rows with ascending seen_at_ms (newest = highest id).
  const rows = Array.from({ length: 40 }, (_, index) =>
    rawRow({
      id: index + 1,
      msg_id: index.toString(16).padStart(64, "0"),
      seen_at_ms: 1_700_000_000_000 + index,
      payload: payload(`m ${index}`, { mentions: [ME] }),
    }),
  );
  setResponse(() => ({ rows, has_more: false }));

  const result = await buildNativeHomeFeed({
    channels: [channel()],
    currentAgentId: ME,
  });

  assert.equal(result.feed.mentions.length, 30);
  // newest-first: first item is the highest seen_at_ms row.
  assert.equal(
    result.feed.mentions[0].createdAt,
    Math.floor((1_700_000_000_000 + 39) / 1000),
  );
});

test("activity bucket is capped at 80, newest-first", async () => {
  const rows = Array.from({ length: 100 }, (_, index) =>
    rawRow({
      id: index + 1,
      msg_id: index.toString(16).padStart(64, "0"),
      seen_at_ms: 1_700_000_000_000 + index,
      payload: payload(`a ${index}`),
    }),
  );
  setResponse(() => ({ rows, has_more: false }));

  const result = await buildNativeHomeFeed({ channels: [channel()] });

  assert.equal(result.feed.activity.length, 80);
  assert.equal(
    result.feed.activity[0].createdAt,
    Math.floor((1_700_000_000_000 + 99) / 1000),
  );
});

test("meta.since is the oldest createdAt across the capped result", async () => {
  setResponse(() => ({
    rows: [
      rawRow({ id: 1, seen_at_ms: 5_000, payload: payload("old") }),
      rawRow({ id: 2, seen_at_ms: 9_000, payload: payload("new") }),
    ],
    has_more: false,
  }));

  const result = await buildNativeHomeFeed({ channels: [channel()] });

  assert.equal(result.meta.since, 5); // Math.floor(5000/1000)
  assert.equal(result.meta.total, 2);
});

// ── Resolved durable-history scope contract ─────────────────────────────────

test("an unresolved group is skipped: no history request and no fallback to the transient id", async () => {
  setResponse(() => ({
    rows: [rawRow({ payload: payload("ghost") })],
    has_more: false,
  }));

  const result = await buildNativeHomeFeed({
    channels: [channel({ id: "unresolved-group" })],
    currentAgentId: ME,
  });

  assert.equal(calls.length, 0, "an unresolved group must not be queried");
  assert.equal(result.meta.total, 0);
});

test("a group is queried on its daemon-resolved stable scope, which may differ from group:<id>", async () => {
  setResponse((_cmd, args) => ({
    rows: [rawRow({ scope: args.scope, payload: payload("stable row") })],
    has_more: false,
  }));
  resolveGroup("transient-9", "group:stable-9");

  const result = await buildNativeHomeFeed({
    channels: [channel({ id: "transient-9" })],
    currentAgentId: ME,
  });

  assert.equal(calls[0].args.scope, "group:stable-9");
  assert.equal(result.feed.activity.length, 1);
});

test("a DM is queried on its deterministic dm scope even when a group scope is planted in the registry", async () => {
  const peer = "ef".repeat(32);
  // Plant a group scope under the DM channel id — the DM projection must ignore it.
  setResolvedHistoryScope(peer, "group:planted");
  setResponse((_cmd, args) => ({
    rows: [rawRow({ scope: args.scope, payload: payload("dm row") })],
    has_more: false,
  }));

  const result = await buildNativeHomeFeed({
    channels: [channel({ id: peer, channelType: "dm" })],
    currentAgentId: ME,
  });

  assert.equal(calls[0].args.scope, `dm:${peer}`);
  assert.equal(result.feed.activity.length, 1);
});
