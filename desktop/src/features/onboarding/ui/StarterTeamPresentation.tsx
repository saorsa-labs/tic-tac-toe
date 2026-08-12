import { cn } from "@/shared/lib/cn";
import { TicTacToeMark } from "@/shared/ui/TicTacToeMark";

export type StarterTeamMarkKind = "board" | "cross" | "circle";

export type StarterTeamPresentationMember = Readonly<{
  id: string;
  label: string;
  mark: StarterTeamMarkKind;
}>;

/** Product-neutral identities shown while the built-in Welcome Team is set up. */
export const STARTER_TEAM_PRESENTATION = [
  { id: "guide", label: "Guide", mark: "board" },
  { id: "cross", label: "X", mark: "cross" },
  { id: "circle", label: "O", mark: "circle" },
] as const satisfies readonly StarterTeamPresentationMember[];

export function StarterTeamMark({
  className,
  mark,
}: {
  className?: string;
  mark: StarterTeamMarkKind;
}) {
  if (mark === "board") {
    return <TicTacToeMark className={className} />;
  }

  return (
    <span
      aria-hidden="true"
      className={cn(
        "flex aspect-square shrink-0 items-center justify-center rounded-[22%] bg-[#10151a] shadow-sm",
        className,
      )}
    >
      <svg
        aria-hidden="true"
        className="h-[58%] w-[58%]"
        viewBox="0 0 64 64"
        xmlns="http://www.w3.org/2000/svg"
      >
        {mark === "cross" ? (
          <g fill="none" stroke="#5eead4" strokeLinecap="round" strokeWidth="9">
            <path d="m14 14 36 36" />
            <path d="M50 14 14 50" />
          </g>
        ) : (
          <circle
            cx="32"
            cy="32"
            fill="none"
            r="20"
            stroke="#f8fafc"
            strokeWidth="9"
          />
        )}
      </svg>
    </span>
  );
}
