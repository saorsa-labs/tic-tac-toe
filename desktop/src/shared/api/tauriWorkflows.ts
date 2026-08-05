import type {
  ApprovalActionResponse,
  TriggerWorkflowResponse,
  Workflow,
  WorkflowApproval,
  WorkflowRun,
  WorkflowSaveResult,
} from "@/shared/api/types";
import {
  x0xCreateStore,
  x0xDeleteStoreValue,
  x0xGetStoreValue,
  x0xListStoreKeys,
  x0xListStores,
  x0xPutStoreValue,
  type X0xStoreSummary,
} from "@/shared/api/tauriNativeAuxiliary";
import {
  decodeWorkflowDefinition,
  definitionPayloadToWorkflow,
  encodeWorkflowDefinition,
  isWorkflowDefinitionStore,
  WORKFLOW_DEFINITION_STORE_PREFIX,
  WORKFLOW_DEFINITION_STORE_SUFFIX,
  workflowDefinitionStoreCreateInput,
  workflowToDefinitionPayload,
  type WorkflowDefinitionPayload,
} from "@/features/workflows/lib/nativeWorkflowData";

const JSON_CONTENT_TYPE = "application/json";

function textBytes(value: string): Uint8Array {
  return new TextEncoder().encode(value);
}

function decodeText(value: Uint8Array, context: string): string {
  try {
    return new TextDecoder("utf-8", { fatal: true }).decode(value);
  } catch {
    throw new Error(`Native workflow data is not valid UTF-8 (${context}).`);
  }
}

function nativeExecutionUnavailable(surface: string): never {
  throw new Error(
    `${surface} is not exposed by x0xd. Start the Symphony service to use workflow execution; no relay fallback is available.`,
  );
}

async function definitionStores(): Promise<X0xStoreSummary[]> {
  return (await x0xListStores()).filter(
    (store) =>
      store.topic.startsWith(WORKFLOW_DEFINITION_STORE_PREFIX) &&
      store.topic.endsWith(WORKFLOW_DEFINITION_STORE_SUFFIX),
  );
}

async function ensureDefinitionStore(
  groupId: string,
): Promise<X0xStoreSummary> {
  const stores = await x0xListStores();
  const existing = stores.find((store) =>
    isWorkflowDefinitionStore(store, groupId),
  );
  if (existing !== undefined) return existing;

  const input = workflowDefinitionStoreCreateInput(groupId);
  try {
    return await x0xCreateStore(input);
  } catch (error) {
    // A second window may have created the same daemon store between LIST and
    // POST. Re-read once and accept only the exact native topic; otherwise
    // surface the original x0xd failure. This is a native race reconciliation,
    // never a relay fallback.
    const raced = (await x0xListStores()).find((store) =>
      isWorkflowDefinitionStore(store, groupId),
    );
    if (raced !== undefined) return raced;
    throw error;
  }
}

async function readDefinition(
  storeId: string,
  workflowId: string,
): Promise<WorkflowDefinitionPayload | null> {
  const entry = await x0xGetStoreValue(storeId, workflowId);
  if (entry === null) return null;
  const encoded = decodeText(entry.value, `${storeId}/${workflowId}`);
  const payload = decodeWorkflowDefinition(encoded);
  if (payload === null) {
    throw new Error(
      `Native workflow definition ${workflowId} has an unsupported or malformed schema.`,
    );
  }
  return payload;
}

async function workflowsInStore(store: X0xStoreSummary): Promise<Workflow[]> {
  const keys = await x0xListStoreKeys(store.id);
  const workflows = await Promise.all(
    keys.map(async ({ key }) => {
      const payload = await readDefinition(store.id, key);
      return payload === null ? null : definitionPayloadToWorkflow(payload);
    }),
  );
  return workflows.filter(
    (workflow): workflow is Workflow => workflow !== null,
  );
}

async function locateWorkflow(workflowId: string): Promise<{
  store: X0xStoreSummary;
  payload: WorkflowDefinitionPayload;
}> {
  for (const store of await definitionStores()) {
    const payload = await readDefinition(store.id, workflowId);
    if (payload !== null) return { store, payload };
  }
  throw new Error(
    `Workflow ${workflowId} was not found in native x0xd stores.`,
  );
}

