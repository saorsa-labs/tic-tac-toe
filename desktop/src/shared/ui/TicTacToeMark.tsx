import { useId } from "react";

import { cn } from "@/shared/lib/cn";

/** Product mark used anywhere the inherited Buzz bee used to be visible. */
export function TicTacToeMark({
  ariaLabel,
  className,
}: {
  ariaLabel?: string;
  className?: string;
}) {
  const titleId = useId();

  return (
    <svg
      aria-hidden={ariaLabel ? undefined : true}
      aria-labelledby={ariaLabel ? titleId : undefined}
      className={cn("shrink-0", className)}
      role={ariaLabel ? "img" : undefined}
      viewBox="0 0 128 128"
      xmlns="http://www.w3.org/2000/svg"
    >
      <title id={titleId}>{ariaLabel ?? "tic-tac-toe mark"}</title>
      <rect fill="#10151a" height="120" rx="26" width="120" x="4" y="4" />
      <g fill="none" stroke="#34414c" strokeLinecap="round" strokeWidth="5">
        <path d="M47 24v80M81 24v80M24 47h80M24 81h80" />
      </g>
      <g fill="none" stroke="#5eead4" strokeLinecap="round" strokeWidth="9">
        <path d="m26 58 16 16m0-16L26 74M86 58l16 16m0-16L86 74" />
      </g>
      <circle
        cx="64"
        cy="66"
        fill="none"
        r="10"
        stroke="#f8fafc"
        strokeWidth="9"
      />
    </svg>
  );
}
