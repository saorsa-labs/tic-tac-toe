import * as React from "react";

import type { AddCommunityPrefillRequest } from "@/features/communities/addCommunityPrefill";
import { expandTilde } from "@/features/communities/communityStorage";
import { createNativeCommunity } from "@/features/communities/nativeCommunityApi";
import { useCommunityOnboarding } from "@/features/onboarding/communityOnboarding";
import { inviteErrorMessage } from "@/shared/api/inviteHelpers";
import { validateReposDir } from "@/shared/api/tauri";
import { Button } from "@/shared/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/shared/ui/dialog";
import { Input } from "@/shared/ui/input";

type AddCommunityDialogProps = {
  prefill?: AddCommunityPrefillRequest | null;
  open: boolean;
  onOpenChange: (open: boolean) => void;
};

/**
 * Native-only Add Community surface: create a new x0xd named group. The dialog
 * performs NO relay policy discovery/acceptance and persists no relay URL/token
 * as credentials — create binds the active native group directly, then hands
 * off to the existing onboarding transaction at `connecting` so App.tsx
 * registers and transitions to the new community.
 *
 * Joining a group via a one-time `x0x://invite/...` link is intentionally not
 * offered: the opaque invite contract cannot authenticate, version, or
 * canonically bind the secure-group bootstrap, so invite mint/accept is gated
 * pending x0x frontier review.
 */
export function AddCommunityDialog({
  prefill,
  open,
  onOpenChange,
}: AddCommunityDialogProps) {
  const [name, setName] = React.useState("");
  const [reposDir, setReposDir] = React.useState("");
  const [error, setError] = React.useState<string | null>(null);
  const [reposDirError, setReposDirError] = React.useState<string | null>(null);
  const [isPending, setIsPending] = React.useState(false);
  const communityOnboarding = useCommunityOnboarding();
  const appliedPrefillId = React.useRef<string | null>(null);

  React.useEffect(() => {
    if (!prefill || appliedPrefillId.current === prefill.requestId) return;
    appliedPrefillId.current = prefill.requestId;
    setName(prefill.name ?? "");
    setReposDir("");
    setReposDirError(null);
    setError(null);
  }, [prefill]);

  const handleClose = React.useCallback(() => {
    onOpenChange(false);
    setName("");
    setReposDir("");
    setError(null);
    setReposDirError(null);
  }, [onOpenChange]);

  const handleSubmit = React.useCallback(
    async (e: React.FormEvent) => {
      e.preventDefault();
      if (isPending) return;
      setError(null);

      const trimmedName = name.trim();
      if (!trimmedName) {
        setError("Enter a community name.");
        return;
      }

      // Expand `~` before save — the backend rejects tilde paths. Empty input
      // resolves to `undefined` so REPOS keeps its default location. Validate
      // the expanded value (the bytes the backend canonicalizes) before save
      // so a bad path is caught here instead of bricking a later boot.
      const expandedReposDir = await expandTilde(reposDir);
      try {
        await validateReposDir(expandedReposDir ?? "");
      } catch (validationError) {
        setReposDirError(String(validationError));
        return;
      }

      setIsPending(true);
      try {
        const group = await createNativeCommunity({
          name: trimmedName,
        });

        communityOnboarding.start({
          source: "add-community",
          communityName: group.name,
          groupId: group.groupId,
          reposDir: expandedReposDir,
        });
        handleClose();
      } catch (submissionError) {
        setError(inviteErrorMessage(submissionError));
      } finally {
        setIsPending(false);
      }
    },
    [communityOnboarding, handleClose, isPending, name, reposDir],
  );

  const canSubmit = !isPending && name.trim().length > 0;

  return (
    <Dialog
      onOpenChange={(nextOpen) => {
        if (!nextOpen) handleClose();
        else onOpenChange(true);
      }}
      open={open}
    >
      <DialogContent className="max-w-md">
        <DialogHeader>
          <DialogTitle>Add Community</DialogTitle>
          <DialogDescription>Create a new x0x community.</DialogDescription>
        </DialogHeader>
        <form
          className="flex flex-col gap-4"
          onSubmit={(e) => void handleSubmit(e)}
        >
          <div className="flex flex-col gap-1.5">
            <label
              className="text-sm font-medium text-foreground"
              htmlFor="ws-name"
            >
              Community name
            </label>
            <Input
              autoFocus
              id="ws-name"
              onChange={(e) => setName(e.target.value)}
              placeholder="My Community"
              type="text"
              value={name}
            />
          </div>
          <div className="flex flex-col gap-1.5">
            <label
              className="text-sm font-medium text-foreground"
              htmlFor="ws-repos-dir"
            >
              Repos Directory
              <span className="ml-1 text-xs font-normal text-muted-foreground">
                (optional)
              </span>
            </label>
            <Input
              id="ws-repos-dir"
              onChange={(e) => {
                setReposDir(e.target.value);
                setReposDirError(null);
              }}
              placeholder="~/Development"
              type="text"
              value={reposDir}
            />
            {reposDirError ? (
              <p className="text-xs text-destructive">{reposDirError}</p>
            ) : null}
            <p className="text-xs text-muted-foreground">
              Point the agent's <code>REPOS</code> directory at an existing
              folder so agents work in your local checkouts. Leave blank to use
              the default location.
            </p>
          </div>
          <p className="text-xs text-muted-foreground">
            Communities share your active identity. To use a different key,
            import it on the profile step (or in settings).
          </p>
          {error ? <p className="text-xs text-destructive">{error}</p> : null}
          <div className="flex justify-end gap-2 pt-2">
            <Button onClick={handleClose} type="button" variant="outline">
              Cancel
            </Button>
            <Button disabled={!canSubmit} type="submit">
              {isPending ? "Adding..." : "Create Community"}
            </Button>
          </div>
        </form>
      </DialogContent>
    </Dialog>
  );
}
