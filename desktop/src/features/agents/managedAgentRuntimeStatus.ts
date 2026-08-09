import type { ManagedAgentRuntimeStatus } from "@/shared/api/types";

export type AgentCommunityAvailability =
  | "Here"
  | "Waking"
  | "Needs setup on this device"
  | "Unavailable";

export function agentCommunityAvailability(
  runtime: ManagedAgentRuntimeStatus,
): AgentCommunityAvailability {
  if (!runtime.localSetup) return "Needs setup on this device";

  switch (runtime.lifecycle) {
    case "starting":
    case "listening":
    case "waking":
      return "Waking";
    case "ready":
      return "Here";
    case "failed":
    case "stopped":
      return "Unavailable";
  }
}

export function agentCommunityStatusDetail(
  runtime: ManagedAgentRuntimeStatus,
): string | null {
  if (!runtime.localSetup)
    return "Set up this agent on this device to start it.";
  if (runtime.lifecycle === "stopped") return "Stopped by you";
  if (runtime.lifecycle === "failed")
    return runtime.error ?? "Could not connect";
  return null;
}

export function managedAgentRuntimeKey(
  runtime: Pick<ManagedAgentRuntimeStatus, "pubkey" | "groupId">,
): string {
  return JSON.stringify([runtime.pubkey, runtime.groupId]);
}

export type ManagedAgentPairAction = "start" | "stop" | "restart";

/** Menu action for one agent+community pair. A missing runtime row means the
 * pair is not running here, so the only sensible action is to start it. */
export function managedAgentPairAction(
  runtime: ManagedAgentRuntimeStatus | undefined,
): ManagedAgentPairAction {
  if (!runtime || runtime.lifecycle === "stopped") return "start";
  if (runtime.lifecycle === "failed") return "restart";
  return "stop";
}

export const MANAGED_AGENT_PAIR_ACTION_LABELS: Record<
  ManagedAgentPairAction,
  string
> = {
  start: "Start",
  stop: "Stop",
  restart: "Restart",
};

/** Find one managed-agent runtime scoped to its native group. */
export function findManagedAgentRuntime(
  runtimes: readonly ManagedAgentRuntimeStatus[],
  pubkey: string,
  groupId: string,
): ManagedAgentRuntimeStatus | undefined {
  const normalizedPubkey = pubkey.toLowerCase();
  return runtimes.find(
    (runtime) =>
      runtime.pubkey.toLowerCase() === normalizedPubkey &&
      (runtime.groupId === groupId || runtime.requestedGroupId === groupId),
  );
}
