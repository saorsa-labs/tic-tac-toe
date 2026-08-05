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
  | "group"
  | "store"
  | "task_list"
  | "agent"
  | "workflow"
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
export type CompanyInstantiationResult = {
  instanceId: string;
  runId: string | null;
  groups: CompanyInstantiatedGroup[];
  agents: CompanyInstantiatedAgent[];
  workflowMd: string | null;
  errors: CompanyInstantiationError[];
};
export type InstantiateCompanyTemplateInput = { displayName?: string };
