import {
  fromRawManagedAgent,
  invokeTauri,
  type RawManagedAgent,
} from "@/shared/api/tauri";
import type {
  ManagedAgent,
  ManagedAgentMentionWakeInput,
  ManagedAgentRuntimeStatus,
} from "@/shared/api/types";

export async function startManagedAgent(pubkey: string): Promise<ManagedAgent> {
  const response = await invokeTauri<RawManagedAgent>("start_managed_agent", {
    pubkey,
  });
  return fromRawManagedAgent(response);
}

export async function stopManagedAgent(pubkey: string): Promise<ManagedAgent> {
  const response = await invokeTauri<RawManagedAgent>("stop_managed_agent", {
    pubkey,
  });
  return fromRawManagedAgent(response);
}

export async function setManagedAgentStartOnAppLaunch(
  pubkey: string,
  startOnAppLaunch: boolean,
): Promise<ManagedAgent> {
  const response = await invokeTauri<RawManagedAgent>(
    "set_managed_agent_start_on_app_launch",
    {
      pubkey,
      startOnAppLaunch,
    },
  );
  return fromRawManagedAgent(response);
}

export async function setManagedAgentAutoRestart(
  pubkey: string,
  autoRestartOnConfigChange: boolean,
): Promise<ManagedAgent> {
  const response = await invokeTauri<RawManagedAgent>(
    "set_managed_agent_auto_restart",
    {
      pubkey,
      autoRestartOnConfigChange,
    },
  );
  return fromRawManagedAgent(response);
}

export async function listManagedAgentRuntimes(): Promise<
  ManagedAgentRuntimeStatus[]
> {
  return invokeTauri<ManagedAgentRuntimeStatus[]>(
    "list_managed_agent_runtimes",
  );
}

export async function startManagedAgentRuntime(
  pubkey: string,
  groupId: string,
): Promise<ManagedAgentRuntimeStatus> {
  return invokeTauri("start_managed_agent_runtime", { pubkey, groupId });
}

/**
 * Revalidate and hand off one exact signed group mention to a managed child.
 * The backend, not the renderer, authorizes the row and durably stages its
 * canonical msg_id before starting the exact (record, group) runtime.
 */
export async function wakeManagedAgentFromMention(
  input: ManagedAgentMentionWakeInput,
): Promise<ManagedAgentRuntimeStatus> {
  return invokeTauri("wake_managed_agent_from_mention", { input });
}

export async function stopManagedAgentRuntime(
  pubkey: string,
  groupId: string,
): Promise<ManagedAgentRuntimeStatus> {
  return invokeTauri("stop_managed_agent_runtime", { pubkey, groupId });
}

export async function restartManagedAgentRuntime(
  pubkey: string,
  groupId: string,
): Promise<ManagedAgentRuntimeStatus> {
  return invokeTauri("restart_managed_agent_runtime", { pubkey, groupId });
}

export async function putManagedAgentRuntimeLifecycle(
  outerPubkey: string,
  payload: unknown,
): Promise<ManagedAgentRuntimeStatus> {
  return invokeTauri("put_managed_agent_runtime_lifecycle", {
    outerPubkey,
    payload,
  });
}

export async function reconcileManagedAgentRuntimes(
  communities: readonly { groupId: string }[],
): Promise<ManagedAgentRuntimeStatus[]> {
  return invokeTauri("reconcile_managed_agent_runtimes", { communities });
}
