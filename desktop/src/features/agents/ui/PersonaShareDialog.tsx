import { ChevronRight, Download, X } from "lucide-react";

import type { AgentPersona } from "@/shared/api/types";
import {
  Dialog,
  DialogClose,
  DialogContent,
  DialogHeader,
  DialogTitle,
} from "@/shared/ui/dialog";

type PersonaShareDialogProps = {
  isPending: boolean;
  /**
   * Formerly drove memory-level encoding for hosted sharing. Kept on the prop
   * type so `AgentsView` continues to compile, but no longer consumed: native
   * media/blob upload was removed, so a snapshot can no longer be hosted and
   * shared as a link or sent as an attachment.
   */
  linkedAgentPubkey: string | null;
  onExport: () => void;
  onOpenChange: (open: boolean) => void;
  open: boolean;
  persona: AgentPersona;
};

type SnapshotShareDialogProps = {
  displayName: string;
  /**
   * Formerly produced snapshot bytes for upload-and-share. Kept (optional) so
   * `TeamShareDialog` continues to compile, but no longer consumed — see
   * `linkedAgentPubkey` above.
   */
  encodeSnapshot?: (
    memoryLevel: "none" | "core" | "everything",
  ) => Promise<{ fileBytes: number[]; fileName: string }>;
  /** See `encodeSnapshot` — retained for caller compatibility, no longer read. */
  hasMemoryOptions?: boolean;
  isPending: boolean;
  onExport: () => void;
  onOpenChange: (open: boolean) => void;
  /** See `encodeSnapshot` — retained for caller compatibility, no longer read. */
  onReset?: () => void;
  open: boolean;
  snapshotKind: "agent" | "team";
  testIdPrefix: string;
};

export function SnapshotShareDialog({
  displayName,
  isPending,
  onExport,
  onOpenChange,
  open,
  snapshotKind,
  testIdPrefix,
}: SnapshotShareDialogProps) {
  const itemLabel = snapshotKind === "team" ? "team" : "agent";

  function handleDialogOpenChange(nextOpen: boolean) {
    if (!nextOpen && isPending) return;
    onOpenChange(nextOpen);
  }

  return (
    <Dialog onOpenChange={handleDialogOpenChange} open={open}>
      <DialogContent
        aria-describedby={undefined}
        className="max-w-xl gap-3 bg-transparent p-0 shadow-none"
        data-testid={`${testIdPrefix}-dialog`}
        showCloseButton={false}
      >
        <div
          className="relative rounded-2xl bg-background p-6 pb-4 shadow-2xl"
          data-testid={`${testIdPrefix}-main-card`}
        >
          <DialogHeader className="space-y-0">
            <DialogTitle className="min-w-0 truncate pr-10">
              Share {displayName}
            </DialogTitle>
          </DialogHeader>
          <DialogClose
            className="absolute right-4 top-4 flex h-8 w-8 shrink-0 items-center justify-center rounded-md text-muted-foreground transition-colors duration-150 ease-out hover:bg-accent hover:text-accent-foreground focus:outline-hidden focus-visible:ring-1 focus-visible:ring-ring disabled:pointer-events-none disabled:opacity-100"
            disabled={isPending}
          >
            <X className="h-4 w-4" />
            <span className="sr-only">Close</span>
          </DialogClose>
        </div>
        <button
          className="relative flex min-h-14 w-full items-center gap-3 rounded-2xl bg-background px-5 py-4 text-left text-sm font-medium shadow-2xl outline-hidden transition-colors hover:bg-muted focus-visible:bg-muted disabled:cursor-default disabled:opacity-100"
          data-testid={`${testIdPrefix}-export`}
          disabled={isPending}
          onClick={onExport}
          type="button"
        >
          <Download className="h-4 w-4 shrink-0 text-muted-foreground" />
          <span className="min-w-0 flex-1">Export {itemLabel}</span>
          <ChevronRight className="h-4 w-4 shrink-0 text-muted-foreground" />
        </button>
      </DialogContent>
    </Dialog>
  );
}

export function PersonaShareDialog({
  isPending,
  onExport,
  onOpenChange,
  open,
  persona,
}: PersonaShareDialogProps) {
  return (
    <SnapshotShareDialog
      displayName={persona.displayName}
      isPending={isPending}
      onExport={onExport}
      onOpenChange={onOpenChange}
      open={open}
      snapshotKind="agent"
      testIdPrefix="persona-share"
    />
  );
}
