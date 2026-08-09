import * as React from "react";

import { copyTextToClipboard } from "@/shared/lib/clipboard";
import {
  MediaContextMenu,
  type MediaContextMenuItem,
  type MediaContextMenuPosition,
  useDismissMediaContextMenu,
} from "@/shared/ui/markdown/MediaContextMenu";

type UseVideoContextMenu = {
  /** `onContextMenuCapture` handler for the inline video surface. */
  onContextMenu: (event: React.MouseEvent) => void;
  /** The positioned menu element while open, or `null`. */
  menu: React.ReactNode;
};

/**
 * Owns the inline video right-click menu: open/close state, the pointer-anchor
 * handler, and the Copy-link action. Kept out of `VideoPlayer` so that large
 * component stays focused on playback.
 *
 * `downloadUrl` is the original relay `/media/` URL (distinct from a rewritten
 * proxy `src`); the menu offers Copy link for it, falling back to `src`.
 */
export function useVideoContextMenu(
  src: string,
  downloadUrl?: string,
  // Kept for the VideoPlayer call site's positional signature; the native
  // transport has no download command, so the filename is no longer used.
  _filename?: string,
): UseVideoContextMenu {
  const [position, setPosition] =
    React.useState<MediaContextMenuPosition | null>(null);
  const close = React.useCallback(() => setPosition(null), []);
  useDismissMediaContextMenu(Boolean(position), close);

  const onContextMenu = React.useCallback((event: React.MouseEvent) => {
    event.preventDefault();
    setPosition({ x: event.clientX, y: event.clientY });
  }, []);

  const items = React.useMemo<MediaContextMenuItem[]>(
    () => [
      {
        label: "Copy link",
        onSelect: () => {
          close();
          copyTextToClipboard(downloadUrl ?? src, "Link copied to clipboard");
        },
      },
    ],
    [close, downloadUrl, src],
  );

  return {
    onContextMenu,
    menu: position ? (
      <MediaContextMenu
        dataAttributes={["data-video-context-menu"]}
        items={items}
        position={position}
      />
    ) : null,
  };
}
