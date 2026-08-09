import type { ManagedAgent } from "@/shared/api/types";

type DeleteManagedAgentInput = {
  pubkey: string;
  forceRemoteDelete?: boolean;
};

type StartManagedAgent = (pubkey: string) => Promise<unknown>;
type StopManagedAgent = (pubkey: string) => Promise<unknown>;
type DeleteManagedAgent = (input: DeleteManagedAgentInput) => Promise<unknown>;

export type ManagedAgentActionResult = {
  cancelled?: boolean;
  noticeMessage?: string;
};

export function isManagedAgentActive(agent: Pick<ManagedAgent, "status">) {
  return agent.status === "running" || agent.status === "deployed";
}

export function getManagedAgentPrimaryActionLabel(
  agent: ManagedAgent,
): string | undefined {
  if (agent.backend.type === "provider") {
    // Provider agents can be deployed but have no native undeploy API, so an
    // already-deployed provider has no stop action. Return undefined so
    // callers omit the primary control rather than rendering a button that
    // can only error.
    return isManagedAgentActive(agent) ? undefined : "Deploy";
  }

  if (isManagedAgentActive(agent)) {
    return "Stop";
  }

  return agent.status === "stopped" ? "Respawn" : "Spawn";
}

export async function startManagedAgentWithRules({
  agent,
  startManagedAgent,
}: {
  agent: ManagedAgent;
  startManagedAgent: StartManagedAgent;
}) {
  // Shared-compute agents delegate availability checks to the backend start
  // preflight, which resolves a live server for the configured model and
  // returns an actionable error when no current member serves it.
  await startManagedAgent(agent.pubkey);
}

export async function respawnManagedAgentWithRules({
  agent,
  startManagedAgent,
  stopManagedAgent,
}: {
  agent: ManagedAgent;
  startManagedAgent: StartManagedAgent;
  stopManagedAgent: StopManagedAgent;
}) {
  if (agent.backend.type === "local" && isManagedAgentActive(agent)) {
    await stopManagedAgent(agent.pubkey);
  }

  await startManagedAgent(agent.pubkey);
}

export async function stopManagedAgentWithRules({
  agent,
  stopManagedAgent,
}: {
  agent: ManagedAgent;
  stopManagedAgent: StopManagedAgent;
}): Promise<ManagedAgentActionResult> {
  // Provider (remote) agents have no native undeploy API yet — the relay
  // !shutdown @mention that formerly idled them is gone with the relay. Do
  // not emulate a stop or clear deployment state (that would be false
  // success); refuse visibly so the unsupported surface is obvious rather
  // than silently succeeding.
  if (agent.backend.type === "provider") {
    throw new Error(
      "Provider agents cannot be stopped in the native workspace; remote undeploy is not yet supported.",
    );
  }

  await stopManagedAgent(agent.pubkey);
  return {};
}

export async function deleteManagedAgentWithRules({
  agent,
  deleteManagedAgent,
  skipRemoteDeleteConfirm = false,
}: {
  agent: ManagedAgent;
  deleteManagedAgent: DeleteManagedAgent;
  skipRemoteDeleteConfirm?: boolean;
}): Promise<ManagedAgentActionResult> {
  const isDeployedRemote =
    agent.backend.type === "provider" && agent.backendAgentId;

  // The relay !shutdown graceful-stop-before-delete path is gone with the
  // relay, and there is no native undeploy. The remote deployment may keep
  // running after the local record is removed, so confirm the orphan risk
  // unless the caller already has, then delete via the registered command
  // with the force-remote-delete guard the backend requires for deployed
  // remote agents.
  if (isDeployedRemote && !skipRemoteDeleteConfirm) {
    const confirmed = window.confirm(
      "This agent has a live remote deployment. Deleting removes the local " +
        "management record; the remote deployment may keep running (native " +
        "undeploy is not yet supported). Continue?",
    );
    if (!confirmed) {
      return { cancelled: true };
    }
  }

  await deleteManagedAgent({
    pubkey: agent.pubkey,
    forceRemoteDelete: isDeployedRemote ? true : undefined,
  });

  return {};
}
