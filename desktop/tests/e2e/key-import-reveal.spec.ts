import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";

import { waitForAnimations } from "../helpers/animations";
import { installMockBridge } from "../helpers/bridge";
import { openSettings } from "../helpers/settings";

/**
 * M2 daemon-owned identity: the keypair lives in the OS keyring; the renderer
 * never holds or shows a private key. The only user-facing identity is the x0x
 * AgentId + four speakable words, and on a fail-closed boot the renderer shows
 * nothing but a recovery screen.
 *
 * These specs pin the privacy invariant — no bech32 (npub/nsec) form and no
 * private-key reveal/import/change-key control may exist in the renderer —
 * across the three surfaces a viewer reaches: initial onboarding,
 * post-onboarding Settings → Profile, and the fail-closed recovery screen.
 * They replace the obsolete "key import masks the key with a reveal toggle"
 * coverage, which exercised a UI the M2 flow removed.
 */

const SHOTS = "test-results/screenshots-key-import-reveal";

// Legacy bech32/private-key controls the M2 daemon-owned flow removed. None of
// these test IDs or affordances may reappear in the renderer.
const FORBIDDEN_TEST_IDS = [
  "nostr-import-nsec-input",
  "nostr-import-reveal-toggle",
  "nostr-import-npub-preview",
  "nostr-import-submit",
  "profile-private-key-toggle",
  "profile-private-key-row",
  "nsec-value",
] as const;

const FORBIDDEN_BUTTON_NAMES = [
  "Use an existing key",
  "Create a new identity key",
  "Reveal",
  "Re-import your key instead",
] as const;

/** The renderer must never expose a bech32 (npub1/nsec1) identity string. */
async function expectNoBech32Identity(page: Page) {
  const text = await page.locator("body").innerText();
  expect(
    /\bnpub1/i.test(text) || /\bnsec1/i.test(text),
    "renderer text must not expose a bech32 (npub/nsec) identity",
  ).toBe(false);
}

/** Assert none of the removed private-key controls are attached to the DOM. */
async function expectNoLegacyKeyControls(page: Page) {
  for (const testId of FORBIDDEN_TEST_IDS) {
    await expect(
      page.getByTestId(testId),
      `removed private-key control "${testId}" must not render`,
    ).toHaveCount(0);
  }
  for (const name of FORBIDDEN_BUTTON_NAMES) {
    await expect(
      page.getByRole("button", { name }),
      `removed "${name}" affordance must not render`,
    ).toHaveCount(0);
  }
}

test("initial onboarding offers the daemon identity, never a key import", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await installMockBridge(page, undefined, {
    skipCommunitySeed: true,
    skipOnboardingSeed: true,
  });
  await page.goto("/");

  // Fresh boot lands on the M2 identity gate — a single "Get started" CTA that
  // loads the daemon-owned identity. It replaces the legacy "Create a new
  // identity key" / "Use an existing key" fork.
  const gate = page.getByTestId("machine-onboarding-gate");
  await expect(gate).toBeVisible();
  const cta = page.getByRole("button", { name: "Get started" });
  await expect(cta).toBeVisible();

  // No private-key import/reveal surface exists at the onboarding entry point.
  await expectNoLegacyKeyControls(page);
  await expectNoBech32Identity(page);

  // The CTA advances to daemon-identity setup — never forks to a key-import
  // page — proving the renderer resolves identity from the daemon, not input.
  await cta.click();
  await expect(page.getByTestId("onboarding-page-2")).toBeVisible();
  await expectNoLegacyKeyControls(page);
});

test("post-onboarding Settings → Profile shows the public key, never a private-key reveal", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await installMockBridge(page);
  await page.goto("/");
  await openSettings(page, "profile");

  // The identity card is the post-onboarding identity surface. Expand it.
  const card = page.getByTestId("profile-identity-card");
  await expect(card).toBeVisible();
  await page.getByTestId("profile-identity-toggle").click();
  await expect(page.getByTestId("profile-identity-details")).toBeVisible();

  // The only identity shown is the public key (hex) — copyable, never secret.
  const pubkey = page.getByTestId("profile-pubkey");
  await expect(pubkey).toBeVisible();
  await expect(pubkey).toHaveText(/^[0-9a-f]{16,}$/);
  await expect(page.getByTestId("copy-profile-pubkey")).toBeVisible();

  // No private-key reveal/import row, no nsec value, no Reveal affordance.
  await expect(page.getByTestId("profile-private-key-toggle")).toHaveCount(0);
  await expect(page.getByTestId("profile-private-key-row")).toHaveCount(0);
  await expect(page.getByTestId("nsec-value")).toHaveCount(0);

  // The identity surface itself must not leak a bech32 identity.
  const surfaceText = await card.innerText();
  expect(
    /\bnpub1/i.test(surfaceText) || /\bnsec1/i.test(surfaceText),
    "identity card must not expose a bech32 (npub/nsec) identity",
  ).toBe(false);

  await waitForAnimations(page);
  await page.screenshot({
    path: `${SHOTS}/profile-identity-no-private-key.png`,
  });
});

test("fail-closed recovery renders no identity and no key import", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await installMockBridge(
    page,
    { identityLocked: true },
    { skipOnboardingSeed: true },
  );
  await page.goto("/");

  // A locked keyring routes to the fail-closed recovery screen — no onboarding
  // gate, no identity, no key-import UI.
  const screen = page.getByTestId("keyring-locked");
  await expect(screen).toBeVisible();
  await expect(
    page.getByRole("heading", { name: "Compatibility service unavailable" }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Relaunch Buzz" }),
  ).toBeVisible();

  // No identity is resolved on a fail-closed boot: the daemon owns the key and
  // get_identity must not be invoked while the keyring is unreachable.
  await expect(page.getByTestId("profile-identity-card")).toHaveCount(0);
  const commands = await page.evaluate(
    () =>
      (window as Window & { __BUZZ_E2E_COMMANDS__?: string[] })
        .__BUZZ_E2E_COMMANDS__ ?? [],
  );
  expect(
    commands,
    "get_identity must not be called on a fail-closed (locked) boot",
  ).not.toContain("get_identity");

  // The recovery surface carries no private-key controls and no bech32 text.
  await expectNoLegacyKeyControls(page);
  await expectNoBech32Identity(page);

  await waitForAnimations(page);
  await page.screenshot({ path: `${SHOTS}/recovery-no-key-import.png` });
});
