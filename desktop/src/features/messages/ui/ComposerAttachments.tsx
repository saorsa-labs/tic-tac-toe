import * as React from "react";
import { AnimatePresence, LayoutGroup, motion } from "motion/react";
import * as DialogPrimitive from "@radix-ui/react-dialog";
import { Bot, FileText, HatGlasses, Play, Users, X } from "lucide-react";

import type { BlobDescriptor } from "@/shared/api/tauri";
import type { ImetaMedia } from "@/features/messages/lib/imetaMediaMarkdown";

import { shortHash } from "@/features/messages/lib/usePendingAttachments";
import { cn } from "@/shared/lib/cn";
import {
  Attachment,
  AttachmentAction,
  AttachmentActions,
  AttachmentContent,
  AttachmentDescription,
  AttachmentMedia,
  AttachmentTitle,
} from "@/shared/ui/attachment";
import { MODAL_BACKDROP_BLUR_CLASS } from "@/shared/ui/modalBackdrop";
import { Toggle } from "@/shared/ui/toggle";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/shared/ui/tooltip";

type ComposerAttachmentsProps = {
  attachments: ImetaMedia[];
  onRemove: (url: string) => void;
  onToggleSpoiler?: (url: string) => void;
  spoileredUrls?: ReadonlySet<string>;
};

function formatAttachmentSize(size: number): string {
  if (size < 1024) return `${size} B`;
  if (size < 1024 * 1024) return `${(size / 1024).toFixed(1)} KB`;
  return `${(size / (1024 * 1024)).toFixed(1)} MB`;
}

type SnapshotKind = "agent" | "team";

function getSnapshotKind(attachment: ImetaMedia): SnapshotKind | null {
  const filename = attachment.filename?.toLowerCase();
  if (attachment.sha256.length !== 64) return null;
  if (filename?.endsWith(".agent.png") || filename?.endsWith(".agent.json")) {
    return "agent";
  }
  if (filename?.endsWith(".team.png") || filename?.endsWith(".team.json")) {
    return "team";
  }
  return null;
}

function ComposerSnapshotCard({
  attachment,
  onRemove,
  snapshotKind,
}: {
  attachment: ImetaMedia;
  onRemove: (url: string) => void;
  snapshotKind: SnapshotKind;
}) {
  const [thumbError, setThumbError] = React.useState(false);
  const isAgentPng =
    snapshotKind === "agent" &&
    attachment.filename?.toLowerCase().endsWith(".agent.png");
  const showThumb = isAgentPng && !thumbError;
  const SnapshotIcon = snapshotKind === "team" ? Users : Bot;
  const fallbackLabel = snapshotKind === "team" ? "Team" : "Agent";
  const displayName =
    attachment.displayLabel?.trim() ||
    attachment.filename?.replace(/\.(?:agent|team)\.(?:png|json)$/i, "") ||
    fallbackLabel;

  return (
    <motion.div
      animate={{ opacity: 1, scale: 1 }}
      className="min-w-0 max-w-full"
      exit={{ opacity: 0, scale: 0.8 }}
      initial={false}
      layout
      transition={{ type: "spring", stiffness: 500, damping: 30 }}
    >
      <Attachment
        className="w-fit max-w-full shadow-none"
        data-testid={`composer-${snapshotKind}-snapshot-card`}
        size="sm"
      >
        <AttachmentMedia
          className={
            showThumb
              ? "relative h-9 w-9"
              : "bg-primary/10 text-primary ring-1 ring-primary/20 dark:bg-primary/15"
          }
          variant={showThumb ? "image" : "icon"}
        >
          {showThumb ? (
            <>
              <img
                alt=""
                aria-hidden="true"
                className="pointer-events-none absolute inset-0 h-full w-full scale-150 object-cover"
                src={attachment.url}
              />
              <img
                alt=""
                className="relative h-full w-full object-cover"
                src={attachment.url}
                onError={() => setThumbError(true)}
              />
            </>
          ) : (
            <SnapshotIcon />
          )}
        </AttachmentMedia>
        <AttachmentContent>
          <AttachmentTitle className="overflow-visible whitespace-normal text-clip">
            {displayName}
          </AttachmentTitle>
          <AttachmentDescription className="text-secondary-foreground/75">
            {formatAttachmentSize(attachment.size)}
          </AttachmentDescription>
        </AttachmentContent>
        <AttachmentActions className="ml-4">
          <AttachmentAction
            aria-label={`Remove ${displayName}`}
            className="border-0 bg-transparent text-muted-foreground/70 shadow-none hover:text-foreground hover:shadow-none focus-visible:bg-muted focus-visible:ring-0"
            data-testid={`composer-${snapshotKind}-snapshot-remove`}
            onClick={() => onRemove(attachment.url)}
            title="Remove"
            type="button"
          >
            <X />
          </AttachmentAction>
        </AttachmentActions>
      </Attachment>
    </motion.div>
  );
}

