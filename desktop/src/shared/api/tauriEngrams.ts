// ── NIP-AE agent memory (engram) reads ──────────────────────────────────────

/**
 * One memory entry. `slug` is the canonical engram slug — `core` for the
 * agent's identity profile, `mem/...` for everything else. `body` is the
 * decrypted UTF-8 payload. `outgoingRefs` is the list of `[[slug]]`
 * references parsed from the body; the UI BFSes from `core.outgoingRefs`
 * to compute reachable vs orphan sets.
 */
export type EngramEntry = {
  slug: string;
  body: string;
  eventId: string;
  /** Unix seconds. */
  createdAt: number;
  outgoingRefs: string[];
};

/**
 * Response shape for `get_agent_memory`. `core` is split from `memories`
 * because the UI roots its reachability tree there. `truncated` flags a
 * relay cap hit (>= 5000 events for this (agent, owner) pair). `fetchedAt`
 * (unix seconds) is for "last loaded" copy on the refetch affordance.
 */
export type AgentMemoryListing = {
  core: EngramEntry | null;
  memories: EngramEntry[];
  truncated: boolean;
  fetchedAt: number;
};

/**
 * Private NIP-AE engrams have no confidentiality-preserving native x0xd API.
 * The generic `/stores` surface is readable by every joined replica, so mapping
 * owner-encrypted memory into a KV store would weaken the old access boundary.
 *
 * Fail explicitly until x0xd exposes an encrypted owner-scoped store. This is
 * intentionally NOT an empty listing (which the UI would interpret as “no
 * memories”) and never falls back to the relay-backed Rust command.
 */
export async function getAgentMemory(
  _agentPubkey: string,
): Promise<AgentMemoryListing> {
  throw new Error(
    "Agent memory is unavailable: x0xd does not yet expose an encrypted owner-scoped engram store, and relay fallback is disabled.",
  );
}
