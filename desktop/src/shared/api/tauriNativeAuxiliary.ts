/**
 * Exact M3 wrappers for x0xd auxiliary REST surfaces.
 *
 * Wire envelopes mirror `../x0x/src/server/routes/{tasks,stores,identity}.rs`.
 * Feature code receives camelCase values and decoded bytes; it never sees
 * relay kinds, Nostr tags, or a silent compatibility fallback.
 */

import { invokeTauri } from "@/shared/api/tauri";
import type { X0xAgentId, X0xMachineId, X0xUserId } from "./tauriNativeX0x";

export type X0xTaskList = { id: string; topic: string };
export type X0xTaskAction = "claim" | "complete";
export type X0xTask = {
  id: string;
  title: string;
  description: string;
  state: string;
  assignee: X0xAgentId | null;
  priority: number;
  claimedBy: X0xAgentId | null;
  claimedAtMs: number | null;
  completedBy: X0xAgentId | null;
  completedAtMs: number | null;
};

export type X0xTaskListPage = {
  tasks: X0xTask[];
  version: number;
  fenceToken: string;
};

export type X0xLocalMutation = {
  version: number;
  committed: "local";
};

export type X0xTaskCreateReceipt = X0xLocalMutation & { taskId: string };
export type X0xTaskResolution = {
  agentId: X0xAgentId;
  locallyWinning: boolean;
  currentWinner: { agentId: X0xAgentId; timestampMs: number } | null;
  pendingConvergence: true;
};
export type X0xTaskMutationReceipt = X0xLocalMutation & {
  fenceToken: string;
  resolution: X0xTaskResolution;
  casScope: "local_replica";
  authorization: "advisory";
  exclusive: false;
};

type RawTask = {
  id: string;
  title: string;
  description: string;
  state: string;
  assignee: string | null;
  priority: number;
  claimed_by: string | null;
  claimed_at: number | null;
  completed_by: string | null;
  completed_at: number | null;
};

function fromRawTask(task: RawTask): X0xTask {
  return {
    id: task.id,
    title: task.title,
    description: task.description,
    state: task.state,
    assignee: task.assignee,
    priority: task.priority,
    claimedBy: task.claimed_by,
    claimedAtMs: task.claimed_at,
    completedBy: task.completed_by,
    completedAtMs: task.completed_at,
  };
}

export async function x0xListTaskLists(): Promise<X0xTaskList[]> {
  const raw = await invokeTauri<{ task_lists: X0xTaskList[] }>(
    "x0x_list_task_lists",
  );
  return raw.task_lists;
}

export async function x0xCreateTaskList(input: {
  name: string;
  topic: string;
}): Promise<X0xLocalMutation & { id: string; fenceToken: string }> {
  const raw = await invokeTauri<{
    id: string;
    version: number;
    fence_token: string;
    committed: "local";
  }>("x0x_create_task_list", input);
  return {
    id: raw.id,
    version: raw.version,
    fenceToken: raw.fence_token,
    committed: raw.committed,
  };
}

export async function x0xListTasks(listId: string): Promise<X0xTaskListPage> {
  const raw = await invokeTauri<{
    tasks: RawTask[];
    version: number;
    fence_token: string;
  }>("x0x_list_tasks", { listId });
  return {
    tasks: raw.tasks.map(fromRawTask),
    version: raw.version,
    fenceToken: raw.fence_token,
  };
}

export async function x0xAddTask(input: {
  listId: string;
  title: string;
  description?: string;
}): Promise<X0xTaskCreateReceipt> {
  const raw = await invokeTauri<{
    task_id: string;
    version: number;
    committed: "local";
  }>("x0x_add_task", {
    listId: input.listId,
    title: input.title,
    description: input.description ?? null,
  });
  return {
    taskId: raw.task_id,
    version: raw.version,
    committed: raw.committed,
  };
}

export async function x0xUpdateTask(input: {
  listId: string;
  taskId: string;
  action: X0xTaskAction;
  fenceToken?: string;
}): Promise<X0xTaskMutationReceipt> {
  const raw = await invokeTauri<{
    version: number;
    fence_token: string;
    committed: "local";
    resolution: {
      agent_id: string;
      locally_winning: boolean;
      current_winner: { agent_id: string; timestamp_ms: number } | null;
      pending_convergence: true;
    };
    cas: { scope: "local_replica" };
    execution: { authorization: "advisory" };
    exclusive: false;
  }>("x0x_update_task", {
    listId: input.listId,
    taskId: input.taskId,
    action: input.action,
    fenceToken: input.fenceToken ?? null,
  });
  return {
    version: raw.version,
    fenceToken: raw.fence_token,
    committed: raw.committed,
    resolution: {
      agentId: raw.resolution.agent_id,
      locallyWinning: raw.resolution.locally_winning,
      currentWinner:
        raw.resolution.current_winner === null
          ? null
          : {
              agentId: raw.resolution.current_winner.agent_id,
              timestampMs: raw.resolution.current_winner.timestamp_ms,
            },
      pendingConvergence: raw.resolution.pending_convergence,
    },
    casScope: raw.cas.scope,
    authorization: raw.execution.authorization,
    exclusive: raw.exclusive,
  };
}

