import assert from "node:assert/strict";
import test from "node:test";

import {
  activateWelcomeTeamPersonasSequentially,
  buildWelcomeStarterCreateInput,
  LEGACY_WELCOME_GUIDE_SYSTEM_PROMPT,
  pickWelcomeGuideAgent,
  pickWelcomeTeamStarterAgent,
  welcomeStarterRuntimeUpdate,
  WELCOME_GUIDE_AGENT_NAME,
  WELCOME_GUIDE_PERSONA_ID,
  WELCOME_TEAM_ID,
  WELCOME_TEAM_STARTERS,
} from "./welcomeGuide.ts";

const PUB_A = "a".repeat(64);
const PUB_B = "b".repeat(64);
const PUB_C = "c".repeat(64);
const RELAY_A = "ws://localhost:3000";

function makeAgent(overrides = {}) {
  return {
    pubkey: PUB_A,
    name: WELCOME_GUIDE_AGENT_NAME,
    personaId: null,
    relayUrl: RELAY_A,
    acpCommand: "buzz-acp",
    agentCommand: "buzz-agent",
    agentCommandOverride: null,
    agentArgs: [],
    mcpCommand: "buzz-dev-mcp",
    turnTimeoutSeconds: 120,
    idleTimeoutSeconds: null,
    maxTurnDurationSeconds: null,
    parallelism: 1,
    systemPrompt: null,
    model: null,
    provider: null,
    envVars: {},
    status: "stopped",
    pid: null,
    createdAt: "2026-06-11T00:00:00.000Z",
    updatedAt: "2026-06-11T00:00:00.000Z",
    lastStartedAt: null,
    lastStoppedAt: null,
    lastExitCode: null,
    lastError: null,
    logPath: "",
    startOnAppLaunch: false,
    backend: { type: "local" },
    backendAgentId: null,
    respondTo: "owner-only",
    respondToAllowlist: [],
    teamId: WELCOME_TEAM_ID,
    ...overrides,
  };
}

test("pickWelcomeGuideAgent reuses a legacy Kit guide", () => {
  const legacyKit = makeAgent({
    name: "Kit",
    pubkey: PUB_A,
    systemPrompt: LEGACY_WELCOME_GUIDE_SYSTEM_PROMPT,
  });

  assert.equal(pickWelcomeGuideAgent([legacyKit]), legacyKit);
});

test("pickWelcomeGuideAgent prefers a running legacy guide over stopped builtin Fizz", () => {
  const stoppedBuiltinFizz = makeAgent({
    pubkey: PUB_A,
    personaId: WELCOME_GUIDE_PERSONA_ID,
    status: "stopped",
  });
  const runningLegacyKit = makeAgent({
    name: "Kit",
    pubkey: PUB_B,
    status: "running",
    systemPrompt: LEGACY_WELCOME_GUIDE_SYSTEM_PROMPT,
  });

  assert.equal(
    pickWelcomeGuideAgent([stoppedBuiltinFizz, runningLegacyKit]),
    runningLegacyKit,
  );
});

test("pickWelcomeGuideAgent ignores non-Kit agents with the legacy prompt", () => {
  const nonKit = makeAgent({
    pubkey: PUB_A,
    name: "Scout",
    systemPrompt: LEGACY_WELCOME_GUIDE_SYSTEM_PROMPT,
  });
  const fizz = makeAgent({
    pubkey: PUB_C,
    personaId: WELCOME_GUIDE_PERSONA_ID,
  });

  assert.equal(pickWelcomeGuideAgent([nonKit, fizz]), fizz);
});

test("starter persona activation is serialized to protect the shared store", async () => {
  const calls = [];
  let activeWrites = 0;

  await activateWelcomeTeamPersonasSequentially(
    ["builtin:fizz", "builtin:honey", "builtin:bumble"],
    async (personaId) => {
      assert.equal(activeWrites, 0, "activation writes must never overlap");
      activeWrites += 1;
      calls.push(personaId);
      await new Promise((resolve) => setTimeout(resolve, 1));
      activeWrites -= 1;
    },
  );

  assert.deepEqual(calls, ["builtin:fizz", "builtin:honey", "builtin:bumble"]);
});

test("all Welcome starters use the onboarding runtime preference", async () => {
  const claude = {
    id: "claude",
    label: "Claude",
    avatarUrl: "https://runtime/claude.png",
    availability: "available",
    command: "claude-code-acp",
    binaryPath: "/bin/claude-code-acp",
    defaultArgs: [],
    mcpCommand: null,
    installHint: "",
    installInstructionsUrl: "",
    canAutoInstall: false,
    underlyingCliPath: "/bin/claude",
  };
  const buzzAgent = {
    ...claude,
    id: "buzz-agent",
    label: "Buzz Agent",
    command: "buzz-agent",
  };

  for (const starter of WELCOME_TEAM_STARTERS) {
    const input = await buildWelcomeStarterCreateInput(
      starter,
      {
        id: starter.personaId,
        displayName: starter.name,
        systemPrompt: `${starter.name} prompt`,
        model: null,
        provider: null,
        runtime: null,
        avatarUrl: null,
        envVars: {},
        isBuiltIn: true,
        isActive: true,
      },
      [buzzAgent, claude],
      "claude",
      RELAY_A,
    );

    assert.equal(input.agentCommand, "claude-code-acp");
    assert.equal(input.harnessOverride, true);
    assert.equal(input.personaId, starter.personaId);
    assert.equal(input.teamId, WELCOME_TEAM_ID);
    assert.equal(input.spawnAfterCreate, false);
    assert.equal(input.startOnAppLaunch, false);
  }
});