export async function getChannelWorkflows(
  channelId: string,
): Promise<Workflow[]> {
  const store = (await x0xListStores()).find((candidate) =>
    isWorkflowDefinitionStore(candidate, channelId),
  );
  return store === undefined ? [] : workflowsInStore(store);
}

export async function getChannelsWorkflows(
  channelIds: string[],
): Promise<Workflow[]> {
  const wanted = new Set(channelIds);
  const stores = (await x0xListStores()).filter((store) => {
    if (
      !store.topic.startsWith(WORKFLOW_DEFINITION_STORE_PREFIX) ||
      !store.topic.endsWith(WORKFLOW_DEFINITION_STORE_SUFFIX)
    ) {
      return false;
    }
    const groupId = store.topic.slice(
      WORKFLOW_DEFINITION_STORE_PREFIX.length,
      -WORKFLOW_DEFINITION_STORE_SUFFIX.length,
    );
    return wanted.has(groupId);
  });
  return (await Promise.all(stores.map(workflowsInStore))).flat();
}

export async function getWorkflow(workflowId: string): Promise<Workflow> {
  const { payload } = await locateWorkflow(workflowId);
  return definitionPayloadToWorkflow(payload);
}

export async function createWorkflow(
  channelId: string,
  yamlDefinition: string,
): Promise<WorkflowSaveResult> {
  const store = await ensureDefinitionStore(channelId);
  const now = Math.floor(Date.now() / 1_000);
  const workflowId = crypto.randomUUID();
  const provisional: Workflow = {
    id: workflowId,
    name: workflowId,
    ownerPubkey: store.owner ?? "",
    channelId,
    definition: {},
    status: "active",
    createdAt: now,
    updatedAt: now,
  };
  const payload = workflowToDefinitionPayload(
    provisional,
    yamlDefinition,
    channelId,
  );
  const projected = definitionPayloadToWorkflow(payload);
  payload.name = projected.name;
  await x0xPutStoreValue({
    storeId: store.id,
    key: workflowId,
    value: textBytes(encodeWorkflowDefinition(payload)),
    contentType: JSON_CONTENT_TYPE,
  });
  return {
    workflow: { ...projected, ownerPubkey: store.owner ?? "" },
    webhookSecret: null,
  };
}

export async function updateWorkflow(
  workflowId: string,
  yamlDefinition: string,
): Promise<WorkflowSaveResult> {
  const { store, payload: previous } = await locateWorkflow(workflowId);
  const previousWorkflow = definitionPayloadToWorkflow(previous);
  const updated = workflowToDefinitionPayload(
    { ...previousWorkflow, updatedAt: Math.floor(Date.now() / 1_000) },
    yamlDefinition,
    previous.group_id,
  );
  const projected = definitionPayloadToWorkflow(updated);
  updated.name = projected.name;
  await x0xPutStoreValue({
    storeId: store.id,
    key: workflowId,
    value: textBytes(encodeWorkflowDefinition(updated)),
    contentType: JSON_CONTENT_TYPE,
  });
  return {
    workflow: { ...projected, ownerPubkey: store.owner ?? "" },
    webhookSecret: null,
  };
}

export async function deleteWorkflow(workflowId: string): Promise<void> {
  const { store } = await locateWorkflow(workflowId);
  await x0xDeleteStoreValue(store.id, workflowId);
}

export async function getWorkflowRuns(
  _workflowId: string,
  _limit?: number,
): Promise<WorkflowRun[]> {
  return nativeExecutionUnavailable("Workflow run history");
}

export async function getRunApprovals(
  _workflowId: string,
  _runId: string,
): Promise<WorkflowApproval[]> {
  return nativeExecutionUnavailable("Workflow approvals");
}

export async function triggerWorkflow(
  _workflowId: string,
): Promise<TriggerWorkflowResponse> {
  return nativeExecutionUnavailable("Workflow triggering");
}

export async function grantApproval(
  _token: string,
  _note?: string,
): Promise<ApprovalActionResponse> {
  return nativeExecutionUnavailable("Workflow approval grants");
}

export async function denyApproval(
  _token: string,
  _note?: string,
): Promise<ApprovalActionResponse> {
  return nativeExecutionUnavailable("Workflow approval denials");
}
