/**
 * Resolved durable-history scope registry contract.
 *
 * The x0xd daemon binds a group's REST durable history to its *stable* group
 * id, surfaced as `historyScope` on `subscribeX0xLive`. That stable id may
 * differ from the transient id used for the live WS backfill (the id
 * `nativeScopeForChannel` derives from `channel.id`). Every durable-history
 * REST consumer targets the scope captured here — never the transient id — so
 * this registry is the single load-bearing seam that prevents a divergent-id
 * group from cold-loading, paging, searching, or threading against the wrong
 * scope (silent wrong/empty results, or cross-channel leakage).
 *
 * Contracts defended (black-box through the exported API only):
 * - a daemon-resolved scope that DIFFERS from the REST channel id is stored
 *   and returned verbatim (never normalized to `group:<id>`);
 * - malformed/empty daemon values are rejected so a bad scope is never cached;
 * - scope data is isolated per channel and cleared on roster/identity reset;
 * - store changes notify every subscriber (the re-render trigger held
 *   consumers like the draft-root hook listen for), without notifying on a
 *   no-op re-set.
 */
import assert from "node:assert/strict";
import test, { beforeEach, afterEach } from "node:test";

const {
  setResolvedHistoryScope,
  getResolvedHistoryScope,
  clearResolvedHistoryScope,
  clearAllResolvedHistoryScopes,
  subscribeHistoryScope,
} = await import("./nativeHistoryScopeStore.ts");

beforeEach(() => clearAllResolvedHistoryScopes());
afterEach(() => clearAllResolvedHistoryScopes());

// ── Divergent daemon scope stored verbatim ──────────────────────────────────

test("a daemon-resolved scope that differs from the REST channel id is returned verbatim, never normalized to group:<id>", () => {
  // REST channel id is the transient "transient-7"; the daemon resolved the
  // stable id "group:stable-abc". Consumers must target the latter or they
  // load the wrong history / leak across channels.
  setResolvedHistoryScope("transient-7", "group:stable-abc");
  const scope = getResolvedHistoryScope("transient-7");
  assert.equal(scope, "group:stable-abc");
  assert.notEqual(scope, "group:transient-7");
});

// ── Malformed / empty daemon values rejected ────────────────────────────────

test("a malformed daemon historyScope is rejected so the channel stays unresolved until a well-formed one arrives", () => {
  for (const bad of [
    "not-a-scope",
    "group",
    "group:",
    "GROUP:upper",
    "dm",
    "x:y",
  ]) {
    setResolvedHistoryScope("c-bad", bad);
    assert.equal(
      getResolvedHistoryScope("c-bad"),
      null,
      `malformed scope "${bad}" must not be cached`,
    );
  }
  // The same channel still accepts a well-formed scope afterwards.
  setResolvedHistoryScope("c-bad", "group:good");
  assert.equal(getResolvedHistoryScope("c-bad"), "group:good");
});

test("a null/empty historyScope never populates the registry", () => {
  for (const empty of [null, undefined, ""]) {
    setResolvedHistoryScope("c-empty", empty);
    assert.equal(getResolvedHistoryScope("c-empty"), null);
  }
});

// ── Per-channel isolation ───────────────────────────────────────────────────

test("scopes are isolated per channel: one channel's scope never shadows another or leaks to an unknown channel", () => {
  setResolvedHistoryScope("channel-a", "group:stable-a");
  setResolvedHistoryScope("channel-b", "group:stable-b");
  assert.equal(getResolvedHistoryScope("channel-a"), "group:stable-a");
  assert.equal(getResolvedHistoryScope("channel-b"), "group:stable-b");
  // A never-set channel is unresolved even while others are populated — scope
  // data does not bleed across keys.
  assert.equal(getResolvedHistoryScope("channel-c"), null);
});

// ── Reset semantics (roster leave / identity-session transition) ─────────────

test("clearResolvedHistoryScope drops one channel without touching the others", () => {
  setResolvedHistoryScope("channel-a", "group:stable-a");
  setResolvedHistoryScope("channel-b", "group:stable-b");
  clearResolvedHistoryScope("channel-a");
  assert.equal(getResolvedHistoryScope("channel-a"), null);
  assert.equal(getResolvedHistoryScope("channel-b"), "group:stable-b");
});

test("clearResolvedHistoryScope on an unknown channel does not wipe populated channels", () => {
  setResolvedHistoryScope("channel-a", "group:stable-a");
  clearResolvedHistoryScope("never-set");
  assert.equal(getResolvedHistoryScope("channel-a"), "group:stable-a");
});

test("clearAllResolvedHistoryScopes wipes every channel (identity/session reset)", () => {
  setResolvedHistoryScope("channel-a", "group:stable-a");
  setResolvedHistoryScope("channel-b", "topic:t-b");
  clearAllResolvedHistoryScopes();
  assert.equal(getResolvedHistoryScope("channel-a"), null);
  assert.equal(getResolvedHistoryScope("channel-b"), null);
});

// ── Reactivity: held consumers re-evaluate on real scope changes ─────────────

test("a subscriber is notified on set, per-channel clear, and bulk clear", () => {
  const events = [];
  const unsub = subscribeHistoryScope(() => events.push("change"));
  setResolvedHistoryScope("channel-a", "group:stable-a");
  setResolvedHistoryScope("channel-b", "group:stable-b");
  clearResolvedHistoryScope("channel-a");
  clearAllResolvedHistoryScopes();
  unsub();
  assert.equal(events.length, 4);
});

test("every subscriber is notified (a change fans out to all held consumers)", () => {
  const a = [];
  const b = [];
  const unsubA = subscribeHistoryScope(() => a.push("x"));
  const unsubB = subscribeHistoryScope(() => b.push("x"));
  setResolvedHistoryScope("channel-a", "group:stable-a");
  unsubA();
  unsubB();
  assert.equal(a.length, 1);
  assert.equal(b.length, 1);
});

test("a no-op re-set of the identical scope does not notify (prevents consumer thrash)", () => {
  const events = [];
  const unsub = subscribeHistoryScope(() => events.push("change"));
  setResolvedHistoryScope("channel-a", "group:stable-a");
  setResolvedHistoryScope("channel-a", "group:stable-a"); // identical — no-op
  unsub();
  assert.equal(events.length, 1);
});

test("a changed value on the same channel notifies (a rotated stable id propagates)", () => {
  const events = [];
  const unsub = subscribeHistoryScope(() => events.push("change"));
  setResolvedHistoryScope("channel-a", "group:stable-a");
  setResolvedHistoryScope("channel-a", "group:stable-a-rotated");
  unsub();
  assert.equal(events.length, 2);
});

test("unsubscribe stops further notifications", () => {
  const events = [];
  const unsub = subscribeHistoryScope(() => events.push("change"));
  unsub();
  setResolvedHistoryScope("channel-a", "group:stable-a");
  assert.equal(events.length, 0);
});
