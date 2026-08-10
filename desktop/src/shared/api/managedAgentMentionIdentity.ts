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
        return recordPubkey &&
          AGENT_ID_PATTERN.test(childAgentId) &&
          childAgentId !== recordPubkey
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
 * Remove every managed lifecycle record key from a raw x0xd roster, then add
 * one back as a UI alias only when its distinct, valid child AgentId is also a
 * native member. This prevents stale legacy record keys from impersonating a
 * provisioned child while still letting UI/readiness code use control keys.
 */
export function expandManagedAgentMemberPubkeys(
  memberPubkeys: Iterable<string>,
  managedRecordPubkeys: Iterable<string>,
  nativeIdentityByRecordPubkey: ManagedAgentNativeIdentityMap,
): Set<string> {
  const managedRecords = new Set(
    [...managedRecordPubkeys].map(normalizePubkey),
  );
  const expanded = new Set(
    [...memberPubkeys]
      .map(normalizePubkey)
      .filter((pubkey) => !managedRecords.has(pubkey)),
  );
  for (const [recordPubkey, childAgentId] of Object.entries(
    nativeIdentityByRecordPubkey,
  )) {
    const normalizedRecordPubkey = normalizePubkey(recordPubkey);
    const normalizedChildAgentId = normalizePubkey(childAgentId);
    if (
      managedRecords.has(normalizedRecordPubkey) &&
      AGENT_ID_PATTERN.test(normalizedChildAgentId) &&
      normalizedChildAgentId !== normalizedRecordPubkey &&
      expanded.has(normalizedChildAgentId)
    ) {
      expanded.add(normalizedRecordPubkey);
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
    if (!AGENT_ID_PATTERN.test(childAgentId) || childAgentId === candidate) {
      throw new Error(
        `Managed agent "${managedAgent.name}" has no native identity. Start or restart it before mentioning it.`,
      );
    }
    resolved.push(childAgentId);
  }

  return [...new Set(resolved)];
}
