import { invokeTauri } from "@/shared/api/tauri";
import type {
  CompanyInstantiationResult,
  CompanyInstanceStatus,
  CompanyInstanceSummary,
  CompanyTemplate,
  InstantiateCompanyTemplateInput,
  SymphonyAgentCard,
  SymphonyApproval,
  SymphonyDaemonStatus,
  SymphonyProof,
  SymphonyRun,
  SymphonyTask,
} from "@/shared/api/symphonyTypes";

type RawSupervisionStatus = {
  running: boolean;
  base_url: string | null;
  owned: boolean;
  active_instance_id: string | null;
  error: string | null;
};

type RawTask = {
  id: string;
  identifier: string;
  title: string;
  state: string;
  priority?: number | null;
  labels: string[];
  claim_by?: string | null;
};

type RawWorker = {
  agent_id: string;
  current_load: number;
  max_load: number;
  runner_presets: string[];
};

type RawWorkers = { workers: RawWorker[]; view_epoch: number };

type RawPendingApproval = {
  issue_id: string;
  title: string;
  state: string;
  content_hash: string;
  signer_agent_id: string;
};

type RawProofList = { proofs: string[] };
type RawProof = { name: string; content: string };

type RawCompanyTemplate = {
  id: string;
  name: string;
  description: string | null;
  groups: {
    id: string;
    name: string;
    kind: CompanyTemplate["groups"][number]["kind"];
  }[];
  agents: {
    role: string;
    group_id: string;
    runtime: string | null;
    model: string | null;
  }[];
  is_builtin: boolean;
};

type RawCompanyInstantiationResult = {
  instance_id: string;
  run_id: string | null;
  // Provisioning outcome (ManifestStatus serde snake_case). Absent on the
  // pre-cutover fixture; a present run implies "complete".
  status?: string;
  groups: CompanyInstantiationResult["groups"];
  agents: { agent_id: string; role: string; group_id: string }[];
  workflow_md: string | null;
  errors: CompanyInstantiationResult["errors"];
};

function taskStatus(state: string): SymphonyTask["status"] {
  if (state === "done" || state === "completed") return "done";
  if (state === "blocked") return "blocked";
  if (state === "in_progress" || state === "running") return "running";
  if (state === "claimed") return "claimed";
  return "backlog";
}

export function fromRawTask(raw: RawTask): SymphonyTask {
  return {
    id: raw.id,
    identifier: raw.identifier,
    title: raw.title,
    status: taskStatus(raw.state),
    assigneeId: raw.claim_by ?? null,
    labels: raw.labels,
    priority: raw.priority ?? null,
  };
}

export function fromRawWorker(raw: RawWorker): SymphonyAgentCard {
  return {
    agentId: raw.agent_id,
    displayName: raw.agent_id,
    role: raw.runner_presets[0] ?? null,
    status: raw.current_load > 0 ? "busy" : "idle",
    currentLoad: raw.current_load,
    maxLoad: raw.max_load,
  };
}

export function fromRawTemplate(raw: RawCompanyTemplate): CompanyTemplate {
  return {
    id: raw.id,
    name: raw.name,
    description: raw.description,
    groups: raw.groups,
    agents: raw.agents.map((agent) => ({
      role: agent.role,
      groupId: agent.group_id,
      runtime: agent.runtime,
      model: agent.model,
    })),
    isBuiltin: raw.is_builtin,
  };
}

export function fromRawInstantiationResult(
  raw: RawCompanyInstantiationResult,
): CompanyInstantiationResult {
  // ManifestStatus serializes snake_case. The attempt is terminal in one of:
  // complete, resumable (durable, retryable), or cancelled (terminal). An
  // unexpected/in_progress token is treated as resumable (never "complete").
  const status: CompanyInstantiationResult["status"] =
    raw.status === "complete"
      ? "complete"
      : raw.status === "cancelled"
        ? "cancelled"
        : "resumable";
  return {
    instanceId: raw.instance_id,
    runId: raw.run_id,
    groups: raw.groups,
    agents: raw.agents.map((agent) => ({
      agentId: agent.agent_id,
      role: agent.role,
      groupId: agent.group_id,
    })),
    workflowMd: raw.workflow_md,
    errors: raw.errors,
    status,
  };
}

export async function getSymphonyDaemonStatus(): Promise<SymphonyDaemonStatus> {
  const raw = await invokeTauri<RawSupervisionStatus>(
    "symphony_supervision_status",
  );
  return {
    available: raw.running,
    baseUrl: raw.base_url,
    owned: raw.owned,
    activeInstanceId: raw.active_instance_id ?? null,
    // Prefer the backend's captured bring-up error; synthesize a label only
    // when the daemon is down and the backend supplied no detail.
    error: raw.error ?? (raw.running ? null : "x0x-symphonyd is not running"),
  };
}

