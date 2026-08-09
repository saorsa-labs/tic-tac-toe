import type { AgentChannelAttachmentFailure } from "@/features/agents/channelAttachmentFailure";
import type { CreateManagedAgentResponse } from "@/shared/api/types";
import { Button } from "@/shared/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/shared/ui/dialog";

export function SecretRevealDialog({
  attachmentFailure,
  created,
  isRetryingAttachment = false,
  onOpenChange,
  onRetryAttachment,
}: {
  attachmentFailure?: AgentChannelAttachmentFailure | null;
  created: CreateManagedAgentResponse | null;
  isRetryingAttachment?: boolean;
  onOpenChange: (open: boolean) => void;
  onRetryAttachment?: () => void;
}) {
  return (
    <Dialog onOpenChange={onOpenChange} open={created !== null}>
      <DialogContent className="max-w-2xl overflow-hidden p-0">
        <div className="flex max-h-[85vh] flex-col">
          <DialogHeader className="border-b border-border/60 px-6 py-5 pr-14">
            <DialogTitle>Agent created</DialogTitle>
            <DialogDescription>
              Review the agent status below.
            </DialogDescription>
          </DialogHeader>

          <div className="flex-1 space-y-4 overflow-y-auto px-6 py-5">
            {created ? (
              <>
                {created.profileSyncError ? (
                  <p className="rounded-2xl border border-amber-500/30 bg-amber-500/10 px-4 py-3 text-sm text-warning">
                    {created.profileSyncError}
                  </p>
                ) : null}

                {created.spawnError ? (
                  <p className="rounded-2xl border border-destructive/30 bg-destructive/10 px-4 py-3 text-sm text-destructive">
                    {created.spawnError}
                  </p>
                ) : attachmentFailure ? (
                  <div
                    className="space-y-1 rounded-2xl border border-destructive/30 bg-destructive/10 px-4 py-3 text-sm text-destructive"
                    role="alert"
                  >
                    <p>
                      {created.agent.name} was created, but couldn’t be added to
                      #{attachmentFailure.channelName}.
                    </p>
                    <p>{attachmentFailure.error}</p>
                  </div>
                ) : (
                  <p className="rounded-2xl border border-primary/20 bg-primary/10 px-4 py-3 text-sm text-primary">
                    {created.agent.name} is ready
                    {created.agent.status === "running"
                      ? " and running."
                      : created.agent.status === "deployed"
                        ? " and deployed."
                        : "."}
                  </p>
                )}
              </>
            ) : null}
          </div>

          <div className="flex justify-end gap-2 border-t border-border/60 px-6 py-4">
            {attachmentFailure && onRetryAttachment ? (
              <Button
                disabled={isRetryingAttachment}
                onClick={onRetryAttachment}
                size="sm"
                type="button"
              >
                {isRetryingAttachment ? "Trying again…" : "Try again"}
              </Button>
            ) : null}
            <Button
              disabled={isRetryingAttachment}
              onClick={() => onOpenChange(false)}
              size="sm"
              type="button"
              variant="outline"
            >
              Done
            </Button>
          </div>
        </div>
      </DialogContent>
    </Dialog>
  );
}
