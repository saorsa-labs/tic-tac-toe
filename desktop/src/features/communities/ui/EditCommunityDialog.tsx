import * as React from "react";

import type { Community } from "@/features/communities/types";
import { expandTilde } from "@/features/communities/communityStorage";
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

type EditCommunityDialogProps = {
  community: Community | null;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onSave: (
    id: string,
    updates: Partial<Pick<Community, "name" | "reposDir">>,
  ) => void;
  onRemove?: (id: string) => void;
  canRemove?: boolean;
};

export function EditCommunityDialog({
  community,
  open,
  onOpenChange,
  onSave,
  onRemove,
  canRemove,
}: EditCommunityDialogProps) {
  const [name, setName] = React.useState("");
  const [reposDir, setReposDir] = React.useState("");
  const [reposDirError, setReposDirError] = React.useState<string | null>(null);

  React.useEffect(() => {
    if (community && open) {
      setName(community.name);
      setReposDir(community.reposDir ?? "");
      setReposDirError(null);
    }
  }, [community, open]);

  const handleClose = React.useCallback(() => {
    onOpenChange(false);
  }, [onOpenChange]);

  const handleSubmit = React.useCallback(
    async (event: React.FormEvent) => {
      event.preventDefault();
      if (!community) return;

      const updates: Partial<Pick<Community, "name" | "reposDir">> = {};
      const trimmedName = name.trim();
      if (trimmedName && trimmedName !== community.name) {
        updates.name = trimmedName;
      }

      const expandedReposDir = await expandTilde(reposDir);
      if (expandedReposDir !== community.reposDir) {
        try {
          await validateReposDir(expandedReposDir ?? "");
        } catch (error) {
          setReposDirError(String(error));
          return;
        }
        updates.reposDir = expandedReposDir;
      }

      if (Object.keys(updates).length > 0) {
        onSave(community.id, updates);
      }
      handleClose();
    },
    [community, handleClose, name, onSave, reposDir],
  );

  const handleRemove = React.useCallback(() => {
    if (community && onRemove) {
      onRemove(community.id);
      handleClose();
    }
  }, [community, handleClose, onRemove]);

  if (!community) return null;

  return (
    <Dialog onOpenChange={onOpenChange} open={open}>
      <DialogContent className="max-w-md">
        <DialogHeader>
          <DialogTitle>Edit Community</DialogTitle>
          <DialogDescription>
            Update this device&apos;s community label and agent checkout
            directory.
          </DialogDescription>
        </DialogHeader>
        <form
          className="flex flex-col gap-4"
          onSubmit={(event) => void handleSubmit(event)}
        >
          <div className="flex flex-col gap-1.5">
            <label
              className="text-sm font-medium text-foreground"
              htmlFor="edit-ws-name"
            >
              Name
            </label>
            <Input
              autoFocus
              id="edit-ws-name"
              onChange={(event) => setName(event.target.value)}
              placeholder="My Community"
              type="text"
              value={name}
            />
          </div>
          <div className="flex flex-col gap-1.5">
            <label
              className="text-sm font-medium text-foreground"
              htmlFor="edit-ws-repos-dir"
            >
              Repos Directory
              <span className="ml-1 text-xs font-normal text-muted-foreground">
                (optional)
              </span>
            </label>
            <Input
              id="edit-ws-repos-dir"
              onChange={(event) => {
                setReposDir(event.target.value);
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
              Point the agent&apos;s <code>REPOS</code> directory at an existing
              folder so agents work in your local checkouts. Leave blank to use
              the default location.
            </p>
          </div>
          <div className="flex items-center justify-between pt-2">
            <div>
              {canRemove && onRemove ? (
                <Button
                  className="text-destructive hover:text-destructive"
                  onClick={handleRemove}
                  size="sm"
                  type="button"
                  variant="ghost"
                >
                  Remove Community
                </Button>
              ) : null}
            </div>
            <div className="flex gap-2">
              <Button onClick={handleClose} type="button" variant="outline">
                Cancel
              </Button>
              <Button disabled={!name.trim()} type="submit">
                Save Changes
              </Button>
            </div>
          </div>
        </form>
      </DialogContent>
    </Dialog>
  );
}
