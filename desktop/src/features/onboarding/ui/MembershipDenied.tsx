import { ShieldX } from "lucide-react";

import { Badge } from "@/shared/ui/badge";
import { Button } from "@/shared/ui/button";
import { StartupWindowDragRegion } from "@/shared/ui/StartupWindowDragRegion";

type MembershipDeniedProps = {
  onBack: () => void;
  onChangeCommunity: () => void;
  onRetry: () => void;
};

export function MembershipDenied({
  onBack,
  onChangeCommunity,
  onRetry,
}: MembershipDeniedProps) {
  return (
    <div
      className="flex min-h-dvh items-center justify-center bg-[radial-gradient(circle_at_top,hsl(var(--primary)/0.14),transparent_48%),linear-gradient(180deg,hsl(var(--background)),hsl(var(--muted)/0.55))] px-4 py-8"
      data-testid="membership-denied"
    >
      <StartupWindowDragRegion />
      <div className="w-full max-w-md rounded-[28px] border border-border/70 bg-background/92 p-8 shadow-2xl backdrop-blur-sm">
        <div className="space-y-3">
          <Badge variant="warning">Membership required</Badge>
          <div className="flex items-center gap-3">
            <div className="flex h-10 w-10 shrink-0 items-center justify-center rounded-full bg-destructive/10">
              <ShieldX className="h-4 w-4 text-destructive" />
            </div>
            <h1 className="text-2xl font-semibold tracking-tight text-foreground">
              Not a member yet
            </h1>
          </div>
          <p className="text-sm leading-6 text-muted-foreground">
            This group requires an invitation. Ask an administrator to add you
            as a member.
          </p>
        </div>

        <div className="mt-6 flex flex-col gap-2">
          <Button className="w-full" onClick={onRetry} type="button">
            Try again
          </Button>
          <div className="flex gap-2">
            <Button
              className="flex-1 text-muted-foreground hover:text-accent-foreground"
              onClick={onBack}
              type="button"
              variant="ghost"
            >
              Back
            </Button>
            <Button
              className="flex-1 text-muted-foreground hover:text-accent-foreground"
              onClick={onChangeCommunity}
              type="button"
              variant="ghost"
            >
              Change community
            </Button>
          </div>
        </div>
      </div>
    </div>
  );
}
