import { getManagedAgentNativeIdentity } from "@/shared/api/observerRelay";
import { listManagedAgents } from "@/shared/api/tauri";
import type { ManagedAgent } from "@/shared/api/types";
import { normalizePubkey } from "@/shared/lib/pubkey";

const AGENT_ID_PATTERN = /^[0-9a-f]{64}$/i;

type ManagedMentionRecord = Pick<ManagedAgent, "name" | "pubkey">;

export type ManagedMentionIdentityDependencies = {
  listManagedAgents: () => Promise<readonly ManagedMentionRecord[]>;
  getManagedAgentNativeIdentity: (pubkey: string) => Promise<string | null>;
};

const defaultDependencies: ManagedMentionIdentityDependencies = {
  listManagedAgents,
  getManagedAgentNativeIdentity,
};

/**
 * Translate persisted managed-agent record keys to their provisioned x0x
 * AgentIds immediately before a message enters the native transport.
 *
 * Human contact AgentIds are not managed records, so they pass through
 * unchanged. A managed record without a child identity fails closed: sending
 * its legacy record key would produce an envelope that the child cannot see.
 */
export async function resolveNativeMentionAgentIds(
  mentionPubkeys: readonly string[] | null | undefined,
  dependencies: ManagedMentionIdentityDependencies = defaultDependencies,
): Promise<string[]> {
  const candidates = [
    ...new Set(
      (mentionPubkeys ?? [])
        .map(normalizePubkey)
        .filter((pubkey) => pubkey.length > 0),
    ),
  ];
  if (candidates.length === 0) return [];

  const managedAgents = await dependencies.listManagedAgents();
  const managedByPubkey = new Map(
    managedAgents.map((agent) => [normalizePubkey(agent.pubkey), agent]),
  );
  const resolved: string[] = [];

  for (const candidate of candidates) {
    const managedAgent = managedByPubkey.get(candidate);
    if (!managedAgent) {
      resolved.push(candidate);
      continue;
    }

    const childAgentId = normalizePubkey(
      (await dependencies.getManagedAgentNativeIdentity(candidate)) ?? "",
    );
    if (!AGENT_ID_PATTERN.test(childAgentId)) {
      throw new Error(
        `Managed agent "${managedAgent.name}" has no native identity. Start or restart it before mentioning it.`,
      );
    }
    resolved.push(childAgentId);
  }

  return [...new Set(resolved)];
}
