import { Headphones } from "lucide-react";

import { Button } from "@/shared/ui/button";
import { DropdownMenuItem } from "@/shared/ui/dropdown-menu";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/shared/ui/tooltip";
import { useHuddle } from "../HuddleContext";

type HuddleIndicatorProps = {
  channelId: string;
  className?: string;
  renderMode?: "button" | "menu-item";
  /** Called when the user clicks the button and no huddle is active (start). */
  onStart?: () => void;
  /** Whether the start action is disabled (e.g., permissions, already starting). */
  startDisabled?: boolean;
};

/**
 * Channel-header affordance to start a huddle.
 *
 * M3 cutover: active-huddle detection (kind 48100–48103 relay events) is gone
 * — there is no native per-channel "huddle active" signal, so the indicator no
 * longer offers a join-active-huddle entrypoint here. Starting a huddle is
 * native (`start_huddle`); joining an active huddle is still possible from the
 * huddle attachment rendered on the start message in the thread.
 */
export function HuddleIndicator({
  className,
  renderMode = "button",
  onStart,
  startDisabled,
}: HuddleIndicatorProps) {
  const { isStarting } = useHuddle();

  if (!onStart) return null;

  if (renderMode === "menu-item") {
    return (
      <DropdownMenuItem
        className={className}
        data-testid="channel-start-huddle-trigger"
        disabled={startDisabled || isStarting}
        onSelect={() => onStart()}
      >
        <Headphones />
        <span>Start huddle</span>
      </DropdownMenuItem>
    );
  }

  return (
    <Tooltip disableHoverableContent>
      <TooltipTrigger asChild>
        <span
          className="inline-flex"
          data-testid="channel-huddle-tooltip-trigger"
        >
          <Button
            aria-label="Start huddle"
            className={className}
            data-testid="channel-start-huddle-trigger"
            disabled={startDisabled || isStarting}
            onClick={() => onStart()}
            size="icon"
            type="button"
            variant="outline"
          >
            <Headphones />
          </Button>
        </span>
      </TooltipTrigger>
      <TooltipContent>Huddle</TooltipContent>
    </Tooltip>
  );
}
