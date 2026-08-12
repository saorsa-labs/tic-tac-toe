import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

import {
  WELCOME_TEAM_DESCRIPTION,
  WELCOME_TEAM_IDENTITIES,
  WELCOME_TEAM_ID,
  WELCOME_TEAM_NAME,
} from "./welcomeTeamIdentity.ts";
import {
  migrateLegacyWelcomePersonas,
  migrateLegacyWelcomeTeams,
} from "./welcomeTeamProfileMigration.ts";

function legacyPersona(identity) {
  return {
    id: identity.personaId,
    displayName: identity.legacyDisplayName,
    avatarUrl: `data:image/png;base64,legacy-${identity.personaId}`,
    systemPrompt: identity.legacySystemPrompt,
    runtime: null,
    model: null,
    provider: null,
    namePool: [...identity.legacyNamePool],
    isBuiltIn: true,
    isActive: true,
    sourceTeam: null,
    envVars: {},
    respondTo: null,
    respondToAllowlist: [],
    parallelism: null,
    createdAt: "2026-01-01T00:00:00Z",
    updatedAt: "2026-01-01T00:00:00Z",
  };
}

function applyPersonaUpdate(personas, calls) {
  return async (input) => {
    calls.push(input);
    const current = personas.find(({ id }) => id === input.id);
    assert.ok(current);
    return {
      ...current,
      displayName: input.displayName,
      avatarUrl: input.avatarUrl ?? null,
      systemPrompt: input.systemPrompt,
      runtime: input.runtime ?? null,
      model: input.model ?? null,
      provider: input.provider ?? null,
      namePool: input.namePool ?? [],
    };
  };
}

test("stock persisted Welcome profiles migrate to neutral identities once", async () => {
  const legacy = WELCOME_TEAM_IDENTITIES.map(legacyPersona);
  const firstCalls = [];
  const migrated = await migrateLegacyWelcomePersonas(
    legacy,
    applyPersonaUpdate(legacy, firstCalls),
  );

  assert.equal(firstCalls.length, 3);
  assert.deepEqual(
    migrated.map(({ displayName, avatarUrl, namePool, systemPrompt }) => ({
      displayName,
      avatarUrl,
      namePool,
      systemPrompt,
    })),
    WELCOME_TEAM_IDENTITIES.map((identity) => ({
      displayName: identity.displayName,
      avatarUrl: null,
      namePool: [...identity.namePool],
      systemPrompt: identity.systemPrompt,
    })),
  );

  const secondCalls = [];
  const secondPass = await migrateLegacyWelcomePersonas(
    migrated,
    applyPersonaUpdate(migrated, secondCalls),
  );
  assert.equal(secondCalls.length, 0);
  assert.deepEqual(secondPass, migrated);
});

test("customized built-in profile is not overwritten by the stock migration", async () => {
  const customized = {
    ...legacyPersona(WELCOME_TEAM_IDENTITIES[0]),
    systemPrompt: "My custom instructions",
  };
  const calls = [];

  assert.deepEqual(
    await migrateLegacyWelcomePersonas(
      [customized],
      applyPersonaUpdate([customized], calls),
    ),
    [customized],
  );
  assert.equal(calls.length, 0);
});

test("stock persisted Welcome Team copy migrates once", async () => {
  const legacyTeam = {
    id: WELCOME_TEAM_ID,
    name: "Welcome Team",
    description:
      "A friendly starter trio ready to help you plan, create, and ship.",
    instructions: null,
    personaIds: WELCOME_TEAM_IDENTITIES.map(({ personaId }) => personaId),
    isBuiltin: true,
    sourceDir: null,
    isSymlink: false,
    symlinkTarget: null,
    version: null,
    createdAt: "2026-01-01T00:00:00Z",
    updatedAt: "2026-01-01T00:00:00Z",
  };
  const calls = [];
  const migrated = await migrateLegacyWelcomeTeams(
    [legacyTeam],
    async (input) => {
      calls.push(input);
      return {
        ...legacyTeam,
        name: input.name,
        description: input.description ?? null,
        instructions: input.instructions ?? null,
        personaIds: input.personaIds,
      };
    },
  );

  assert.equal(calls.length, 1);
  assert.equal(migrated[0].name, WELCOME_TEAM_NAME);
  assert.equal(migrated[0].description, WELCOME_TEAM_DESCRIPTION);

  const secondCalls = [];
  assert.deepEqual(
    await migrateLegacyWelcomeTeams(migrated, async (input) => {
      secondCalls.push(input);
      return migrated[0];
    }),
    migrated,
  );
  assert.equal(secondCalls.length, 0);
});

test("persona and team queries apply the upgrade before exposing records", async () => {
  const [agentHooks, teamHooks] = await Promise.all([
    readFile(new URL("../hooks.ts", import.meta.url), "utf8"),
    readFile(new URL("../teamHooks.ts", import.meta.url), "utf8"),
  ]);

  assert.match(
    agentHooks,
    /migrateLegacyWelcomePersonas\(await listPersonas\(\)/,
  );
  assert.match(teamHooks, /migrateLegacyWelcomeTeams\(await listTeams\(\)/);
});
