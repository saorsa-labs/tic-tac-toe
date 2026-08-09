import * as React from "react";
import { FileText } from "lucide-react";

import { useSmoothCorners } from "@/shared/ui/smoothCorners";

/** Human-readable byte size: "820 B", "12.4 KB", "3.1 MB". */
function formatFileSize(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes < 0) return "";
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let size = bytes / 1024;
  let i = 0;
  while (size >= 1024 && i < units.length - 1) {
    size /= 1024;
    i += 1;
  }
  return `${size < 10 ? size.toFixed(1) : Math.round(size)} ${units[i]}`;
}

/**
 * Display-only file card for a generic (non-image, non-video) attachment:
 * icon, filename, and size. Native download is not supported on the native
 * transport, so no download action is offered.
 */
export function FileCard({
  filename,
  size,
}: {
  filename: string;
  size?: number;
}) {
  const cardRef = React.useRef<HTMLDivElement | null>(null);
  const sizeLabel = size != null ? formatFileSize(size) : "";
  useSmoothCorners(cardRef);

  return (
    <div
      ref={cardRef}
      data-testid="file-card"
      className="my-1 inline-flex max-w-sm items-center gap-3 rounded-2xl border border-border/70 bg-muted/40 px-3 py-2 text-left no-underline"
      style={{ borderRadius: "1rem" }}
    >
      <span className="flex h-9 w-9 shrink-0 items-center justify-center rounded-lg bg-background text-muted-foreground">
        <FileText className="h-4 w-4" />
      </span>
      <span className="min-w-0 flex-1">
        <span className="block truncate text-sm font-medium text-foreground">
          {filename}
        </span>
        {sizeLabel ? (
          <span className="block text-xs text-muted-foreground">
            {sizeLabel}
          </span>
        ) : null}
      </span>
    </div>
  );
}
