import { getManagedAgentNativeIdentity } from "@/shared/api/observerRelay";
import { listManagedAgents } from "@/shared/api/tauri";
import { startManagedAgent } from "@/shared/api/tauriManagedAgents";
import type { ManagedAgent } from "@/shared/api/types";
import { normalizePubkey } from "@/shared/lib/pubkey";

const AGENT_ID_PATTERN = /^[0-9a-f]{64}$/i;

type ManagedMentionRecord = Pick<ManagedAgent, "name" | "pubkey">;
type ManagedMentionWakeRecord = Pick<
  ManagedAgent,
  "backend" | "name" | "pubkey" | "status"
>;

export type StructuredManagedMentionEvent = {
  pubkey: string;
  tags: readonly (readonly string[])[];
};

export type ManagedMentionWakeDependencies = {
  listManagedAgents: () => Promise<readonly ManagedMentionWakeRecord[]>;
  getManagedAgentNativeIdentity: (pubkey: string) => Promise<string | null>;
  startManagedAgent: (pubkey: string) => Promise<unknown>;
};

export type ManagedAgentNativeIdentityMap = Readonly<Record<string, string>>;

export type ManagedMentionIdentityDependencies = {
  listManagedAgents: () => Promise<readonly ManagedMentionRecord[]>;
  getManagedAgentNativeIdentity: (pubkey: string) => Promise<string | null>;
};

const defaultDependencies: ManagedMentionIdentityDependencies = {
  listManagedAgents,
  getManagedAgentNativeIdentity,
};

const defaultWakeDependencies: ManagedMentionWakeDependencies = {
  listManagedAgents,
  getManagedAgentNativeIdentity,
  startManagedAgent,
};

// Collapse duplicate live frames and near-simultaneous child replies onto one
// lifecycle start for each managed record. The entry is removed after settle
// so a later mention can retry a transiently failed start.
const managedMentionStartsInFlight = new Map<string, Promise<void>>();

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

/**
 * Wake stopped local agents explicitly mentioned by another managed child.
 *
 * Native message envelopes carry child AgentIds, while lifecycle commands are
 * keyed by durable managed-record pubkeys. This router builds that reverse
 * mapping from the same native identity source as the send boundary. It only
 * acts when the signed message author is also one of this owner's managed
 * children, so an arbitrary community member cannot cold-start local compute.
 * Display names, body text, and legacy record-key mentions are never inferred.
 */
export async function wakeManagedAgentsForStructuredMention(
  event: StructuredManagedMentionEvent,
  dependencies: ManagedMentionWakeDependencies = defaultWakeDependencies,
): Promise<string[]> {
  const authorAgentId = normalizePubkey(event.pubkey);
  const mentionedAgentIds = new Set(
    event.tags
      .filter((tag) => tag[0] === "p")
      .map((tag) => normalizePubkey(tag[1] ?? ""))
      .filter(
        (agentId) =>
          AGENT_ID_PATTERN.test(agentId) && agentId !== authorAgentId,
      ),
  );
  if (!AGENT_ID_PATTERN.test(authorAgentId) || mentionedAgentIds.size === 0) {
    return [];
  }

  const agents = await dependencies.listManagedAgents();
  const nativeIdentityByRecord = await resolveManagedAgentNativeIdentityMap(
    agents,
    dependencies.getManagedAgentNativeIdentity,
  );
  const recordByNativeIdentity = new Map(
    Object.entries(nativeIdentityByRecord).map(
      ([recordPubkey, childAgentId]) => [
        normalizePubkey(childAgentId),
        normalizePubkey(recordPubkey),
      ],
    ),
  );

  // Child-authored collaboration only. Human-authored messages already use
  // the composer pre-send startup path; unknown remote authors must not gain a
  // resource-start capability merely by adding a structured mention.
  const authorRecordPubkey = recordByNativeIdentity.get(authorAgentId);
  if (!authorRecordPubkey) {
    return [];
  }

  const agentsByRecord = new Map(
    agents.map((agent) => [normalizePubkey(agent.pubkey), agent]),
  );
  const targetRecordPubkeys = [
    ...new Set(
      [...mentionedAgentIds]
        .map((agentId) => recordByNativeIdentity.get(agentId))
        .filter(
          (recordPubkey): recordPubkey is string =>
            Boolean(recordPubkey) && recordPubkey !== authorRecordPubkey,
        ),
    ),
  ].filter((recordPubkey) => {
    const agent = agentsByRecord.get(recordPubkey);
    return agent?.backend.type === "local" && agent.status === "stopped";
  });

  await Promise.all(
    targetRecordPubkeys.map(async (recordPubkey) => {
      const existing = managedMentionStartsInFlight.get(recordPubkey);
      if (existing) {
        await existing;
        return;
      }

      const start = dependencies
        .startManagedAgent(recordPubkey)
        .then(() => undefined);
      managedMentionStartsInFlight.set(recordPubkey, start);
      try {
        await start;
      } finally {
        if (managedMentionStartsInFlight.get(recordPubkey) === start) {
          managedMentionStartsInFlight.delete(recordPubkey);
        }
      }
    }),
  );

  return targetRecordPubkeys;
}
