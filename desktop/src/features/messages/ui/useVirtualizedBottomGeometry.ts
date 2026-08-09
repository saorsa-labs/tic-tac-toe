import * as React from "react";

import { isVirtualizedAtBottom } from "@/features/messages/lib/virtualizedTimelineItems";

/**
 * Owns the at-bottom verdict for the virtualized timeline: a stable callback
 * that re-derives the verdict from settled DOM geometry, plus a ResizeObserver
 * that re-runs it when viewport or bottom-spacer (composer) geometry changes
 * without an offset change.
 *
 * Virtua fires onScroll only on scroll, so a viewport resize or a bottom-spacer
 * / composer resize (the only content whose height changes without a message
 * arrival) would otherwise leave the affordance stale: DOM distance below the
 * threshold while the last onScroll reported `false`. Observing the scroller
 * (viewport) and the spacer (composer height) closes that gap; the observer
 * callback recomputes only on an actual resize, so it never adds a layout read
 * to a prepend or scroll pass. Re-running on item count re-observes a spacer
 * that mounts after the virtualizer converges.
 *
 * Returns the stable `recomputeAtBottom` callback so callers can feed it to the
 * bottom-settle hook.
 */
export function useVirtualizedBottomGeometry(
  hostRef: React.RefObject<HTMLDivElement | null>,
  itemCount: number,
  onAtBottomStateChange?: ((atBottom: boolean) => void) | null,
) {
  // Virtua fires onScroll only on offset changes. Re-read the settled DOM so
  // composer or spacer geometry changes cannot leave the affordance stale.
  const recomputeAtBottom = React.useCallback(() => {
    const scroller = hostRef.current?.firstElementChild;
    if (!(scroller instanceof HTMLDivElement)) return;
    const atBottom = isVirtualizedAtBottom(
      scroller.scrollHeight,
      scroller.clientHeight,
      scroller.scrollTop,
    );
    onAtBottomStateChange?.(atBottom);
  }, [hostRef, onAtBottomStateChange]);

  React.useEffect(() => {
    void itemCount;
    const scroller = hostRef.current?.firstElementChild;
    if (!(scroller instanceof HTMLDivElement)) return;
    if (typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(() => recomputeAtBottom());
    observer.observe(scroller);
    const spacer = scroller.querySelector<HTMLElement>("[data-bottom-spacer]");
    if (spacer) observer.observe(spacer);
    return () => observer.disconnect();
  }, [hostRef, itemCount, recomputeAtBottom]);

  return recomputeAtBottom;
}
