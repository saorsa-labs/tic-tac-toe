/**
 * Native x0xd keying for agent observer frames + active turns (former kind
 * 24200). Replaces relay-pubkey keying with the agent's authenticated x0x
 * `AgentId` (its signed AgentCard.agent_id from its dedicated loopback child
 * daemon).
 *
 * Relay era: a live subscription filtered `kind 24200` by `#p` = owner pubkey;
 * the derived active-turn store keyed entries by the agent's relay pubkey.
 *
 * Native era: observer turn frames route over the agent's child x0xd daemon as
 * `x0xPublish({ topic, payload })` (content classified daemon-side — there is
 * no relay envelope kind to reuse). The active-turn store keys by the agent's
 * `AgentId`. A relay pubkey is NEVER reinterpreted as an AgentId; the mapping
 * is sourced from the managed-agent record's native identity link.
 */

import type { X0xAgentId } from "@/shared/api/tauriNativeX0x";

/**
 * The pub/sub topic owner-scoped observer turn frames publish to. One topic
 * per owner identity; the agent's child daemon classifies the payload. The
 * desktop subscribes live over the daemon `/ws` surface (backfill-then-live).
 */
export function observerTurnTopic(ownerAgentId: X0xAgentId): string {
  return `x0x.observer.${ownerAgentId}`;
}

/**
 * Canonical active-turn store key. The store is keyed by the agent's x0x
 * AgentId (64-char lowercase hex) — never a relay pubkey. Lowercasing guards
 * against mixed-case hex from upstream parsers; the value is otherwise opaque.
 */
export function activeTurnAgentKey(agentId: X0xAgentId): string {
  return agentId.toLowerCase();
}

/**
 * Resolve the native AgentId for a managed agent's observer/turn state.
 *
 * `agentId` is the agent's signed AgentCard.agent_id (sourced from its
 * dedicated child daemon). It wins unconditionally when present. When the
 * native identity link is not yet populated (pre-cutover record), `null` is
 * returned — callers MUST NOT fall back to the relay pubkey, which is a
 * separate namespace and would silently mis-key turns.
 */
export function resolveTurnAgentId(agent: {
  agentId?: X0xAgentId | null;
}): X0xAgentId | null {
  const id = agent.agentId;
  if (typeof id === "string" && /^[0-9a-f]{64}$/.test(id)) return id;
  return null;
}

/**
 * Build a slug→AgentId index from persona definition payloads. Used to re-key
 * team rosters and active-turn state from persona slugs to native agent ids.
 * Personas without a deployed agent link are omitted.
 */
export function indexAgentIdBySlug(
  payloads: ReadonlyArray<{ slug: string; agent_id: X0xAgentId | null }>,
): Map<string, X0xAgentId> {
  const map = new Map<string, X0xAgentId>();
  for (const p of payloads) {
    if (p.agent_id !== null) map.set(p.slug, p.agent_id);
  }
  return map;
}
