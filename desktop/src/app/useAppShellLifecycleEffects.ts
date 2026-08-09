import * as React from "react";

import { setDesktopAppBadge } from "@/features/notifications/lib/desktop";

type AppShellLifecycleEffectsOptions = {
  homeBadgeCountExcludingHighPriority: number;
  unreadChannelIds: ReadonlySet<string>;
  unreadChannelNotificationCount: number;
};

export function useAppShellLifecycleEffects({
  homeBadgeCountExcludingHighPriority,
  unreadChannelIds,
  unreadChannelNotificationCount,
}: AppShellLifecycleEffectsOptions) {
  // Prevent webview file:/// navigation on file drop outside the composer.
  // Scoped to file drags only (text drag-and-drop into inputs still works).
  // Composer's onDrop fires first (React synthetic before window bubble).
  React.useEffect(() => {
    function preventNavigation(e: DragEvent) {
      if (e.dataTransfer?.types.includes("Files")) {
        e.preventDefault();
      }
    }
    window.addEventListener("dragover", preventNavigation);
    window.addEventListener("drop", preventNavigation);
    return () => {
      window.removeEventListener("dragover", preventNavigation);
      window.removeEventListener("drop", preventNavigation);
    };
  }, []);

  React.useEffect(() => {
    const count =
      unreadChannelNotificationCount + homeBadgeCountExcludingHighPriority;
    void setDesktopAppBadge(
      count
        ? { kind: "count", count }
        : { kind: unreadChannelIds.size ? "dot" : "none" },
    );
  }, [
    homeBadgeCountExcludingHighPriority,
    unreadChannelIds,
    unreadChannelNotificationCount,
  ]);
}