// ── KV stores ───────────────────────────────────────────────────────────────

export type X0xStorePolicy = "signed" | "append_only";
export type X0xOwnershipStatus = "anchored" | "unknown" | "conflict";
export type X0xStoreSummary = {
  id: string;
  topic: string;
  owner: X0xAgentId | null;
  policy: X0xStorePolicy;
  version: number;
  policyVersion: number;
  ownershipStatus: X0xOwnershipStatus;
  durabilityDegraded: boolean;
};
export type X0xStoreKey = {
  key: string;
  contentType: string;
  contentHash: string;
  size: number;
  updatedAtMs: number;
};
export type X0xKvEntry = {
  key: string;
  value: Uint8Array;
  contentHash: string;
  contentType: string;
  metadata: Record<string, string>;
  createdAtMs: number;
  updatedAtMs: number;
};

type RawStoreSummary = {
  id: string;
  /** Create/join responses may omit topic; list responses always include it. */
  topic?: string;
  owner: string | null;
  policy: X0xStorePolicy;
  version: number;
  policy_version: number;
  ownership_status: X0xOwnershipStatus;
  durability_degraded: boolean;
};
type RawStoreKey = {
  key: string;
  content_type: string;
  content_hash: string;
  size: number;
  updated_at: number;
};
type RawKvEntry = {
  key: string;
  value: string;
  content_hash: string;
  content_type: string;
  metadata: Record<string, string>;
  created_at: number;
  updated_at: number;
};

function fromRawStore(
  store: RawStoreSummary,
  fallbackTopic?: string,
): X0xStoreSummary {
  const topic = store.topic ?? fallbackTopic;
  if (topic === undefined) {
    throw new Error(`x0xd store ${store.id} response omitted its topic`);
  }
  return {
    id: store.id,
    topic,
    owner: store.owner,
    policy: store.policy,
    version: store.version,
    policyVersion: store.policy_version,
    ownershipStatus: store.ownership_status,
    durabilityDegraded: store.durability_degraded,
  };
}

export async function x0xListStores(): Promise<X0xStoreSummary[]> {
  const raw = await invokeTauri<{ stores: RawStoreSummary[] }>(
    "x0x_list_stores",
  );
  return raw.stores.map((store) => fromRawStore(store));
}

export async function x0xCreateStore(input: {
  name: string;
  topic: string;
  policy?: X0xStorePolicy;
}): Promise<X0xStoreSummary> {
  const raw = await invokeTauri<RawStoreSummary>("x0x_create_store", {
    name: input.name,
    topic: input.topic,
    policy: input.policy ?? null,
  });
  return fromRawStore(raw, input.topic);
}

export async function x0xJoinStore(input: {
  storeId: string;
  expectedOwner: X0xAgentId;
}): Promise<X0xStoreSummary> {
  const raw = await invokeTauri<RawStoreSummary>("x0x_join_store", {
    storeId: input.storeId,
    expectedOwner: input.expectedOwner,
  });
  return fromRawStore(raw, input.storeId);
}

export async function x0xListStoreKeys(
  storeId: string,
): Promise<X0xStoreKey[]> {
  const raw = await invokeTauri<{ keys: RawStoreKey[] }>(
    "x0x_list_store_keys",
    {
      storeId,
    },
  );
  return raw.keys.map((key) => ({
    key: key.key,
    contentType: key.content_type,
    contentHash: key.content_hash,
    size: key.size,
    updatedAtMs: key.updated_at,
  }));
}

export async function x0xGetStoreValue(
  storeId: string,
  key: string,
): Promise<X0xKvEntry | null> {
  const raw = await invokeTauri<RawKvEntry | null>("x0x_get_store_value", {
    storeId,
    key,
  });
  if (raw === null) return null;
  return {
    key: raw.key,
    value: base64ToBytes(raw.value),
    contentHash: raw.content_hash,
    contentType: raw.content_type,
    metadata: raw.metadata,
    createdAtMs: raw.created_at,
    updatedAtMs: raw.updated_at,
  };
}

