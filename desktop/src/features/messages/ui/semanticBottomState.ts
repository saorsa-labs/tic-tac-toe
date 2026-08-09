/**
 * Pure transition model for MessageTimeline's semantic-bottom ("Zulip-style
 * tail freeze") state.
 *
 * Extracted as a dependency-free reducer so the rAF/ref coordination around
 * freeze recovery is unit-testable with controlled frames instead of real
 * timers. MessageTimeline mirrors this machine with refs + rAF; the three
 * behaviours this model pins down (and that the inline hook must match) are:
 *
 *  1. A transient content-size leave (inline-video resize, thread-summary
 *     growth) followed by a live arrival and a genuine physical return must
 *     RELEASE the buffered tail. `pendingCount` alone cannot decide that — it
 *     is non-zero for both a real return and a synthetic freeze-shortening — so
 *     a short post-freeze ignore window distinguishes the immediate synthetic
 *     emission from a later genuine return.
 *  2. A queued `false` on channel A must not overwrite channel B's reset.
 *  3. A synchronous release (Jump-to-latest / own message) must not be
 *     overwritten by a stale deferred commit.
 *
 * `release` and `channelReset` are the authoritative synchronous paths: they
 * clear any pending deferred commit, so a subsequent `flush` is a no-op — the
 * reducer-level equivalent of cancelling the outstanding requestAnimationFrame.
 */

/** Number of frames after a freeze commits during which a `virtualizer(true)`
 *  is treated as a synthetic artifact of the freeze shortening the model.
 *  Mirrors the component's double-rAF settle window. */
export const FREEZE_IGNORE_FRAMES = 2;

export type SemanticBottomState = {
  /** True while the virtualizer's logical tail tracks live output. While false,
   *  new arrivals buffer behind the "new messages" affordance. */
  semanticAtBottom: boolean;
  /** True once the virtualizer has reported bottom at least once this channel.
   *  Guards against treating Virtua's mount-convergence transient as a leave. */
  confirmedBottom: boolean;
  /** A `virtualizer(true)` arriving on a frame strictly before this value is a
   *  synthetic post-freeze artifact and is ignored. 0 = no window armed. Armed
   *  when a freeze *commits* (the deferred false lands on a flush), at
   *  `commitFrame + FREEZE_IGNORE_FRAMES`. */
  ignoreUntilFrame: number;
  /** A deferred commit queued for the next flush, or null when none is pending.
   *  `false` = a freeze commit is queued; `true` is unreachable in practice but
   *  kept for the union. null = no deferred commit (a later flush is a no-op). */
  pending: boolean | null;
};

export type SemanticBottomEvent =
  | { type: "virtualizer"; atBottom: boolean; frame: number }
  | { type: "flush"; frame: number }
  | { type: "release" }
  | { type: "channelReset" };

export function initialSemanticBottomState(): SemanticBottomState {
  return {
    semanticAtBottom: true,
    confirmedBottom: false,
    ignoreUntilFrame: 0,
    pending: null,
  };
}

export function reduceSemanticBottom(
  state: SemanticBottomState,
  event: SemanticBottomEvent,
): SemanticBottomState {
  switch (event.type) {
    case "virtualizer": {
      if (event.atBottom) {
        const confirmedBottom = true;
        if (
          state.ignoreUntilFrame > 0 &&
          event.frame < state.ignoreUntilFrame
        ) {
          // Synthetic post-freeze emission: freezing shortened Virtua's model
          // and it re-reported bottom without the reader moving. Do not
          // release; consume the one-shot ignore window. `pending` is already
          // null here (the window is armed only when the freeze commits on a
          // flush), but every bottom report cancels a queued freeze, so clear
          // it for consistency with the other bottom branches.
          return {
            ...state,
            confirmedBottom,
            ignoreUntilFrame: 0,
            pending: null,
          };
        }
        if (!state.semanticAtBottom) {
          // Genuine physical return past the ignore window: release the tail.
          return {
            ...state,
            semanticAtBottom: true,
            confirmedBottom,
            ignoreUntilFrame: 0,
            pending: null,
          };
        }
        // Already at bottom. A queued freeze — e.g. a resize-bounce that
        // returned to bottom before the deferred false-commit landed — is
        // cancelled so the later flush does not freeze.
        return { ...state, confirmedBottom, pending: null };
      }
      // Left the bottom. Ignore Virtua's mount-convergence transient until this
      // channel has confirmed bottom once, and ignore redundant leaves while the
      // tail is already frozen.
      if (state.confirmedBottom && state.semanticAtBottom) {
        return { ...state, pending: false };
      }
      return state;
    }
    case "flush": {
      if (state.pending === null) {
        // Nothing deferred — a stale rAF that fired after an authoritative
        // release/channelReset lands here as a no-op.
        return state;
      }
      const committed = state.pending;
      return {
        ...state,
        semanticAtBottom: committed,
        // Arm the ignore window only when freezing, so the immediate synthetic
        // post-freeze emission is absorbed. A deferred `true` (unreachable)
        // needs no window.
        ignoreUntilFrame:
          committed === false ? event.frame + FREEZE_IGNORE_FRAMES : 0,
        pending: null,
      };
    }
    case "release": {
      // Authoritative synchronous release (Jump-to-latest / own message).
      // Cancels any pending deferred commit so a stale rAF cannot re-freeze.
      return {
        ...state,
        semanticAtBottom: true,
        ignoreUntilFrame: 0,
        pending: null,
      };
    }
    case "channelReset": {
      // Channel switch resets to the same contract as a fresh mount: the new
      // channel has not yet confirmed bottom, and any deferred commit from the
      // previous channel is discarded.
      return initialSemanticBottomState();
    }
    default: {
      return state;
    }
  }
}
