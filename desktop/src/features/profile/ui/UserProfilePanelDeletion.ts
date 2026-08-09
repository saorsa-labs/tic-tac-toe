import * as React from "react";

import {
  deleteManagedAgentWithRules,
  type ManagedAgentActionResult,
} from "@/features/agents/lib/managedAgentControlActions";
import { x0xRemoveGroupMember } from "@/shared/api/tauriNativeX0x";
import type {
  AgentPersona,
  Channel,
  ManagedAgent,
  RelayAgent,
} from "@/shared/api/types";
import { getRelayAgentChannelIds } from "@/features/profile/ui/UserProfilePanelUtils";

type DeleteManagedAgentRulesContext = Omit<
  Parameters<typeof deleteManagedAgentWithRules>[0],
  "agent"
>;

type DeleteProfileManagedAgentContext = DeleteManagedAgentRulesContext & {
  removeAgentFromAllChannels: (pubkey: string) => Promise<void>;
};

type DeleteProfileManagedAgentsForPersonaContext =
  DeleteProfileManagedAgentContext & {
    managedAgents: readonly ManagedAgent[];
    selectedAgent?: ManagedAgent;
  };

type UseProfileAgentDeletionInput = {
  channels?: readonly Channel[];
  deleteManagedAgent: DeleteManagedAgentRulesContext["deleteManagedAgent"];
  managedAgent?: ManagedAgent;
  managedAgents?: readonly ManagedAgent[];
  relayAgents?: readonly RelayAgent[];
};

export function useProfileAgentDeletion({
  channels,
  deleteManagedAgent,
  managedAgent,
  managedAgents,
  relayAgents,
}: UseProfileAgentDeletionInput) {
  const removeAgentFromAllChannels = React.useCallback(
    async (agentPubkey: string) => {
      const normalizedPubkey = agentPubkey.toLowerCase();
      const channelIds = new Set(
        getRelayAgentChannelIds(relayAgents, agentPubkey),
      );
      for (const channel of channels ?? []) {
        if (
          channel.memberPubkeys.some(
            (memberPubkey) => memberPubkey.toLowerCase() === normalizedPubkey,
          )
        ) {
          channelIds.add(channel.id);
        }
      }
      if (channelIds.size === 0) return;
      await Promise.allSettled(
        [...channelIds].map((channelId) =>
          x0xRemoveGroupMember(channelId, agentPubkey),
        ),
      );
    },
    [channels, relayAgents],
  );

  const deleteManagedAgentRecord = React.useCallback(
    (agentToDelete: ManagedAgent) =>
      deleteProfileManagedAgent(agentToDelete, {
        deleteManagedAgent,
        removeAgentFromAllChannels,
        skipRemoteDeleteConfirm: true,
      }),
    [deleteManagedAgent, removeAgentFromAllChannels],
  );

  const deleteManagedAgentsForPersona = React.useCallback(
    (persona: AgentPersona) =>
      deleteProfileManagedAgentsForPersona(persona, {
        deleteManagedAgent,
        managedAgents: managedAgents ?? [],
        removeAgentFromAllChannels,
        selectedAgent: managedAgent,
      }),
    [
      deleteManagedAgent,
      managedAgent,
      managedAgents,
      removeAgentFromAllChannels,
    ],
  );

  return {
    deleteManagedAgentRecord,
    deleteManagedAgentsForPersona,
    removeAgentFromAllChannels,
  };
}

export async function deleteProfileManagedAgent(
  agent: ManagedAgent,
  context: DeleteProfileManagedAgentContext,
): Promise<ManagedAgentActionResult> {
  const { removeAgentFromAllChannels, ...deleteContext } = context;
  const result = await deleteManagedAgentWithRules({
    agent,
    ...deleteContext,
  });
  if (result.cancelled) return result;

  await removeAgentFromAllChannels(agent.pubkey);
  return result;
}

export async function deleteProfileManagedAgentsForPersona(
  persona: AgentPersona,
  context: DeleteProfileManagedAgentsForPersonaContext,
): Promise<ManagedAgentActionResult> {
  const { managedAgents, selectedAgent, ...deleteContext } = context;
  const agentsByPubkey = new Map<string, ManagedAgent>();

  for (const agent of managedAgents) {
    if (agent.personaId === persona.id) {
      agentsByPubkey.set(agent.pubkey, agent);
    }
  }

  if (selectedAgent?.personaId === persona.id) {
    agentsByPubkey.set(selectedAgent.pubkey, selectedAgent);
  }

  for (const agent of agentsByPubkey.values()) {
    const result = await deleteProfileManagedAgent(agent, deleteContext);
    if (result.cancelled) return result;
  }

  return {};
}
