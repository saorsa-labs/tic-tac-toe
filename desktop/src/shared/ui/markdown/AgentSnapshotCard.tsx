import * as React from "react";
import { Bot, Users } from "lucide-react";

import { cn } from "@/shared/lib/cn";
import {
  Attachment,
  AttachmentContent,
  AttachmentDescription,
  AttachmentMedia,
  AttachmentTitle,
} from "@/shared/ui/attachment";

export type AgentSnapshotCardProps = {
  displayName: string;
  sharedBy?: string;
  size?: number;
  /** Discriminant used to label the card and pick its icon. */
  snapshotKind: "agent" | "team";
  /**
   * Optional thumbnail URL for the card icon — the agent's avatar image.
   * When present, renders in place of the generic Bot icon. Falls back to
   * the Bot icon when absent, when the URL is a non-image MIME, or when
   * the image fails to load.
   */
  thumb?: string;
};

/**
 * Display-only snapshot attachment card rendered in a message timeline when
 * an agent or team snapshot attachment is classified as a snapshot candidate.
 *
 * Renders the snapshot's avatar/icon, display name, and shared-by metadata.
 * Native import/download of snapshot bytes is not supported on the native
 * transport, so no actions are offered — the card is presentation-only.
 */
export function AgentSnapshotCard({
  displayName,
  sharedBy,
  size,
  snapshotKind,
  thumb,
}: AgentSnapshotCardProps) {
  const [thumbError, setThumbError] = React.useState(false);

  const SnapshotIcon = snapshotKind === "team" ? Users : Bot;
  const showThumb = !!thumb && !thumbError;
  const formattedSize =
    size == null
      ? null
      : size < 1024
        ? `${size} B`
        : size < 1024 * 1024
          ? `${(size / 1024).toFixed(1)} KB`
          : `${(size / (1024 * 1024)).toFixed(1)} MB`;
  const metadata = [sharedBy ? `Shared by ${sharedBy}` : null, formattedSize]
    .filter(Boolean)
    .join(" · ");

  return (
    <Attachment
      className="my-1 inline-flex w-fit max-w-full shadow-none"
      data-testid="agent-snapshot-card"
      state="done"
    >
      <AttachmentMedia
        className={cn(
          showThumb
            ? "relative h-9 w-9"
            : "bg-primary/10 text-primary ring-1 ring-primary/20 dark:bg-primary/15",
        )}
        variant={showThumb ? "image" : "icon"}
      >
        {showThumb ? (
          <>
            <img
              alt=""
              aria-hidden="true"
              className="pointer-events-none absolute inset-0 h-full w-full scale-150 object-cover"
              src={thumb}
              referrerPolicy="no-referrer"
            />
            <img
              alt=""
              className="relative h-full w-full object-cover"
              data-testid="agent-snapshot-card-thumb"
              src={thumb}
              referrerPolicy="no-referrer"
              onError={() => setThumbError(true)}
            />
          </>
        ) : (
          <SnapshotIcon />
        )}
      </AttachmentMedia>
      <AttachmentContent>
        <AttachmentTitle
          className="overflow-visible whitespace-normal text-clip"
          title={displayName}
        >
          {displayName}
        </AttachmentTitle>
        {metadata ? (
          <AttachmentDescription className="overflow-visible whitespace-normal text-clip text-secondary-foreground/75">
            {metadata}
          </AttachmentDescription>
        ) : null}
      </AttachmentContent>
    </Attachment>
  );
}
