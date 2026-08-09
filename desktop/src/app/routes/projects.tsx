import { createFileRoute } from "@tanstack/react-router";

// The Nostr-backed projects feature (repo announcements, issues, pull requests,
// reviews) has no native x0x endpoint and was removed in the M3 relay cutover.
// The route is retained so navigation and history entries land on an explicit
// unavailable state instead of dead-ending; the relay transport and its UI
// tree live nowhere in the production graph.
export const Route = createFileRoute("/projects")({
  component: ProjectsRouteComponent,
});

function ProjectsRouteComponent() {
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
