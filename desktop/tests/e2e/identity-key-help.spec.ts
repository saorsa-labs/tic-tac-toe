import { expect, test } from "@playwright/test";

import { installMockBridge } from "../helpers/bridge";

// Removed in M2: the first-run "identity key help" trigger/dialog and its
// `*-seen` persistence flag. The x0x daemon owns the AgentId (64-hex) plus four
// speakable words, so there is no user-facing "identity key" concept to explain
// and no private-key surface to back up or reveal. This spec guards that
// removal: the help surface must not leak back, and no key material may be
// exposed on first launch.

const HELP_SEEN_KEY = "buzz.machine-onboarding.identity-key-help-seen.v1";

test("first launch exposes no identity-key help surface and persists no help flag", async ({
  page,
}) => {
  await installMockBridge(page, undefined, {
    skipCommunitySeed: true,
    skipOnboardingSeed: true,
  });
  await page.goto("/");

  // The M2 first-launch gate renders under the daemon identity…
  await expect(page.getByTestId("machine-onboarding-gate")).toBeVisible();
  await expect(page.getByRole("button", { name: "Get started" })).toBeVisible();

  // …and the removed private-key help surface is absent.
  await expect(page.getByTestId("identity-key-help-trigger")).toHaveCount(0);
  await expect(page.getByTestId("identity-key-help-dialog")).toHaveCount(0);
  await expect(
    page.getByRole("heading", { name: "What’s an identity key?" }),
  ).toHaveCount(0);
  // No nsec/npub import or reveal control is reachable from first launch.
  await expect(page.getByTestId("nostr-import-nsec-input")).toHaveCount(0);
  await expect(page.getByTestId("nsec-value")).toHaveCount(0);

  // The help-seen persistence flag is never written.
  await expect
    .poll(() =>
      page.evaluate((key) => localStorage.getItem(key), HELP_SEEN_KEY),
    )
    .toBeNull();

  // Stability across navigation: a reload must not bring the surface back.
  await page.reload();
  await expect(page.getByTestId("identity-key-help-trigger")).toHaveCount(0);
  await expect(
    page.getByRole("heading", { name: "What’s an identity key?" }),
  ).toHaveCount(0);
});