test("existing Welcome starter rematerializes runtime-specific fields atomically", () => {
  const existing = makeAgent({
    pubkey: PUB_A,
    personaId: WELCOME_GUIDE_PERSONA_ID,
    agentCommand: "claude-agent-acp",
    agentCommandOverride: "claude-agent-acp",
    agentArgs: ["--old"],
    mcpCommand: "",
    model: "claude-sonnet",
    provider: "anthropic",
  });

  assert.deepEqual(
    welcomeStarterRuntimeUpdate(existing, {
      name: "Fizz",
      agentCommand: "codex-acp",
      agentArgs: ["--new"],
      mcpCommand: "buzz-dev-mcp",
      model: "gpt-5.6-sol",
      provider: null,
    }),
    {
      pubkey: PUB_A,
      agentCommand: "codex-acp",
      harnessOverride: true,
      agentArgs: ["--new"],
      mcpCommand: "buzz-dev-mcp",
      model: "gpt-5.6-sol",
      provider: null,
    },
  );
});

test("existing Welcome starter clears stale model and provider for Claude", () => {
  const existing = makeAgent({
    personaId: WELCOME_GUIDE_PERSONA_ID,
    agentCommand: "codex-acp",
    agentArgs: [],
    model: "gpt-5.6-sol",
    provider: "openai",
  });

  assert.deepEqual(
    welcomeStarterRuntimeUpdate(existing, {
      name: "Fizz",
      agentCommand: "claude-agent-acp",
      agentArgs: [],
      mcpCommand: "",
    }),
    {
      pubkey: PUB_A,
      agentCommand: "claude-agent-acp",
      harnessOverride: true,
      agentArgs: [],
      mcpCommand: "",
      model: null,
      provider: null,
    },
  );
});

test("existing Welcome starter needs no update when runtime already matches", () => {
  const existing = makeAgent({
    personaId: WELCOME_GUIDE_PERSONA_ID,
    agentCommand: "codex-acp",
    agentArgs: ["--same"],
  });

  assert.equal(
    welcomeStarterRuntimeUpdate(existing, {
      name: "Fizz",
      agentCommand: "codex-acp",
      agentArgs: ["--same"],
      mcpCommand: "buzz-dev-mcp",
      model: null,
      provider: null,
    }),
    null,
  );
});

test("welcome team starter definitions and role identities are stable", () => {
  assert.equal(WELCOME_TEAM_ID, "builtin-team:welcome");
  assert.deepEqual(WELCOME_TEAM_STARTERS, [
    { name: "Fizz", personaId: "builtin:fizz", role: "lead" },
    { name: "Honey", personaId: "builtin:honey", role: "teammate" },
    { name: "Bumble", personaId: "builtin:bumble", role: "teammate" },
  ]);
});

test("starter matching ignores user agents with a Welcome persona", () => {
  const honey = WELCOME_TEAM_STARTERS[1];
  const userHoney = makeAgent({
    personaId: honey.personaId,
    teamId: null,
  });

  assert.equal(pickWelcomeTeamStarterAgent([userHoney], honey), null);
});

test("starter matching uses persona identity rather than display name", () => {
  const honey = WELCOME_TEAM_STARTERS[1];
  const renamedHoney = makeAgent({
    name: "Honey the Helper",
    personaId: honey.personaId,
  });
  const nameOnlyHoney = makeAgent({ name: honey.name, pubkey: PUB_B });

  assert.equal(
    pickWelcomeTeamStarterAgent([nameOnlyHoney, renamedHoney], honey),
    renamedHoney,
  );
});

test("starter matching reuses the team persona across groups (group-independent)", () => {
  // Native reuse is group-independent: the same team persona is reused wherever
  // it runs, with no per-community scoping. A running instance wins by status.
  const bumble = WELCOME_TEAM_STARTERS[2];
  const firstInstance = makeAgent({
    personaId: bumble.personaId,
    status: "running",
  });
  const secondInstance = makeAgent({
    personaId: bumble.personaId,
    pubkey: PUB_B,
  });

  assert.equal(
    pickWelcomeTeamStarterAgent([firstInstance, secondInstance], bumble),
    firstInstance,
  );
});

test("starter matching prefers running, then deployed instances", () => {
  const fizz = WELCOME_TEAM_STARTERS[0];
  const stopped = makeAgent({ personaId: fizz.personaId });
  const deployed = makeAgent({
    personaId: fizz.personaId,
    pubkey: PUB_B,
    status: "deployed",
  });
  const running = makeAgent({
    personaId: fizz.personaId,
    pubkey: PUB_C,
    status: "running",
  });

  assert.equal(
    pickWelcomeTeamStarterAgent([stopped, deployed, running], fizz),
    running,
  );
  assert.equal(
    pickWelcomeTeamStarterAgent([stopped, deployed], fizz),
    deployed,
  );
});
