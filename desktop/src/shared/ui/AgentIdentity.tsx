import { Check, Copy } from "lucide-react";
import * as React from "react";

import { copyTextToClipboard } from "@/shared/lib/clipboard";
import { cn } from "@/shared/lib/cn";
import { Button } from "@/shared/ui/button";
import { Popover, PopoverContent, PopoverTrigger } from "@/shared/ui/popover";

const HOVER_OPEN_DELAY_MS = 500;
const HOVER_CLOSE_DELAY_MS = 200;

export type AgentIdentityProps = {
  /** x0x AgentId — exact 64-hex. Copied as the canonical machine identity. */
  agentId: string;
  /** Four speakable words derived from `agentId`. The displayed identity. */
  identityWords: string[];
  /**
   * `compact` (default) — the four words; hover/click opens a popover with the
   * full AgentId and copy buttons. The default for identity display in lists,
   * cards, and metadata rows.
   *
   * `full` — the four words rendered inline with a copy button; the popover
   * carries the complete AgentId for verification on trust-decision surfaces.
   */
  variant?: "compact" | "full";
  className?: string;
  testId?: string;
};

function CopyRow({ label, value }: { label: string; value: string }) {
  const [copied, setCopied] = React.useState(false);
  const resetTimer = React.useRef<number | undefined>(undefined);
  React.useEffect(() => () => window.clearTimeout(resetTimer.current), []);

  return (
    <div className="flex min-w-0 items-start gap-1.5">
      <div className="min-w-0 flex-1">
        <div className="text-2xs font-medium text-muted-foreground">
          {label}
        </div>
        <div className="break-all font-mono text-xs">{value}</div>
      </div>
      <Button
        aria-label={`Copy ${label}`}
        onClick={() => {
          copyTextToClipboard(value, `${label} copied`);
          setCopied(true);
          window.clearTimeout(resetTimer.current);
          resetTimer.current = window.setTimeout(() => setCopied(false), 1500);
        }}
        size="icon-xs"
        type="button"
        variant="ghost"
      >
        {copied ? <Check /> : <Copy />}
      </Button>
    </div>
  );
}

function joinWords(identityWords: string[]): string {
  return identityWords.filter((word) => word.length > 0).join(" ");
}

function AgentIdentityDetails({
  agentId,
  identityWords,
}: {
  agentId: string;
  identityWords: string[];
}) {
  const words = joinWords(identityWords);
  return (
    <div className="space-y-2">
      {words ? <CopyRow label="Identity" value={words} /> : null}
      <CopyRow label="Agent ID" value={agentId} />
    </div>
  );
}

/**
 * Canonical displayed-identity renderer. Shows the four speakable AgentId words
 * — never a bech32 (npub/nsec) form and never the internal relay signer
 * pubkey. The raw 64-hex AgentId is available to copy (and, in the `full`
 * variant, to verify) but is not the primary display.
 */
export function AgentIdentity({
  agentId,
  identityWords,
  variant = "compact",
  className,
  testId,
}: AgentIdentityProps) {
  const [open, setOpen] = React.useState(false);
  const hoverTimerRef = React.useRef<ReturnType<typeof setTimeout> | null>(
    null,
  );
  const words = joinWords(identityWords);

  const clearHoverTimer = React.useCallback(() => {
    if (hoverTimerRef.current !== null) {
      clearTimeout(hoverTimerRef.current);
      hoverTimerRef.current = null;
    }
  }, []);

  const handleTriggerMouseEnter = React.useCallback(() => {
    clearHoverTimer();
    hoverTimerRef.current = setTimeout(() => {
      setOpen(true);
    }, HOVER_OPEN_DELAY_MS);
  }, [clearHoverTimer]);

  const handleMouseLeave = React.useCallback(() => {
    clearHoverTimer();
    hoverTimerRef.current = setTimeout(() => {
      setOpen(false);
    }, HOVER_CLOSE_DELAY_MS);
  }, [clearHoverTimer]);

  const handleContentMouseEnter = React.useCallback(() => {
    clearHoverTimer();
  }, [clearHoverTimer]);

  React.useEffect(() => clearHoverTimer, [clearHoverTimer]);

  if (variant === "full") {
    return (
      <span
        className={cn("inline-flex min-w-0 items-center gap-1", className)}
        data-testid={testId}
      >
        <span className="break-words font-medium text-xs">{words}</span>
        <Popover>
          <PopoverTrigger asChild>
            <Button
              aria-label="Copy agent identity"
              size="icon-xs"
              type="button"
              variant="ghost"
            >
              <Copy />
            </Button>
          </PopoverTrigger>
          <PopoverContent align="start" className="w-96 max-w-[90vw]">
            <AgentIdentityDetails
              agentId={agentId}
              identityWords={identityWords}
            />
          </PopoverContent>
        </Popover>
      </span>
    );
  }

  return (
    <Popover onOpenChange={setOpen} open={open}>
      <PopoverTrigger asChild>
        <button
          aria-label={`Show agent identity ${words}`}
          className={cn(
            "cursor-pointer rounded font-medium hover:underline focus-visible:outline-hidden focus-visible:ring-1 focus-visible:ring-ring",
            className,
          )}
          data-testid={testId}
          onMouseEnter={handleTriggerMouseEnter}
          onMouseLeave={handleMouseLeave}
          type="button"
        >
          {words}
        </button>
      </PopoverTrigger>
      <PopoverContent
        align="start"
        className="w-96 max-w-[90vw]"
        onMouseEnter={handleContentMouseEnter}
        onMouseLeave={handleMouseLeave}
        onOpenAutoFocus={(event) => event.preventDefault()}
      >
        <AgentIdentityDetails agentId={agentId} identityWords={identityWords} />
      </PopoverContent>
    </Popover>
  );
}
