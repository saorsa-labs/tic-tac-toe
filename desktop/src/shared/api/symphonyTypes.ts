// Native x0x Symphony and Company product types. These shapes intentionally
// contain no relay event, Nostr key, or relay-fallback fields.

export type SymphonyTaskStatus =
  | "backlog"
  | "claimed"
  | "running"
  | "done"
  | "blocked";

export type SymphonyTask = {
  id: string;
  identifier: string;
  title: string;
  status: SymphonyTaskStatus;
  assigneeId: string | null;
  labels: string[];
  priority: number | null;
};

export type SymphonyWorkerStatus = "idle" | "busy" | "offline";
export type SymphonyAgentId = string;
export type SymphonyAgentCard = {
  agentId: SymphonyAgentId;
  displayName: string;
  role: string | null;
  status: SymphonyWorkerStatus;
  currentLoad: number;
  maxLoad: number;
};
export type SymphonyWorker = SymphonyAgentCard;

export type SymphonyRunStatus =
  | "pending"
  | "running"
  | "completed"
  | "failed"
  | "waiting_approval";
export type SymphonyRun = {
  id: string;
  status: SymphonyRunStatus;
  currentTaskId: string | null;
  error: string | null;
};

export type SymphonyApprovalStatus = "pending" | "granted" | "denied";
export type SymphonyApproval = {
  token: string;
  runId: string;
  taskId: string;
  title: string;
  contentHash: string;
  signerAgentId: SymphonyAgentId;
  status: SymphonyApprovalStatus;
};

export type SymphonyProofKind = "handoff" | "completion";
export type SymphonyProof = {
  id: string;
  runId: string;
  taskId: string;
  kind: SymphonyProofKind;
  summary: string;
};

export type SymphonyDaemonStatus = {
  available: boolean;
  baseUrl: string | null;
  owned: boolean;
  error: string | null;
  /** Bound active Company instance, if any (live supervision signal). */
  activeInstanceId: string | null;
};

export type CompanyGroupKind = "engineering" | "sales" | "all_hands" | "custom";
export type CompanyTemplateGroup = {
  id: string;
  name: string;
  kind: CompanyGroupKind;
};
export type CompanyTemplateAgent = {
  role: string;
  groupId: string;
  runtime: string | null;
  model: string | null;
};
export type CompanyTemplate = {
  id: string;
  name: string;
  description: string | null;
  groups: CompanyTemplateGroup[];
  agents: CompanyTemplateAgent[];
  isBuiltin: boolean;
};

export type CompanyInstantiationErrorKind =
  // Provisioning step labels (resumable failures).
  | "group"
  | "store"
  | "task_list"
  | "symphony_config"
  | "workflow_md"
  // Public group member-add (ref: "{role}→{localGroup}").
  | "membership"
  // Managed-agent identity binding.
  | "agent"
  // Daemon bring-up or run soft-failure (not resumable provisioning).
  | "daemon";
export type CompanyInstantiationError = {
  kind: CompanyInstantiationErrorKind;
  ref: string;
  message: string;
};
export type CompanyInstantiatedGroup = {
  id: string;
  name: string;
  kind: CompanyGroupKind;
};
export type CompanyInstantiatedAgent = {
  agentId: SymphonyAgentId;
  role: string;
  groupId: string;
};
/** Lifecycle status of an instantiation/resume attempt. Provisioning +
 * post-phases either reach `complete`, stop `resumable` (durable, retryable),
 * or are aborted `cancelled` (terminal). `in_progress` is a persisted-manifest
 * status only (between runs); an attempt always returns one of these three. */
export type CompanyInstantiationStatus = "complete" | "resumable" | "cancelled";

/** Lifecycle status of a persisted Company instance (manifest on disk). */
export type CompanyInstanceStatus =
  | "complete"
  | "resumable"
  | "in_progress"
  | "cancelled";

export type CompanyInstantiationResult = {
  instanceId: string;
  runId: string | null;
  /** Lifecycle outcome of the attempt: complete, resumable, or cancelled. */
  status: CompanyInstantiationStatus;
  groups: CompanyInstantiatedGroup[];
  agents: CompanyInstantiatedAgent[];
  workflowMd: string | null;
  errors: CompanyInstantiationError[];
};
export type InstantiateCompanyTemplateInput = { displayName?: string };

/** Persisted Company instance surfaced by `list_company_instances`.
 * Wire is camelCase (Rust struct carries `rename_all = "camelCase"`). */
export type CompanyInstanceSummary = {
  instanceId: string;
  runId: string | null;
  status: CompanyInstanceStatus;
  /** True on the one bound active Company instance, false elsewhere. */
  active: boolean;
};
