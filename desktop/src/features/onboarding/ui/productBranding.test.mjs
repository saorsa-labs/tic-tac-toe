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