const LIGHTBOX_BUTTON_CLASS =
  "rounded-full bg-black/50 p-2 text-white/80 transition-colors hover:bg-black/70 hover:text-white focus:outline-hidden focus:ring-2 focus:ring-white/30";

const COMPOSER_MEDIA_HEIGHT_PX = 55;
const COMPOSER_MEDIA_WIDTH_PX = 55;

function composerMediaStyle(): React.CSSProperties {
  return {
    height: COMPOSER_MEDIA_HEIGHT_PX,
    width: COMPOSER_MEDIA_WIDTH_PX,
  };
}

type MediaAttachmentItemProps = {
  attachment: BlobDescriptor;
  isSpoilered: boolean;
  onRemove: (url: string) => void;
  onToggleSpoiler?: (url: string) => void;
};

/**
 * Displays archived image/video attachments in a read-only lightbox.
 *
 * Forwards its ref to the root motion.div — required by the parent
 * `AnimatePresence mode="popLayout"`, which measures exiting children.
 */
const MediaAttachmentItem = React.forwardRef<
  HTMLDivElement,
  MediaAttachmentItemProps
>(function MediaAttachmentItem(
  { attachment, isSpoilered, onRemove, onToggleSpoiler },
  ref,
) {
  const [open, setOpen] = React.useState(false);

  const handleOpenChange = React.useCallback((nextOpen: boolean) => {
    setOpen(nextOpen);
  }, []);
  const handleEscapeKeyDown = React.useCallback(() => {
    setOpen(false);
  }, []);

  const hash = shortHash(attachment.sha256);
  const isVideo = attachment.type.startsWith("video/");
  const thumbUrl = attachment.thumb ? attachment.thumb : attachment.url;
  const videoPosterUrl = attachment.image
    ? attachment.image
    : attachment.thumb
      ? attachment.thumb
      : undefined;

  return (
    <motion.div
      ref={ref}
      layout
      initial={false}
      animate={{ opacity: 1, scale: 1 }}
      exit={{ opacity: 0, scale: 0.8 }}
      transition={{ type: "spring", stiffness: 500, damping: 30 }}
      className="group relative"
    >
      <div
        className="relative h-[55px] max-w-[55px]"
        style={composerMediaStyle()}
      >
        <DialogPrimitive.Root open={open} onOpenChange={handleOpenChange}>
          <DialogPrimitive.Trigger asChild>
            <div className="h-full w-full cursor-pointer overflow-hidden rounded-2xl border border-border/70">
              {isVideo ? (
                <div className="relative flex h-full w-full items-center justify-center bg-muted text-white">
                  {videoPosterUrl ? (
                    <img
                      src={videoPosterUrl}
                      alt={`Video attachment ${hash}`}
                      className="h-full w-full object-cover"
                    />
                  ) : (
                    <div className="h-full w-full bg-muted/80" />
                  )}
                  <div className="absolute inset-0 bg-black/15" />
                  <div className="absolute flex h-5 w-5 items-center justify-center rounded-full bg-black/55 backdrop-blur-sm">
                    <Play className="h-4 w-4 fill-white text-white" />
                  </div>
                </div>
              ) : (
                <img
                  src={thumbUrl}
                  alt={`Attachment ${hash}`}
                  className="h-full w-full object-cover"
                />
              )}
              {isSpoilered ? (
                <div
                  className="pointer-events-none absolute inset-0 flex items-center justify-center rounded-2xl bg-background/55 text-foreground/70 backdrop-blur-[1px]"
                  data-composer-media-spoiler=""
                >
                  <HatGlasses className="h-4 w-4" />
                </div>
              ) : null}
            </div>
          </DialogPrimitive.Trigger>
          <DialogPrimitive.Portal>
            <DialogPrimitive.Overlay
              className={cn(
                "fixed inset-0 z-50 bg-black/80 data-[state=open]:animate-in data-[state=closed]:animate-out data-[state=closed]:fade-out-0 data-[state=open]:fade-in-0",
                MODAL_BACKDROP_BLUR_CLASS,
              )}
            />
            <DialogPrimitive.Content
              className="fixed inset-0 z-50 flex items-center justify-center p-8"
              onPointerDownOutside={(e) => e.preventDefault()}
              onInteractOutside={(e) => e.preventDefault()}
              onEscapeKeyDown={handleEscapeKeyDown}
            >
              <DialogPrimitive.Title className="sr-only">
                Attachment {hash} preview
              </DialogPrimitive.Title>
              <DialogPrimitive.Description className="sr-only">
                Full-size attachment preview. Press Escape or click outside to
                close.
              </DialogPrimitive.Description>
              {isVideo ? (
                // biome-ignore lint/a11y/useMediaCaption: archived user media may not include captions
                <video
                  src={attachment.url}
                  controls
                  className={cn(
                    "relative max-h-[90vh] max-w-[90vw] rounded-lg",
                    isSpoilered && "blur-2xl brightness-75",
                  )}
                />
              ) : (
                <img
                  alt={`Attachment ${hash}`}
                  className={cn(
                    "relative max-h-[90vh] max-w-[90vw] rounded-lg object-contain",
                    isSpoilered && "blur-2xl brightness-75",
                  )}
                  src={attachment.url}
                />
              )}
              {isSpoilered ? (
                /*
                 * Expanded-media counterpart of the thumbnail spoiler treatment:
                 * the media itself is blurred above, and this layer centers the
                 * spoiler glyph. pointer-events-none keeps controls and
                 * backdrop-close clickable.
                 */
                <div
                  className="pointer-events-none absolute inset-0 flex items-center justify-center text-foreground/70"
                  data-lightbox-media-spoiler=""
                >
                  <HatGlasses className="h-10 w-10" />
                </div>
              ) : null}
              <div className="absolute right-4 top-4 flex items-center gap-2">
                {onToggleSpoiler ? (
                  <Tooltip disableHoverableContent>
                    <TooltipTrigger asChild>
                      <Toggle
                        aria-label={
                          isSpoilered ? "Remove spoiler" : "Mark as spoiler"
                        }
                        className={cn(
                          LIGHTBOX_BUTTON_CLASS,
                          "h-auto min-w-0",
                          // Active state driven by component state, not
                          // Radix's data-state: the TooltipTrigger clobbers
                          // the Toggle's data-state attribute. Swap the
                          // circular pill for the shared button radius with
                          // a visible ring so a spoilered attachment reads
                          // as "selected" on the dark lightbox backdrop.
                          isSpoilered &&
                            "rounded-lg bg-white/25 text-white ring-2 ring-white",
                        )}
                        data-testid="composer-attachment-spoiler"
                        onPressedChange={() => onToggleSpoiler(attachment.url)}
                        pressed={isSpoilered}
                      >
                        <HatGlasses className="h-4 w-4" />
                      </Toggle>
                    </TooltipTrigger>
                    <TooltipContent>
                      {isSpoilered ? "Remove spoiler" : "Mark as spoiler"}
                    </TooltipContent>
                  </Tooltip>
                ) : null}
                <DialogPrimitive.Close className={LIGHTBOX_BUTTON_CLASS}>
                  <X className="h-4 w-4" />
                  <span className="sr-only">Close</span>
                </DialogPrimitive.Close>
              </div>
            </DialogPrimitive.Content>
          </DialogPrimitive.Portal>
        </DialogPrimitive.Root>
        <Tooltip disableHoverableContent>
          <TooltipTrigger asChild>
            <button
              type="button"
              onClick={() => onRemove(attachment.url)}
              className="absolute -right-1 -top-1 hidden h-4 w-4 items-center justify-center rounded-full bg-foreground text-background group-hover:flex"
            >
              <X className="h-2.5 w-2.5" />
            </button>
          </TooltipTrigger>
          <TooltipContent>Remove attachment</TooltipContent>
        </Tooltip>
      </div>
    </motion.div>
  );
});

