import { createFileRoute } from "@tanstack/react-router";

// The Nostr-backed project detail screen was removed in the M3 relay cutover
// (no native x0x projects endpoint). `validateSearch` is retained so the
// generated route tree's search-param contract stays satisfied; the component
// renders an explicit unavailable state.
export const Route = createFileRoute("/projects/$projectId")({
  component: ProjectDetailRouteComponent,
  validateSearch: (search: Record<string, unknown>) => ({
    commitHash:
      typeof search.commitHash === "string" ? search.commitHash : undefined,
    pullRequestId:
      typeof search.pullRequestId === "string"
        ? search.pullRequestId
        : undefined,
    issueId: typeof search.issueId === "string" ? search.issueId : undefined,
  }),
});

function ProjectDetailRouteComponent() {
  return (
    <div
      className="flex min-h-0 flex-1 flex-col items-center justify-center gap-2 p-8 text-center"
      data-testid="projects-unavailable"
    >
      <h1 className="text-lg font-semibold">Projects unavailable</h1>
      <p className="max-w-sm text-sm text-muted-foreground">
        Projects are not available in this build.
      </p>
    </div>
  );
}
