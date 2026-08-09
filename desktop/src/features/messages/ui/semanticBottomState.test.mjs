import assert from "node:assert/strict";
import test from "node:test";

import {
  FREEZE_IGNORE_FRAMES,
  initialSemanticBottomState,
  reduceSemanticBottom,
} from "./semanticBottomState.ts";

// ─── Controlled rAF harness ─────────────────────────────────────────────────
// The component defers a semantic-bottom commit through requestAnimationFrame.
// Here that rAF is an explicit `flush` event the test fires on a chosen frame —
// no setTimeout, no real timers, no sleeps. `frame` is the monotonic time
// source the ignore-window arithmetic reads, advanced only by the test.

const vTrue = (frame, bufferHolding = false) => ({
  type: "virtualizer",
  atBottom: true,
  frame,
  bufferHolding,
});
const vFalse = (frame) => ({ type: "virtualizer", atBottom: false, frame });
const flush = (frame) => ({ type: "flush", frame });
const RELEASE = { type: "release" };
const RESET = { type: "channelReset" };

const run = (events, init = initialSemanticBottomState()) =>
  events.reduce(reduceSemanticBottom, init);

// ─── Reviewed (pre-fix) reference ────────────────────────────────────────────
// Faithful encoding of the reviewed MessageTimeline logic, kept here ONLY to
// prove the three regression scenarios would have been red before the fix:
//  • the pendingCount suppress gate (review issue 1): virtualizer(true) is
//    suppressed whenever suppressNext && bufferHolding — identical for a
//    synthetic freeze-shortening and a genuine transient return with pending,
//    so it cannot tell them apart.
//  • the non-authoritative release / channel-reset (review issues 2/3):
//    setIsSemanticallyAtBottom(true) leaves the queued rAF alive, so a stale
//    deferred `false` overwrites the intentional release.
// `semantic` stands in for the committed React state the buffer/pill read.

const reviewedInitial = () => ({
  semantic: true,
  confirmed: false,
  suppressNext: false,
  pending: null,
});

function reduceReviewed(state, event) {
  const bufferHolding = event.bufferHolding ?? false;
  switch (event.type) {
    case "virtualizer": {
      if (event.atBottom) {
        const confirmed = true;
        if (state.suppressNext) {
          if (!bufferHolding) {
            return {
              ...state,
              semantic: true,
              confirmed,
              suppressNext: false,
              pending: true,
            };
          }
          return { ...state, confirmed, suppressNext: false };
        }
        if (!state.semantic) {
          return { ...state, semantic: true, confirmed, pending: true };
        }
        return { ...state, confirmed };
      }
      if (state.confirmed && state.semantic) {
        return { ...state, suppressNext: true, pending: false };
      }
      return state;
    }
    case "flush": {
      if (state.pending === null) return state;
      return { ...state, semantic: state.pending, pending: null };
    }
    case "release": {
      // BUG: synchronous release does not cancel the queued rAF.
      return { ...state, semantic: true };
    }
    case "channelReset": {
      // BUG: channel reset does not cancel the queued rAF.
      return { ...state, semantic: true, confirmed: false };
    }
    default:
      return state;
  }
}

const runReviewed = (events, init = reviewedInitial()) =>
  events.reduce(reduceReviewed, init);

// ===========================================================================
// Scenario 1 — transient resize leave → live arrival → genuine return releases
// ===========================================================================

test("scenario 1 (fixed): a genuine return after a resize transient releases the buffered tail even with pending", () => {
  // Reader confirms bottom, then an inline-video resize briefly flips the
  // virtualizer off-bottom. A live message lands during the freeze window; when
  // the resize settles the virtualizer reports bottom again and the tail MUST
  // release so the buffered row enters the model.
  const commitFrame = 2;
  const out = run([
    vTrue(0), // confirmed at bottom
    vFalse(1), // resize transient leaves bottom → freeze queued
    flush(commitFrame), // freeze commits, ignore window armed
    vTrue(commitFrame + FREEZE_IGNORE_FRAMES + 3, true), // genuine return, msg arrived
    flush(commitFrame + FREEZE_IGNORE_FRAMES + 4),
  ]);
  assert.equal(
    out.semanticAtBottom,
    true,
    "genuine physical return past the ignore window must release pending",
  );
  assert.equal(out.pending, null, "no deferred commit left dangling");
});

