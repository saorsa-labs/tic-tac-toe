import {
  getWelcomeTeamIdentity,
  WELCOME_TEAM_DESCRIPTION,
  WELCOME_TEAM_ID,
  WELCOME_TEAM_IDENTITIES,
  WELCOME_TEAM_NAME,
} from "@/features/agents/lib/welcomeTeamIdentity";
import type {
  AgentPersona,
  AgentTeam,
  UpdatePersonaInput,
  UpdateTeamInput,
} from "@/shared/api/types";

const LEGACY_WELCOME_TEAM_NAME = "Welcome Team";
const LEGACY_WELCOME_TEAM_DESCRIPTION =
  "A friendly starter trio ready to help you plan, create, and ship.";

function stringArraysEqual(left: readonly string[], right: readonly string[]) {
  return (
    left.length === right.length &&
    left.every((value, index) => value === right[index])
  );
}

function legacyWelcomePersonaUpdate(
  persona: AgentPersona,
): UpdatePersonaInput | null {
  const identity = getWelcomeTeamIdentity(persona.id);
  if (
    !identity ||
    !persona.isBuiltIn ||
    persona.displayName !== identity.legacyDisplayName ||
    persona.systemPrompt !== identity.legacySystemPrompt ||
    !stringArraysEqual(persona.namePool, identity.legacyNamePool)
  ) {
    return null;
  }

  return {
    id: persona.id,
    displayName: identity.displayName,
    avatarUrl: undefined,
    systemPrompt: identity.systemPrompt,
    runtime: persona.runtime ?? undefined,
    model: persona.model ?? undefined,
    provider: persona.provider ?? undefined,
    namePool: [...identity.namePool],
  };
}

function legacyWelcomeTeamUpdate(team: AgentTeam): UpdateTeamInput | null {
  if (
    team.id !== WELCOME_TEAM_ID ||
    !team.isBuiltin ||
    team.name !== LEGACY_WELCOME_TEAM_NAME ||
    team.description !== LEGACY_WELCOME_TEAM_DESCRIPTION ||
    team.instructions !== null ||
    !stringArraysEqual(
      team.personaIds,
      WELCOME_TEAM_IDENTITIES.map(({ personaId }) => personaId),
    )
  ) {
    return null;
  }

  return {
    id: team.id,
    name: WELCOME_TEAM_NAME,
    description: WELCOME_TEAM_DESCRIPTION,
    personaIds: [...team.personaIds],
  };
}

export async function migrateLegacyWelcomePersonas(
  personas: readonly AgentPersona[],
  update: (input: UpdatePersonaInput) => Promise<AgentPersona>,
): Promise<AgentPersona[]> {
  const migrated: AgentPersona[] = [];
  for (const persona of personas) {
    const input = legacyWelcomePersonaUpdate(persona);
    migrated.push(input ? await update(input) : persona);
  }
  return migrated;
}

export async function migrateLegacyWelcomeTeams(
  teams: readonly AgentTeam[],
  update: (input: UpdateTeamInput) => Promise<AgentTeam>,
): Promise<AgentTeam[]> {
  const migrated: AgentTeam[] = [];
  for (const team of teams) {
    const input = legacyWelcomeTeamUpdate(team);
    migrated.push(input ? await update(input) : team);
  }
  return migrated;
}
