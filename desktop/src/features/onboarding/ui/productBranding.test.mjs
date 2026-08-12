import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { test } from "node:test";

const productSurfaces = [
  new URL("../../../app/App.tsx", import.meta.url),
  new URL("./MachineOnboardingFlow.tsx", import.meta.url),
  new URL("./SetupStep.tsx", import.meta.url),
  new URL("./CommunityOnboardingFlow.tsx", import.meta.url),
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
    /BuzzMark|FlappingBee|LandingBees|buzz-wordmark|Welcome to Buzz|Take me to Buzz/,
  );
});
