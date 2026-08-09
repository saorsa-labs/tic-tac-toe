import * as React from "react";

import { useCommunities } from "../useCommunities";
import { CommunityEditForm } from "./CommunityEditForm";

type CommunityChangeOverlayProps = {
  onClose: () => void;
  onUpdated?: (name: string, groupId: string) => void;
};

export function CommunityChangeOverlay({
  onClose,
  onUpdated,
}: CommunityChangeOverlayProps) {
  const { activeCommunity, updateCommunity } = useCommunities();
  const [error, setError] = React.useState<string | null>(null);
  const overlayRef = React.useRef<HTMLDivElement>(null);

  // Focus trap: focus the overlay on mount
  React.useEffect(() => {
    overlayRef.current?.focus();
  }, []);

  // Escape key closes the overlay
  React.useEffect(() => {
    function handleKeyDown(event: KeyboardEvent) {
      if (event.key === "Escape") {
        onClose();
      }
    }
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [onClose]);

  const handleSubmit = React.useCallback(
    async (name: string) => {
      if (!activeCommunity) return;
      setError(null);
      try {
        const result = await updateCommunity(activeCommunity.id, { name });
        switch (result.kind) {
          case "unchanged":
            onClose();
            break;
          case "updated":
            onUpdated?.(name, activeCommunity.groupId);
            onClose();
            break;
          case "not-found":
            setError("Community not found.");
            break;
        }
      } catch (cause) {
        setError(cause instanceof Error ? cause.message : String(cause));
      }
    },
    [activeCommunity, onClose, onUpdated, updateCommunity],
  );

  if (!activeCommunity) return null;

  return (
    <div
      aria-modal="true"
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/50 backdrop-blur-sm"
      data-testid="community-change-overlay"
      ref={overlayRef}
      role="dialog"
      tabIndex={-1}
    >
      {/* Background click closes */}
      <div aria-hidden="true" className="absolute inset-0" onClick={onClose} />
      <div className="relative z-10 w-full max-w-md rounded-2xl border border-border bg-background p-8 shadow-2xl">
        <h2 className="text-xl font-semibold tracking-tight">
          Change community name
        </h2>
        <p className="mt-2 text-sm text-muted-foreground">
          Rename this community on this device.
        </p>
        <div className="mt-6">
          <CommunityEditForm
            initialName={activeCommunity.name}
            onCancel={onClose}
            onSubmit={handleSubmit}
            submitLabel="Save changes"
          />
        </div>
        {error ? (
          <p className="mt-4 text-center text-sm text-destructive">{error}</p>
        ) : null}
      </div>
    </div>
  );
}