/** Read-only previews for attachments already present in a restored draft. */
export const ComposerAttachments = React.memo(function ComposerAttachments({
  attachments,
  onRemove,
  onToggleSpoiler,
  spoileredUrls,
}: ComposerAttachmentsProps) {
  if (attachments.length === 0) return null;

  return (
    <LayoutGroup>
      <motion.div
        layout
        className="flex items-center gap-2"
        transition={{ type: "spring", stiffness: 500, damping: 30 }}
      >
        <AnimatePresence mode="popLayout">
          {attachments.map((attachment) => {
            const hash = shortHash(attachment.sha256);
            const isVideo = attachment.type.startsWith("video/");
            const isImage = attachment.type.startsWith("image/");
            const isFile = !isVideo && !isImage;

            const snapshotKind = getSnapshotKind(attachment);
            if (snapshotKind) {
              return (
                <ComposerSnapshotCard
                  attachment={attachment}
                  key={attachment.url}
                  onRemove={onRemove}
                  snapshotKind={snapshotKind}
                />
              );
            }

            // Generic file: compact chip with a file icon + filename, plus the
            // same remove button. No lightbox (nothing to preview).
            if (isFile) {
              const label =
                attachment.filename ||
                attachment.url.split("/").pop() ||
                `file ${hash}`;
              return (
                <motion.div
                  key={attachment.url}
                  layout
                  initial={false}
                  animate={{ opacity: 1, scale: 1 }}
                  exit={{ opacity: 0, scale: 0.8 }}
                  transition={{ type: "spring", stiffness: 500, damping: 30 }}
                  className="group relative"
                >
                  <div className="flex h-5 max-w-40 items-center gap-1 rounded border border-border/70 bg-muted px-1.5">
                    <FileText className="h-4 w-4 shrink-0 text-muted-foreground" />
                    <span className="truncate text-2xs text-muted-foreground">
                      {label}
                    </span>
                  </div>
                  <Tooltip disableHoverableContent>
                    <TooltipTrigger asChild>
                      <button
                        type="button"
                        onClick={() => onRemove(attachment.url)}
                        className="absolute -right-1 -top-1 hidden h-4 w-4 items-center justify-center rounded-full bg-foreground text-background group-hover:flex"
                      >
                        <X className="h-2.5 w-2.5" />
                      </button>
                    </TooltipTrigger>
                    <TooltipContent>Remove attachment</TooltipContent>
                  </Tooltip>
                </motion.div>
              );
            }

            return (
              <MediaAttachmentItem
                attachment={attachment}
                isSpoilered={spoileredUrls?.has(attachment.url) ?? false}
                key={attachment.url}
                onRemove={onRemove}
                onToggleSpoiler={onToggleSpoiler}
              />
            );
          })}
        </AnimatePresence>
      </motion.div>
    </LayoutGroup>
  );
});