test("scenario 1 (reviewed): the pendingCount gate wrongly withholds the buffered tail on a genuine return", () => {
  // Same event stream through the reviewed logic. bufferHolding is true (a
  // message arrived), so the suppress branch fires and the reader is left at
  // the physical floor with messages stuck behind a non-semantic pill.
  const commitFrame = 2;
  const out = runReviewed([
    vTrue(0),
    vFalse(1),
    flush(commitFrame),
    vTrue(commitFrame + FREEZE_IGNORE_FRAMES + 3, true),
    flush(commitFrame + FREEZE_IGNORE_FRAMES + 4),
  ]);
  assert.equal(
    out.semantic,
    false,
    "reviewed gate cannot distinguish synthetic from genuine when pending > 0",
  );
});

// ===========================================================================
// Scenario 2 — queued false on channel A cannot overwrite channel B's reset
// ===========================================================================

test("scenario 2 (fixed): a channel reset discards a queued freeze so the stale rAF is a no-op on the new channel", () => {
  const out = run([
    vTrue(0), // A: confirmed at bottom
    vFalse(1), // A: reader scrolled up → freeze queued
    RESET, // switch to B: authoritative reset (semantic true, pending cleared)
    flush(3), // the stale rAF from A fires — must not re-freeze B
  ]);
  assert.equal(out.semanticAtBottom, true, "channel B stays released");
  assert.equal(out.confirmedBottom, false, "B has not confirmed bottom yet");
  assert.equal(out.pending, null);
});

test("scenario 2 (reviewed): a stale queued false overwrites the new channel's reset", () => {
  const out = runReviewed([
    vTrue(0),
    vFalse(1),
    RESET, // sets semantic=true but leaves the queued false rAF alive
    flush(3), // stale rAF commits false on channel B
  ]);
  assert.equal(
    out.semantic,
    false,
    "reviewed reset does not cancel the rAF, so B is re-frozen",
  );
});

// ===========================================================================
// Scenario 3 — jump / own-message synchronous release vs a stale rAF
// ===========================================================================

test("scenario 3 (fixed): an own-message release cancels the queued freeze so a stale rAF cannot re-freeze", () => {
  const out = run([
    vTrue(0), // confirmed at bottom
    vFalse(1), // reader drifted off bottom → freeze queued
    RELEASE, // own send / Jump-to-latest: authoritative synchronous release
    flush(3), // stale rAF fires — must not overwrite the release
  ]);
  assert.equal(out.semanticAtBottom, true, "release survives the stale rAF");
  assert.equal(out.pending, null);
});

test("scenario 3 (reviewed): a stale queued false overwrites the synchronous release", () => {
  const out = runReviewed([
    vTrue(0),
    vFalse(1),
    RELEASE, // sets semantic=true but leaves the queued false rAF alive
    flush(3),
  ]);
  assert.equal(
    out.semantic,
    false,
    "reviewed release does not cancel the rAF, so the send re-freezes",
  );
});

// ===========================================================================
// Preservation — genuine freeze and synthetic suppression still hold
// ===========================================================================

test("preservation A (fixed): a genuine scroll-up keeps the tail frozen across arrivals", () => {
  // Reader leaves bottom and never returns; live arrivals must buffer behind the
  // pill, not mutate Virtua's model under the reading position.
  const out = run([
    vTrue(0),
    vFalse(1),
    flush(2), // frozen
    vFalse(5), // still reading up the history — redundant leave is a no-op
    flush(10),
  ]);
  assert.equal(
    out.semanticAtBottom,
    false,
    "tail stays frozen while scrolled up",
  );
});

test("preservation B (fixed): the immediate synthetic post-freeze bottom is suppressed", () => {
  // Freezing shortens Virtua's model, which can make the current offset report
  // "at bottom" on the very next frame without the reader moving. That synthetic
  // emission (inside the ignore window) must not release.
  const out = run([
    vTrue(0),
    vFalse(1),
    flush(2), // ignore window armed at frame 2 + FREEZE_IGNORE_FRAMES
    vTrue(2 + FREEZE_IGNORE_FRAMES - 1, true), // synthetic, within window
    flush(3),
  ]);
  assert.equal(
    out.semanticAtBottom,
    false,
    "synthetic freeze-shortening bottom is suppressed",
  );
});

