import {
  AlertCircle,
  Bot,
  Building2,
  CircleDot,
  Play,
  RotateCcw,
  Users,
} from "lucide-react";
import * as React from "react";
import {
  useCancelCompanyRunMutation,
  useCompanyInstancesQuery,
  useCompanyTemplatesQuery,
  useInstantiateCompanyTemplateMutation,
  useResumeCompanyInstanceMutation,
  useSymphonyDaemonStatusQuery,
  useSymphonyLiveEventCache,
  useSymphonyTasksQuery,
  useSymphonyWorkersQuery,
} from "@/features/symphony/hooks";
import { SymphonyRunDetail } from "@/features/company/ui/SymphonyRunDetail";
import type {
  CompanyInstantiationResult,
  CompanyInstanceStatus,
} from "@/shared/api/symphonyTypes";
import { Badge } from "@/shared/ui/badge";
import { Button } from "@/shared/ui/button";
import { Card } from "@/shared/ui/card";
import { Input } from "@/shared/ui/input";
import { Skeleton } from "@/shared/ui/skeleton";

const ACTIVE_STATUS_LABEL: Record<CompanyInstanceStatus, string> = {
  complete: "complete",
  resumable: "resumable",
  in_progress: "in progress",
  cancelled: "cancelled",
};

export function CompanyScreen() {
  useSymphonyLiveEventCache();
  const templates = useCompanyTemplatesQuery();
  const daemon = useSymphonyDaemonStatusQuery();
  const tasks = useSymphonyTasksQuery();
  const workers = useSymphonyWorkersQuery();
  const instances = useCompanyInstancesQuery();
  const instantiate = useInstantiateCompanyTemplateMutation();
  const resume = useResumeCompanyInstanceMutation();
  const cancelRun = useCancelCompanyRunMutation();
  const [displayName, setDisplayName] = React.useState("");
  const [selectedTemplateId, setSelectedTemplateId] = React.useState<
    string | null
  >(null);
  const [activeRun, setActiveRun] = React.useState<{
    instanceId: string | null;
    runId: string;
  } | null>(null);

  // The persisted Company instances (manifests on disk) are the durable
  // authority for which Symphony runs belong to Company. The runId→instanceId
  // map and the task-stream filter are both derived from this query, which is
  // invalidated on instantiate/cancel/live events — so they survive reload and
  // never admit unrelated global Symphony tasks as Company runs.
  const persistedInstances = instances.data ?? [];
  const runInstanceMap = React.useMemo(() => {
    const map: Record<string, string> = {};
    for (const inst of persistedInstances) {
      if (inst.runId) map[inst.runId] = inst.instanceId;
    }
    return map;
  }, [persistedInstances]);
  const companyRunIds = React.useMemo(() => {
    const ids = new Set<string>();
    for (const inst of persistedInstances) {
      if (inst.runId) ids.add(inst.runId);
    }
    return ids;
  }, [persistedInstances]);
  const companyTasks = React.useMemo(
    () => (tasks.data ?? []).filter((task) => companyRunIds.has(task.id)),
    [tasks.data, companyRunIds],
  );

  // Single-active-company invariant, gated on disk state (not only the live
  // binding). ANY non-cancelled instance — complete OR incomplete — blocks a new
  // instantiation; the operator resumes or cancels the existing one first.
  // Cancelled instances are terminal and excluded everywhere.
  const nonCancelledInstances = persistedInstances.filter(
    (inst) => inst.status !== "cancelled",
  );
  const incompleteInstances = nonCancelledInstances.filter(
    (inst) =>
      inst.status === "in_progress" ||
      inst.status === "resumable" ||
      // Defense-in-depth: a Complete instance lacking a durable runId has no
      // lifecycle proof — treat it as resumable/cancellable, never finished.
      (inst.status === "complete" && !inst.runId),
  );
  const hasNonCancelled = nonCancelledInstances.length > 0;

  const liveActiveId = daemon.data?.activeInstanceId ?? null;
  const activeInstance =
    (liveActiveId
      ? persistedInstances.find((inst) => inst.instanceId === liveActiveId)
      : null) ??
    persistedInstances.find((inst) => inst.active) ??
    null;
  const activeInstanceId = liveActiveId ?? activeInstance?.instanceId ?? null;

  React.useEffect(() => {
    if (!selectedTemplateId && templates.data?.[0]) {
      setSelectedTemplateId(templates.data?.[0].id);
    }
  }, [selectedTemplateId, templates.data]);

  // On a complete attempt (instantiate OR resume), open the run. A resumable or
  // cancelled outcome (runId null) stays on this screen so its state/errors are
  // visible and the operator can resume or cancel.
  React.useEffect(() => {
    const attempt = resume.data ?? instantiate.data;
    const runId = attempt?.runId;
    const instanceId = attempt?.instanceId;
    if (!runId || !instanceId || attempt?.status !== "complete") {
      return;
    }
    setActiveRun({ instanceId, runId });
  }, [resume.data, instantiate.data]);

  if (activeRun) {
    return (
      <SymphonyRunDetail
        instanceId={activeRun.instanceId}
        onLeave={() => setActiveRun(null)}
        runId={activeRun.runId}
      />
    );
  }

  // Surface the most recent attempt's thrown error (resume takes precedence).
  const attemptErrorMessage = (() => {
    const error = resume.error ?? instantiate.error;
    if (error instanceof Error) return error.message;
    return error ? String(error) : null;
  })();
  const attemptResult = (resume.data ?? instantiate.data) as
    | CompanyInstantiationResult
    | undefined;

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

      {/* Incomplete Company instances — resume the exact id or cancel it. A
          resumable failure never deadlocks: the operator resumes (advancing the
          remaining phases idempotently) or cancels (terminal, owns no
          processes). */}
      {incompleteInstances.length > 0 ? (
        <section
          className="mb-5 space-y-2"
          aria-labelledby="company-incomplete-heading"
        >
          <h2 className="text-sm font-semibold" id="company-incomplete-heading">
            Incomplete Company instances
          </h2>
          {incompleteInstances.map((inst) => (
            <Card
              className="flex flex-wrap items-center justify-between gap-3 p-3"
              key={inst.instanceId}
            >
              <div className="min-w-0">
                <code className="break-all text-xs">{inst.instanceId}</code>
                <Badge
                  variant={
                    inst.status === "resumable" ? "outline" : "secondary"
                  }
                >
                  {ACTIVE_STATUS_LABEL[inst.status]}
                </Badge>
              </div>
              <div className="flex gap-2">
                <Button
                  disabled={resume.isPending || cancelRun.isPending}
                  onClick={() => resume.mutate(inst.instanceId)}
                  size="sm"
                >
                  <RotateCcw className="h-4 w-4" />
                  {resume.isPending ? "Resuming…" : "Resume"}
                </Button>
                <Button
                  disabled={cancelRun.isPending || resume.isPending}
                  onClick={() =>
                    cancelRun.mutate({
                      instanceId: inst.instanceId,
                      runId: inst.runId,
                    })
                  }
                  size="sm"
                  variant="destructive"
                >
                  {cancelRun.isPending ? "Cancelling…" : "Cancel"}
                </Button>
              </div>
            </Card>
          ))}
          <p className="text-xs text-muted-foreground">
            A previous run stopped before completion. Resume advances the
            remaining phases idempotently; cancel is terminal and releases any
            owned processes.
          </p>
        </section>
      ) : null}

      {activeInstanceId ? (
        <div className="mb-5 flex flex-wrap items-center justify-between gap-3 rounded-md border border-amber-500/40 bg-amber-500/10 p-3">
          <p className="text-xs text-amber-200">
            Company instance{" "}
            <code className="break-all">{activeInstanceId}</code>
            {` is active${
              activeInstance
                ? ` (${ACTIVE_STATUS_LABEL[activeInstance.status]})`
                : ""
            }. Cancel it before starting a new company.`}
          </p>
          <Button
            disabled={cancelRun.isPending}
            onClick={() => {
              if (activeInstanceId) {
                cancelRun.mutate({
                  instanceId: activeInstanceId,
                  runId: activeInstance?.runId ?? null,
                });
              }
            }}
            size="sm"
            variant="destructive"
          >
            {cancelRun.isPending ? "Cancelling…" : "Cancel active company"}
          </Button>
        </div>
      ) : null}

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
            disabled={
              !selectedTemplateId ||
              instantiate.isPending ||
              resume.isPending ||
              hasNonCancelled
            }
            onClick={() => {
              if (selectedTemplateId) {
                instantiate.mutate({
                  templateId: selectedTemplateId,
                  displayName: displayName.trim() || undefined,
                });
              }
            }}
            title={
              hasNonCancelled
                ? "Another Company instance is already active"
                : undefined
            }
          >
            <Play className="h-4 w-4" />
            {instantiate.isPending ? "Instantiating…" : "Instantiate & run"}
          </Button>
        </div>

        {/* Thrown rejection (e.g. single-active conflict) — rendered verbatim
            so no backend failure is silently absent or generic. */}
        {attemptErrorMessage ? (
          <p className="flex items-start gap-1.5 text-xs text-red-400">
            <AlertCircle className="mt-0.5 h-3.5 w-3.5 shrink-0" />
            <span className="break-words">{attemptErrorMessage}</span>
          </p>
        ) : null}

        {/* Resumable / cancelled attempt outcome — distinct from a run. */}
        {attemptResult?.status === "resumable" ? (
          <p className="flex items-start gap-1.5 text-xs text-amber-300">
            <AlertCircle className="mt-0.5 h-3.5 w-3.5 shrink-0" />
            <span>
              The run stopped before completion (resumable). Resume the instance
              to advance the remaining phases.
            </span>
          </p>
        ) : null}
        {attemptResult?.status === "cancelled" ? (
          <p className="flex items-start gap-1.5 text-xs text-amber-300">
            <AlertCircle className="mt-0.5 h-3.5 w-3.5 shrink-0" />
            <span>
              The run was cancelled. Cancelled runs are terminal — start a new
              company if needed.
            </span>
          </p>
        ) : null}

        {/* Structured result errors. Daemon/run soft-failures are separated
            from resumable provisioning step errors. */}
        {attemptResult?.errors.map((error) => (
          <p
            className={
              error.kind === "daemon"
                ? "flex items-start gap-1.5 text-xs text-red-400"
                : "flex items-start gap-1.5 text-xs text-amber-300"
            }
            key={`${error.kind}-${error.ref}`}
          >
            <AlertCircle className="mt-0.5 h-3.5 w-3.5 shrink-0" />
            <span className="break-words">
              {error.kind === "daemon"
                ? `Daemon/run (${error.ref}): ${error.message}`
                : `${error.kind}${error.ref ? ` ${error.ref}` : ""}: ${error.message}`}
            </span>
          </p>
        ))}
      </section>

      <div className="mt-6 grid gap-5 xl:grid-cols-2">
        <section className="space-y-2" aria-labelledby="company-tasks-heading">
          <h2 className="text-sm font-semibold" id="company-tasks-heading">
            Task stream
          </h2>
          {companyTasks.map((task) => (
            <button
              className="block w-full text-left"
              key={task.id}
              onClick={() =>
                setActiveRun({
                  instanceId: runInstanceMap[task.id] ?? null,
                  runId: task.id,
                })
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
          {daemon.data?.available && companyTasks.length === 0 ? (
            <p className="text-xs text-muted-foreground">
              No Company tasks yet.
            </p>
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
