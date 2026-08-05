import { Bot, Building2, CircleDot, Play, Users } from "lucide-react";
import * as React from "react";

import {
  useCompanyTemplatesQuery,
  useInstantiateCompanyTemplateMutation,
  useSymphonyDaemonStatusQuery,
  useSymphonyLiveEventCache,
  useSymphonyTasksQuery,
  useSymphonyWorkersQuery,
} from "@/features/symphony/hooks";
import { SymphonyRunDetail } from "@/features/company/ui/SymphonyRunDetail";
import type { CompanyInstantiationResult } from "@/shared/api/symphonyTypes";
import { Badge } from "@/shared/ui/badge";
import { Button } from "@/shared/ui/button";
import { Card } from "@/shared/ui/card";
import { Input } from "@/shared/ui/input";
import { Skeleton } from "@/shared/ui/skeleton";

export function CompanyScreen() {
  useSymphonyLiveEventCache();
  const templates = useCompanyTemplatesQuery();
  const daemon = useSymphonyDaemonStatusQuery();
  const tasks = useSymphonyTasksQuery();
  const workers = useSymphonyWorkersQuery();
  const instantiate = useInstantiateCompanyTemplateMutation();
  const [displayName, setDisplayName] = React.useState("");
  const [selectedTemplateId, setSelectedTemplateId] = React.useState<
    string | null
  >(null);
  const [activeRun, setActiveRun] = React.useState<{
    instanceId: string;
    runId: string;
  } | null>(null);
  const result = instantiate.data as CompanyInstantiationResult | undefined;

  React.useEffect(() => {
    if (!selectedTemplateId && templates.data?.[0]) {
      setSelectedTemplateId(templates.data[0].id);
    }
  }, [selectedTemplateId, templates.data]);

  React.useEffect(() => {
    if (result?.runId) {
      setActiveRun({ instanceId: result.instanceId, runId: result.runId });
    }
  }, [result]);

  if (activeRun) {
    return (
      <SymphonyRunDetail
        instanceId={activeRun.instanceId}
        onLeave={() => setActiveRun(null)}
        runId={activeRun.runId}
      />
    );
  }

  return (
    <main
      className="flex h-full min-h-0 flex-col overflow-y-auto p-5"
      data-testid="company-screen"
    >
      <div className="mb-5 flex items-start justify-between gap-4">
        <div>
          <h1 className="flex items-center gap-2 text-xl font-semibold">
            <Building2 className="h-5 w-5" /> Company
          </h1>
          <p className="mt-1 text-sm text-muted-foreground">
            Instantiate an x0x-native team and supervise its work with Symphony.
          </p>
        </div>
        <Badge variant={daemon.data?.available ? "success" : "secondary"}>
          {daemon.data?.available
            ? "Symphony online"
            : "Symphony starts on run"}
        </Badge>
      </div>

      <section className="space-y-3" aria-labelledby="company-template-heading">
        <h2 className="text-sm font-semibold" id="company-template-heading">
          Company template
        </h2>
        {templates.isLoading ? <Skeleton className="h-36 w-full" /> : null}
        <div className="grid gap-3 lg:grid-cols-2">
          {templates.data?.map((template) => (
            <button
              aria-pressed={selectedTemplateId === template.id}
              className="text-left"
              key={template.id}
              onClick={() => setSelectedTemplateId(template.id)}
              type="button"
            >
              <Card
                className={
                  selectedTemplateId === template.id
                    ? "h-full border-primary p-4"
                    : "h-full p-4"
                }
              >
                <div className="flex items-center justify-between">
                  <span className="font-medium">{template.name}</span>
                  {template.isBuiltin ? (
                    <Badge variant="outline">Built in</Badge>
                  ) : null}
                </div>
                <p className="mt-1 text-xs text-muted-foreground">
                  {template.description}
                </p>
                <div className="mt-3 flex gap-3 text-xs text-muted-foreground">
                  <span className="flex items-center gap-1">
                    <Users className="h-3.5 w-3.5" /> {template.groups.length}{" "}
                    groups
                  </span>
                  <span className="flex items-center gap-1">
                    <Bot className="h-3.5 w-3.5" /> {template.agents.length}{" "}
                    agents
                  </span>
                </div>
              </Card>
            </button>
          ))}
        </div>
        <div className="flex max-w-xl gap-2">
          <Input
            aria-label="Company display name"
            onChange={(event) => setDisplayName(event.target.value)}
            placeholder="Company name (optional)"
            value={displayName}
          />
          <Button
            disabled={!selectedTemplateId || instantiate.isPending}
            onClick={() => {
              if (selectedTemplateId) {
                instantiate.mutate({
                  templateId: selectedTemplateId,
                  displayName: displayName.trim() || undefined,
                });
              }
            }}
          >
            <Play className="h-4 w-4" />
            {instantiate.isPending ? "Instantiating…" : "Instantiate & run"}
          </Button>
        </div>
        {result?.errors.map((error) => (
          <p
            className="text-xs text-red-400"
            key={`${error.kind}-${error.ref}`}
          >
            {error.kind} {error.ref}: {error.message}
          </p>
        ))}
      </section>

      <div className="mt-6 grid gap-5 xl:grid-cols-2">
        <section className="space-y-2" aria-labelledby="company-tasks-heading">
          <h2 className="text-sm font-semibold" id="company-tasks-heading">
            Task stream
          </h2>
          {tasks.data?.map((task) => (
            <button
              className="block w-full text-left"
              key={task.id}
              onClick={() =>
                setActiveRun({ instanceId: task.id, runId: task.id })
              }
              type="button"
            >
              <Card className="flex items-center justify-between p-3">
                <div>
                  <p className="text-sm font-medium">{task.title}</p>
                  <p className="text-xs text-muted-foreground">
                    {task.identifier}
                  </p>
                </div>
                <Badge variant="outline">{task.status}</Badge>
              </Card>
            </button>
          ))}
          {daemon.data?.available && tasks.data?.length === 0 ? (
            <p className="text-xs text-muted-foreground">No tasks yet.</p>
          ) : null}
        </section>

        <section
          className="space-y-2"
          aria-labelledby="company-workers-heading"
        >
          <h2 className="text-sm font-semibold" id="company-workers-heading">
            Worker stream
          </h2>
          {workers.data?.map((worker) => (
            <Card
              className="flex items-center justify-between p-3"
              key={worker.agentId}
            >
              <code className="break-all text-xs" title="Native x0x AgentId">
                {worker.agentId}
              </code>
              <span className="flex items-center gap-1 text-xs text-muted-foreground">
                <CircleDot className="h-3 w-3" /> {worker.status} ·{" "}
                {worker.currentLoad}/{worker.maxLoad}
              </span>
            </Card>
          ))}
          {daemon.data?.available && workers.data?.length === 0 ? (
            <p className="text-xs text-muted-foreground">
              No workers advertising yet.
            </p>
          ) : null}
        </section>
      </div>
    </main>
  );
}
