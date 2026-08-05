/**
 * Native x0xd mapping for agent teams (former kind 30176).
 *
 * A team is a user-facing grouping of personas. It maps 1:1 to a native
 * **named group** whose roster members are the personas' deployed x0x
 * `agent_id`s — never their relay pubkeys. Team display metadata
 * (instructions) that the group object cannot carry lives in a per-group KV
 * store. The relay kind 30176 coordinate `(pubkey, "d"=team_id)` is replaced
 * by the daemon-assigned `groupId`; cross-device authority is the daemon's
 * authenticated roster frontier (ADR-0001), not a relay signature.
 *
 * Group membership authority comes from x0xd: the UI performs NO roster
 * reconstruction or crypto-state inference.
 */

import type {
  X0xAgentId,
  X0xGroupPolicyPreset,
  X0xNamedGroup,
} from "@/shared/api/tauriNativeX0x";
import type { AgentTeam, CreateTeamInput } from "@/shared/api/types";

/**
 * Policy preset for agent teams. Agent teams are private by default: a roster
 * of the owner's personas, not a public directory. Maps to the x0xd
 * `private_secure` preset (Hidden / InviteOnly / MLS-encrypted).
 */
export const TEAM_GROUP_PRESET: X0xGroupPolicyPreset = "private_secure";

/** Metadata-tag version on team metadata store entries. */
export const TEAM_METADATA_SCHEMA = "buzz.team.v1";

/** Topic for a team's auxiliary metadata store (per group). */
export function teamMetadataStoreTopic(groupId: string): string {
  return `x0x.team.${groupId}`;
}

/** Creation input for the named group that backs a team. */
export function teamToNamedGroupInput(team: CreateTeamInput): {
  name: string;
  description: string;
  preset: X0xGroupPolicyPreset;
} {
  return {
    name: team.name,
    description: team.description ?? "",
    preset: TEAM_GROUP_PRESET,
  };
}

/** Extra metadata the group object cannot carry (instructions, version). */
export type TeamMetadataPayload = {
  schema: typeof TEAM_METADATA_SCHEMA;
  instructions: string | null;
  version: string | null;
  /** Local team id from the relay era, retained only to locate the source dir. */
  legacy_team_id: string;
};

export function teamToMetadataPayload(team: AgentTeam): TeamMetadataPayload {
  return {
    schema: TEAM_METADATA_SCHEMA,
    instructions: team.instructions,
    version: team.version,
    legacy_team_id: team.id,
  };
}

/**
 * Resolve a team's persona roster to native agent ids. Each `personaId` is
 * looked up in `agentIdByPersonaSlug`; personas without a deployed agent link
 * are dropped (they have no authenticated identity to enroll). The returned
 * ids are x0x AgentIds sourced from AgentCards — never relay pubkeys.
 */
export function teamRosterAgentIds(
  personaIds: readonly string[],
  agentIdByPersonaSlug: ReadonlyMap<string, X0xAgentId>,
): X0xAgentId[] {
  const ids: X0xAgentId[] = [];
  for (const slug of personaIds) {
    const agentId = agentIdByPersonaSlug.get(slug);
    if (agentId !== undefined) ids.push(agentId);
  }
  return ids;
}

/**
 * Project a native named group (+ metadata) back into the editor/renderer
 * {@link AgentTeam}. `personaIds` are recovered from the roster by reversing
 * the slug→agentId map (agentId→slug); roster entries with no matching persona
 * are foreign members and excluded from `personaIds`.
 */
export function namedGroupToTeam(
  group: X0xNamedGroup,
  metadata: TeamMetadataPayload | null,
  slugByAgentId: ReadonlyMap<X0xAgentId, string>,
): AgentTeam {
  const personaIds: string[] = [];
  for (const member of group.members) {
    const slug = slugByAgentId.get(member.agentId);
    if (slug !== undefined) personaIds.push(slug);
  }
  const createdAt = new Date(group.createdAtMs).toISOString();
  const updatedAt = new Date(group.updatedAtMs).toISOString();
  return {
    id: metadata?.legacy_team_id ?? group.groupId,
    name: group.name,
    description: group.description || null,
    instructions: metadata?.instructions ?? null,
    personaIds,
    isBuiltin: false,
    // A native group has no backing filesystem directory; sourceDir stays null.
    sourceDir: null,
    isSymlink: false,
    symlinkTarget: null,
    version: metadata?.version ?? null,
    createdAt,
    updatedAt,
  };
}