export async function x0xPutStoreValue(input: {
  storeId: string;
  key: string;
  value: Uint8Array;
  contentType?: string;
}): Promise<void> {
  await invokeTauri("x0x_put_store_value", {
    storeId: input.storeId,
    key: input.key,
    valueB64: bytesToBase64(input.value),
    contentType: input.contentType ?? null,
  });
}

export async function x0xDeleteStoreValue(
  storeId: string,
  key: string,
): Promise<void> {
  await invokeTauri("x0x_delete_store_value", { storeId, key });
}

// ── Agent cards ─────────────────────────────────────────────────────────────

export type X0xDmCapabilities = {
  maxProtocolVersion: number;
  gossipInbox: boolean;
  kemAlgorithm: string;
  maxEnvelopeBytes: number;
  kemPublicKey: Uint8Array;
};
export type X0xAgentCard = {
  displayName: string;
  agentId: X0xAgentId;
  machineId: X0xMachineId;
  userId: X0xUserId | null;
  addresses: string[];
  groups: Array<{ name: string; inviteLink: string }>;
  stores: Array<{ name: string; topic: string }>;
  createdAt: number;
  dmCapabilities: X0xDmCapabilities | null;
  agentPublicKey: string | null;
  signature: string | null;
};
export type X0xAgentCardEnvelope = { card: X0xAgentCard; link: string };
export type X0xAgentCardImportReceipt = {
  agentId: X0xAgentId;
  displayName: string;
  trustLevel: string;
  trustChangeIgnored: boolean;
  groupCount: number;
  storeCount: number;
};

type RawAgentCard = {
  display_name: string;
  agent_id: string;
  machine_id: string;
  user_id?: string;
  addresses?: string[];
  groups?: Array<{ name: string; invite_link: string }>;
  stores?: Array<{ name: string; topic: string }>;
  created_at: number;
  dm_capabilities?: {
    max_protocol_version: number;
    gossip_inbox: boolean;
    kem_algorithm: string;
    max_envelope_bytes: number;
    kem_public_key: number[];
  };
  agent_public_key?: string;
  signature?: string;
};

function fromRawAgentCard(card: RawAgentCard): X0xAgentCard {
  const capabilities = card.dm_capabilities;
  return {
    displayName: card.display_name,
    agentId: card.agent_id,
    machineId: card.machine_id,
    userId: card.user_id ?? null,
    addresses: card.addresses ?? [],
    groups: (card.groups ?? []).map((group) => ({
      name: group.name,
      inviteLink: group.invite_link,
    })),
    stores: card.stores ?? [],
    createdAt: card.created_at,
    dmCapabilities:
      capabilities === undefined
        ? null
        : {
            maxProtocolVersion: capabilities.max_protocol_version,
            gossipInbox: capabilities.gossip_inbox,
            kemAlgorithm: capabilities.kem_algorithm,
            maxEnvelopeBytes: capabilities.max_envelope_bytes,
            kemPublicKey: Uint8Array.from(capabilities.kem_public_key),
          },
    agentPublicKey: card.agent_public_key ?? null,
    signature: card.signature ?? null,
  };
}

export async function x0xGetAgentCard(
  input: { displayName?: string; includeGroups?: boolean } = {},
): Promise<X0xAgentCardEnvelope> {
  const raw = await invokeTauri<{ card: RawAgentCard; link: string }>(
    "x0x_get_agent_card",
    {
      displayName: input.displayName ?? null,
      includeGroups: input.includeGroups ?? null,
    },
  );
  return { card: fromRawAgentCard(raw.card), link: raw.link };
}

export async function x0xImportAgentCard(input: {
  card: string;
  trustLevel?: string;
}): Promise<X0xAgentCardImportReceipt> {
  const raw = await invokeTauri<{
    agent_id: string;
    display_name: string;
    trust_level: string;
    trust_change_ignored: boolean;
    groups: number;
    stores: number;
  }>("x0x_import_agent_card", {
    card: input.card,
    trustLevel: input.trustLevel ?? null,
  });
  return {
    agentId: raw.agent_id,
    displayName: raw.display_name,
    trustLevel: raw.trust_level,
    trustChangeIgnored: raw.trust_change_ignored,
    groupCount: raw.groups,
    storeCount: raw.stores,
  };
}

function bytesToBase64(bytes: Uint8Array): string {
  let binary = "";
  const chunkSize = 0x8000;
  for (let i = 0; i < bytes.length; i += chunkSize) {
    binary += String.fromCharCode(...bytes.subarray(i, i + chunkSize));
  }
  return btoa(binary);
}

function base64ToBytes(value: string): Uint8Array {
  const binary = atob(value);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i += 1) {
    bytes[i] = binary.charCodeAt(i);
  }
  return bytes;
}