test("preservation (reviewed): current logic still freezes and suppresses the synthetic case", () => {
  // The reviewed logic is not globally broken — only the three coordination
  // scenarios above fail. A plain scroll-up freeze, and a synthetic bottom with
  // the buffer holding, both behave correctly. This scopes the regression.
  const frozen = runReviewed([vTrue(0), vFalse(1), flush(2), flush(10)]);
  assert.equal(frozen.semantic, false, "reviewed still freezes on scroll-up");

  const suppressed = runReviewed([
    vTrue(0),
    vFalse(1),
    flush(2),
    vTrue(3, true), // synthetic, buffer holding → suppress branch fires
    flush(4),
  ]);
  assert.equal(
    suppressed.semantic,
    false,
    "reviewed still suppresses the synthetic bottom when holding",
  );
});

// ===========================================================================
// Ignore-window boundary — controlled rAF precision (no sleeps)
// ===========================================================================

test("ignore window: the frame before the boundary suppresses, the boundary frame releases", () => {
  const commitFrame = 10;
  const boundary = commitFrame + FREEZE_IGNORE_FRAMES;

  const suppressed = run([
    vTrue(0),
    vFalse(1),
    flush(commitFrame),
    vTrue(boundary - 1), // one frame inside the window → synthetic
  ]);
  assert.equal(
    suppressed.semanticAtBottom,
    false,
    `frame ${boundary - 1} (< ${boundary}) is a synthetic artifact`,
  );

  const released = run([
    vTrue(0),
    vFalse(1),
    flush(commitFrame),
    vTrue(boundary), // first frame at/after the boundary → genuine return
  ]);
  assert.equal(
    released.semanticAtBottom,
    true,
    `frame ${boundary} (>= ${boundary}) is a genuine return`,
  );
});

test("after a synthetic is absorbed, a later genuine return still releases", () => {
  // The ignore window is one-shot: consuming the synthetic does not trap the
  // reader frozen forever.
  const out = run([
    vTrue(0),
    vFalse(1),
    flush(2),
    vTrue(2 + FREEZE_IGNORE_FRAMES - 1), // synthetic absorbed, window cleared
    vTrue(2 + FREEZE_IGNORE_FRAMES + 5), // genuine return later
  ]);
  assert.equal(out.semanticAtBottom, true);
});

test("a resize-bounce that returns to bottom before the freeze commits does not freeze", () => {
  // virtualizer(false) queues a deferred false; a virtualizer(true) arriving
  // before the flush cancels that queue (any bottom report drops `pending`),
  // so the later flush is a no-op and the tail never freezes. Without the
  // cancellation this regresses the old eager-ref behaviour, where the
  // pre-flush return cancelled the queued freeze.
  const out = run([
    vTrue(0), // confirmed at bottom
    vFalse(1), // resize dip → freeze queued (pending: false)
    vTrue(2), // bounce back to bottom, pre-flush → cancels the queue
    flush(3), // nothing deferred → no-op
  ]);
  assert.equal(out.semanticAtBottom, true, "bounce-back must not freeze");
  assert.equal(out.pending, null);
  assert.equal(out.ignoreUntilFrame, 0, "no freeze-settle window armed");
});

// ===========================================================================
// Mount-transient guard + idempotency
// ===========================================================================

test("a leave before the channel has confirmed bottom is ignored (mount convergence)", () => {
  const out = run([vFalse(0), flush(1)]);
  assert.equal(out.semanticAtBottom, true);
  assert.equal(out.confirmedBottom, false);
  assert.equal(out.pending, null, "no freeze queued during mount convergence");
});

test("a redundant at-bottom report is idempotent", () => {
  const a = run([vTrue(0)]);
  const b = run([vTrue(0), vTrue(1), vTrue(2)]);
  assert.deepEqual(b, { ...a, ignoreUntilFrame: 0 });
});
