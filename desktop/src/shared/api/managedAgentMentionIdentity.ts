import { getManagedAgentNativeIdentity } from "@/shared/api/observerRelay";
import { listManagedAgents } from "@/shared/api/tauri";
import type { ManagedAgent } from "@/shared/api/types";
import { normalizePubkey } from "@/shared/lib/pubkey";

const AGENT_ID_PATTERN = /^[0-9a-f]{64}$/i;

type ManagedMentionRecord = Pick<ManagedAgent, "name" | "pubkey">;

export type ManagedAgentNativeIdentityMap = Readonly<Record<string, string>>;

export type ManagedMentionIdentityDependencies = {
  listManagedAgents: () => Promise<readonly ManagedMentionRecord[]>;
  getManagedAgentNativeIdentity: (pubkey: string) => Promise<string | null>;
};

const defaultDependencies: ManagedMentionIdentityDependencies = {
  listManagedAgents,
  getManagedAgentNativeIdentity,
};

/**
 * Resolve the native child identities used by x0xd rosters without changing
 * the persisted record keys the desktop uses for lifecycle operations.
 *
 * This is a best-effort directory projection: an individual stopped or
 * unprovisioned agent must not prevent other live managed agents from being
 * matched to their roster entries. The send boundary below remains fail
 * closed for an explicitly selected managed agent.
 */
export async function resolveManagedAgentNativeIdentityMap(
  agents: readonly ManagedMentionRecord[],
  resolveIdentity: (
    pubkey: string,
  ) => Promise<string | null> = getManagedAgentNativeIdentity,
): Promise<Record<string, string>> {
  const entries = await Promise.all(
    agents.map(async (agent) => {
      try {
        const recordPubkey = normalizePubkey(agent.pubkey);
        const childAgentId = normalizePubkey(
          (await resolveIdentity(recordPubkey)) ?? "",
        );
        return recordPubkey && AGENT_ID_PATTERN.test(childAgentId)
          ? ([recordPubkey, childAgentId] as const)
          : null;
      } catch {
        return null;
      }
    }),
  );
  return Object.fromEntries(entries.filter((entry) => entry !== null));
}

/**
 * Add managed record-key aliases for child AgentIds already present in an
 * x0xd roster. The child IDs remain in the set; record keys are added only so
 * UI/readiness code can compare its control identity against native members.
 */
export function expandManagedAgentMemberPubkeys(
  memberPubkeys: Iterable<string>,
  nativeIdentityByRecordPubkey: ManagedAgentNativeIdentityMap,
): Set<string> {
  const expanded = new Set([...memberPubkeys].map(normalizePubkey));
  for (const [recordPubkey, childAgentId] of Object.entries(
    nativeIdentityByRecordPubkey,
  )) {
    if (expanded.has(normalizePubkey(childAgentId))) {
      expanded.add(normalizePubkey(recordPubkey));
    }
  }
  return expanded;
}

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
