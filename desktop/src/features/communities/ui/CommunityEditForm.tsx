import * as React from "react";

import { Button } from "@/shared/ui/button";
import { Input } from "@/shared/ui/input";
import { Spinner } from "@/shared/ui/spinner";

export type CommunityEditFormProps = {
  cancelLabel?: string;
  initialName: string;
  isSubmitting?: boolean;
  onCancel: () => void;
  onSubmit: (name: string) => void;
  submitLabel: string;
};

export function CommunityEditForm({
  cancelLabel = "Cancel",
  initialName,
  isSubmitting = false,
  onCancel,
  onSubmit,
  submitLabel,
}: CommunityEditFormProps) {
  const [name, setName] = React.useState(initialName);
  const [error, setError] = React.useState<string | null>(null);

  const handleSubmit = React.useCallback(
    (event: React.FormEvent<HTMLFormElement>) => {
      event.preventDefault();
      const trimmedName = name.trim();
      if (!trimmedName) {
        setError("Please enter a community name.");
        return;
      }
      onSubmit(trimmedName);
    },
    [name, onSubmit],
  );

  return (
    <form className="flex w-full flex-col gap-4" onSubmit={handleSubmit}>
      <div className="space-y-1.5 text-left">
        <label
          className="text-sm font-medium text-foreground"
          htmlFor="community-edit-name"
        >
          Community name
        </label>
        <Input
          autoFocus
          className="h-10 bg-background"
          disabled={isSubmitting}
          id="community-edit-name"
          onChange={(event) => {
            setName(event.target.value);
            setError(null);
          }}
          placeholder="Design team"
          type="text"
          value={name}
        />
      </div>

      {error ? (
        <p className="text-sm text-destructive" role="alert">
          {error}
        </p>
      ) : null}

      <div className="flex gap-2">
        <Button className="h-10 flex-1" disabled={isSubmitting} type="submit">
          {isSubmitting ? (
            <Spinner
              aria-label="Saving community"
              className="h-4 w-4 border-2"
            />
          ) : (
            submitLabel
          )}
        </Button>
        <Button
          className="h-10 flex-1 text-muted-foreground hover:text-accent-foreground"
          disabled={isSubmitting}
          onClick={onCancel}
          type="button"
          variant="ghost"
        >
          {cancelLabel}
        </Button>
      </div>
    </form>
  );
}
