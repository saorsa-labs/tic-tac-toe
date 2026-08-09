import * as React from "react";
import type { VListHandle } from "virtua";

const BOTTOM_EPSILON_PX = 1;
const SETTLE_DEADLINE_MS = 250;

export function useVirtualizedBottomSettle(
  hostRef: React.RefObject<HTMLDivElement | null>,
  listRef: React.RefObject<VListHandle | null>,
  itemsLengthRef: React.RefObject<number>,
  onComplete?: () => void,
) {
  const frameRef = React.useRef<number | null>(null);
  const cancel = React.useCallback(() => {
    if (frameRef.current !== null) {
      cancelAnimationFrame(frameRef.current);
      frameRef.current = null;
    }
  }, []);

  React.useLayoutEffect(() => {
    const scroller = hostRef.current?.firstElementChild;
    if (!(scroller instanceof HTMLDivElement)) return;
    const retire = () => cancel();
    scroller.addEventListener("pointerdown", retire, { passive: true });
    scroller.addEventListener("touchstart", retire, { passive: true });
    scroller.addEventListener("wheel", retire, { passive: true });
    window.addEventListener("keydown", retire, true);
    return () => {
      scroller.removeEventListener("pointerdown", retire);
      scroller.removeEventListener("touchstart", retire);
      scroller.removeEventListener("wheel", retire);
      window.removeEventListener("keydown", retire, true);
    };
  }, [cancel, hostRef]);

  const settle = React.useCallback(() => {
    cancel();
    const deadline = performance.now() + SETTLE_DEADLINE_MS;
    let settledFrames = 0;
    let previousHeight = -1;
    const next = () => {
      const scroller = hostRef.current?.firstElementChild;
      const lastIndex = itemsLengthRef.current - 1;
      if (!(scroller instanceof HTMLDivElement) || lastIndex < 0) {
        cancel();
        return;
      }
      listRef.current?.scrollToIndex(lastIndex, { align: "end" });
      const atBottom =
        scroller.scrollHeight - scroller.clientHeight - scroller.scrollTop <=
        BOTTOM_EPSILON_PX;
      settledFrames =
        atBottom && scroller.scrollHeight === previousHeight
          ? settledFrames + 1
          : 0;
      if (settledFrames >= 2 || performance.now() >= deadline) {
        frameRef.current = null;
        // The settle reached the floor (or its deadline). The scroll it
        // performed may not have moved scrollTop (already pinned) and so may
        // not have fired onScroll — re-derive the at-bottom verdict from the
        // now-settled DOM so the affordance cannot stay stale.
        onComplete?.();
        return;
      }
      previousHeight = scroller.scrollHeight;
      frameRef.current = requestAnimationFrame(next);
    };
    next();
  }, [cancel, hostRef, itemsLengthRef, listRef, onComplete]);

  React.useEffect(() => cancel, [cancel]);
  return { cancel, settle };
}
