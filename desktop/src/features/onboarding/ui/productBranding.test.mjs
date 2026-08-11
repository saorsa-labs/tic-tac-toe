import assert from "node:assert/strict";
import { access, readFile } from "node:fs/promises";
import { test } from "node:test";

const productSurfaces = [
  new URL("../../../app/App.tsx", import.meta.url),
  new URL("../../communities/ui/WelcomeSetup.tsx", import.meta.url),
  new URL("./MachineOnboardingFlow.tsx", import.meta.url),
  new URL("./SetupStep.tsx", import.meta.url),
  new URL("./CommunityOnboardingFlow.tsx", import.meta.url),
  new URL("./StarterTeamPresentation.tsx", import.meta.url),
  new URL("./WelcomeKickoffStage.tsx", import.meta.url),
  new URL("../welcomeGuide.ts", import.meta.url),
  new URL("../welcomeKickoff.ts", import.meta.url),
  new URL("../../channels/ui/WelcomeComposerBanner.tsx", import.meta.url),
];

test("startup and welcome surfaces ship tic-tac-toe branding without the Buzz mascot", async () => {
  const sources = await Promise.all(
    productSurfaces.map((url) => readFile(url, "utf8")),
  );
  const shippedSurface = sources.join("\n");

  assert.match(shippedSurface, /tic-tac-toe/);
  assert.match(shippedSurface, /TicTacToeMark/);
  assert.doesNotMatch(
    shippedSurface,
    /BuzzMark|FlappingBee|LandingBees|buzz-wordmark|Welcome to Buzz|Take me to Buzz|Fizz|Honey|Bumble|\/onboarding\/starter-team\//,
  );
});

test("project tagline emphasizes peaceful coexistence", async () => {
  const readme = await readFile(
    new URL("../../../../../README.md", import.meta.url),
    "utf8",
  );

  assert.match(readme, /The winning move is to live together in peace\./);
  assert.doesNotMatch(readme, /The only winning move is to play together\./);
});

test("first-launch starter team uses neutral X and O marks without character images", async () => {
  const [communityFlow, kickoffStage, presentation] = await Promise.all([
    readFile(new URL("./CommunityOnboardingFlow.tsx", import.meta.url), "utf8"),
    readFile(new URL("./WelcomeKickoffStage.tsx", import.meta.url), "utf8"),
    readFile(new URL("./StarterTeamPresentation.tsx", import.meta.url), "utf8"),
  ]);
  const starterTeamSurface = [communityFlow, kickoffStage, presentation].join(
    "\n",
  );

  assert.match(starterTeamSurface, /STARTER_TEAM_PRESENTATION/);
  assert.match(presentation, /label: "Guide"/);
  assert.match(presentation, /label: "X"/);
  assert.match(presentation, /label: "O"/);
  assert.doesNotMatch(starterTeamSurface, /<img|animated character/);
});

test("create-from-scratch agent form uses a neutral starter name", async () => {
  const agentDefinitionDialog = await readFile(
    new URL("../../agents/ui/AgentDefinitionDialog.tsx", import.meta.url),
    "utf8",
  );

  assert.match(agentDefinitionDialog, /placeholder="Guide"/);
  assert.doesNotMatch(agentDefinitionDialog, /placeholder="Fizz"/);
});

test("agent catalog and bundled help use tic-tac-toe branding while preserving compatibility identifiers", async () => {
  const [
    runtimeCatalog,
    nestRuntime,
    agentWorkspace,
    cliSkill,
    managedNode,
    envVars,
    agentAuth,
  ] = await Promise.all([
    readFile(
      new URL(
        "../../../../src-tauri/src/managed_agents/discovery.rs",
        import.meta.url,
      ),
      "utf8",
    ),
    readFile(
      new URL(
        "../../../../src-tauri/src/managed_agents/nest.rs",
        import.meta.url,
      ),
      "utf8",
    ),
    readFile(
      new URL(
        "../../../../src-tauri/src/managed_agents/nest_agents.md",
        import.meta.url,
      ),
      "utf8",
    ),
    readFile(
      new URL(
        "../../../../src-tauri/src/managed_agents/nest_skill.md",
        import.meta.url,
      ),
      "utf8",
    ),
    readFile(
      new URL(
        "../../../../src-tauri/src/commands/agent_discovery/managed_node.rs",
        import.meta.url,
      ),
      "utf8",
    ),
    readFile(
      new URL(
        "../../../../src-tauri/src/managed_agents/env_vars.rs",
        import.meta.url,
      ),
      "utf8",
    ),
    readFile(
      new URL("../../../../../crates/buzz-agent/src/auth.rs", import.meta.url),
      "utf8",
    ),
  ]);
  const visibleAgentCopy = [
    runtimeCatalog,
    nestRuntime,
    agentWorkspace,
    cliSkill,
    managedNode,
    envVars,
    agentAuth,
  ].join("\n");

  assert.match(runtimeCatalog, /label: "x0x Agent"/);
  assert.match(runtimeCatalog, /cli_install_hint: "Ships with tic-tac-toe\."/);
  assert.match(agentWorkspace, /^# tic-tac-toe Agent Workspace/m);
  assert.match(cliSkill, /^# tic-tac-toe CLI Skill/m);
  assert.match(managedNode, /restart tic-tac-toe/);
  assert.match(envVars, /reserved by tic-tac-toe/);
  assert.match(agentAuth, /tic-tac-toe: signed in/);
  assert.doesNotMatch(
    visibleAgentCopy,
    /"Buzz Agent"|Ships with the Buzz desktop app|# Buzz Nest|Created once by the Buzz desktop app|Add agents in the Buzz desktop app|# Buzz CLI Skill|Buzz CLI for relay operations|current Buzz `\[Context\]`|Buzz hosts real git repos|restart Buzz|Buzz could not|failed to (?:resolve|create) Buzz|Buzz's private Node tools|reserved by Buzz|<h2>Buzz/,
  );

  // These are compatibility contracts, not product copy.
  assert.match(runtimeCatalog, /id: "buzz-agent"/);
  assert.match(runtimeCatalog, /commands: &\["buzz-agent"\]/);
  assert.match(runtimeCatalog, /BUZZ_AGENT_PROVIDER/);
  assert.match(agentWorkspace, /BEGIN BUZZ MANAGED/);
  assert.match(cliSkill, /name: buzz-cli/);
  assert.match(cliSkill, /BUZZ_PRIVATE_KEY/);
  assert.match(cliSkill, /buzz agents draft-create/);
});

test("retired starter character images are not shipped as public assets", async () => {
  const retiredAssets = ["fizz.png", "honey.png", "bumble.png"];

  for (const asset of retiredAssets) {
    await assert.rejects(
      access(
        new URL(
          `../../../../public/onboarding/starter-team/${asset}`,
          import.meta.url,
        ),
      ),
      /ENOENT/,
    );
  }
});