export async function listSymphonyTasks(): Promise<SymphonyTask[]> {
  return (
    await invokeTauri<RawTask[]>("symphony_tasks", { stateFilter: null })
  ).map(fromRawTask);
}

export async function listSymphonyWorkers(): Promise<SymphonyAgentCard[]> {
  const raw = await invokeTauri<RawWorkers>("symphony_workers");
  return raw.workers.map(fromRawWorker);
}

export async function getSymphonyRun(runId: string): Promise<SymphonyRun> {
  const task = await invokeTauri<RawTask>("symphony_task", { id: runId });
  const status = taskStatus(task.state);
  return {
    id: task.id,
    status:
      status === "done"
        ? "completed"
        : status === "blocked"
          ? "waiting_approval"
          : status === "backlog" || status === "claimed"
            ? "pending"
            : "running",
    currentTaskId: task.id,
    error: null,
  };
}

export async function listSymphonyApprovals(
  runId: string,
): Promise<SymphonyApproval[]> {
  const rows = await invokeTauri<RawPendingApproval[]>(
    "symphony_approvals_pending",
  );
  return rows
    .filter((row) => row.issue_id === runId)
    .map((row) => ({
      token: row.issue_id,
      runId: row.issue_id,
      taskId: row.issue_id,
      title: row.title,
      contentHash: row.content_hash,
      signerAgentId: row.signer_agent_id,
      status: "pending",
    }));
}

export async function approveSymphonyTask(
  approval: SymphonyApproval,
): Promise<void> {
  await invokeTauri("symphony_approve", {
    id: approval.taskId,
    expectedContentHash: approval.contentHash,
    expectedSignerAgentId: approval.signerAgentId,
  });
}

export async function denySymphonyTask(
  approval: SymphonyApproval,
): Promise<void> {
  await invokeTauri("symphony_deny", {
    id: approval.taskId,
    expectedContentHash: approval.contentHash,
    expectedSignerAgentId: approval.signerAgentId,
  });
}

export async function listSymphonyProofs(
  runId: string,
): Promise<SymphonyProof[]> {
  const list = await invokeTauri<RawProofList>("symphony_proofs");
  const names = list.proofs.filter((name) => name.includes(runId));
  return Promise.all(
    names.map(async (name) => {
      const proof = await invokeTauri<RawProof>("symphony_proof", { name });
      return {
        id: proof.name,
        runId,
        taskId: runId,
        kind: proof.name.includes("handoff") ? "handoff" : "completion",
        summary: proof.content,
      };
    }),
  );
}

export async function listCompanyTemplates(): Promise<CompanyTemplate[]> {
  return (
    await invokeTauri<RawCompanyTemplate[]>("list_company_templates")
  ).map(fromRawTemplate);
}

export async function instantiateCompanyTemplate(
  templateId: string,
  input: InstantiateCompanyTemplateInput = {},
): Promise<CompanyInstantiationResult> {
  return fromRawInstantiationResult(
    await invokeTauri<RawCompanyInstantiationResult>(
      "instantiate_company_template",
      {
        templateId,
        input,
      },
    ),
  );
}

/** Resume an existing in_progress/resumable Company instance by its exact id.
 * Never mints a new id: provisioning skips completed steps and the post-phases
 * advance idempotently to complete, or remain honestly resumable. */
export async function resumeCompanyInstance(
  instanceId: string,
): Promise<CompanyInstantiationResult> {
  return fromRawInstantiationResult(
    await invokeTauri<RawCompanyInstantiationResult>(
      "resume_company_instance",
      {
        instanceId,
      },
    ),
  );
}

export async function subscribeSymphonyEvents(): Promise<void> {
  await invokeTauri("symphony_subscribe_events");
}

export async function cancelCompanyRun(instanceId: string): Promise<void> {
  await invokeTauri("cancel_company_run", { instanceId });
}

export async function listCompanyInstances(): Promise<
  CompanyInstanceSummary[]
> {
  // Wire is camelCase (Rust CompanyInstanceSummary has rename_all="camelCase");
  // reading snake_case here previously yielded undefined instance ids. The
  // status/active fields are additive and may be absent pre-cutover, so they
  // are defaulted rather than required.
  const raw = await invokeTauri<
    {
      instanceId: string;
      runId: string | null;
      status?: string;
      active?: boolean;
    }[]
  >("list_company_instances");
  return raw.map((r) => {
    // Known statuses pass through; an UNKNOWN wire status must never default
    // to "complete" (complete is a lifecycle-proven claim owned by the
    // backend). Default to the safe, cancellable/resumable state instead.
    const status: CompanyInstanceStatus =
      r.status === "complete" ||
      r.status === "resumable" ||
      r.status === "in_progress" ||
      r.status === "cancelled"
        ? r.status
        : "resumable";
    return {
      instanceId: r.instanceId,
      runId: r.runId,
      status,
      active: r.active ?? false,
    };
  });
}
