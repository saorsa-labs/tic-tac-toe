import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";

import { waitForAnimations } from "../helpers/animations";
import { installMockBridge } from "../helpers/bridge";

/**
 * M2 daemon-owned identity contract for machine onboarding.
 *
 * The displayed identity is the daemon-resolved x0x AgentId + its four
 * speakable words. There is NO client-side key: the landing page advances with
 * a single "Get started" CTA that resolves the identity via `get_identity`,
 * never `get_nsec`. These specs pin the backup-free flow — first-launch
 * advancement, absence of every npub/nsec/backup surface, reload resuming the
 * daemon identity, and fail-closed recovery — replacing the removed
 * fresh-key/backup/nsec assertions.
 */

const SHOTS = "test-results/screenshots-onboarding";

// Every backup / Nostr-key surface was removed with the daemon-owned AgentId
// cutover. These test IDs/text must never reappear; asserted in one place.
const REMOVED_KEY_UI_TEST_IDS = [
  "onboarding-page-backup",
  "nsec-value",
  "nsec-reveal-toggle",
  "backup-load-error",
  "backup-retry",
  "backup-skip",
  "nostr-import-card",
  "nostr-import-nsec-input",
  "nostr-import-npub-preview",
  "nostr-import-submit",
] as const;

type Mock = Parameters<typeof installMockBridge>[1];

/** Boot into a fresh machine onboarding (no community, no completion voucher). */
async function bootFreshOnboarding(page: Page, mock?: Mock) {
  await installMockBridge(page, mock, {
    skipCommunitySeed: true,
    skipOnboardingSeed: true,
  });
  await page.goto("/");
}

async function expectNoKeyUi(page: Page) {
  await expect(page.locator("body")).not.toContainText(/nsec1/i);
  await expect(page.locator("body")).not.toContainText(/npub1/i);
  for (const id of REMOVED_KEY_UI_TEST_IDS) {
    await expect(page.getByTestId(id)).toHaveCount(0);
  }
}

test("first launch advances from the daemon identity landing via Get started", async ({
  page,
}) => {
  await bootFreshOnboarding(page);

  const gate = page.getByTestId("machine-onboarding-gate");
  await expect(gate).toBeVisible();

  // The landing page's sole primary action is the daemon-backed "Get started".
  const getStarted = page.getByRole("button", { name: "Get started" });
  await expect(getStarted).toBeVisible();
  await expect(getStarted).toBeEnabled();
  await waitForAnimations(page);
  await page.screenshot({ path: `${SHOTS}/01-identity-landing.png` });

  await getStarted.click();

  // Advancing resolves the daemon identity and lands directly on the setup
  // step — no backup step is interposed between identity resolution and setup.
  await expect(page.getByTestId("onboarding-page-2")).toBeVisible();
  await waitForAnimations(page);
  await page.screenshot({ path: `${SHOTS}/02-setup-step.png` });

  // The identity comes from the daemon (`get_identity`), never a secret
  // (`get_nsec`) and never a client-side persist (`persist_current_identity`).
  const commands = await page.evaluate(
    () =>
      (
        window as Window & {
          __BUZZ_E2E_COMMAND_PAYLOADS__?: Array<{ command: string }>;
        }
      ).__BUZZ_E2E_COMMAND_PAYLOADS__ ?? [],
  );
  expect(commands.some((entry) => entry.command === "get_identity")).toBe(true);
  expect(commands.some((entry) => entry.command === "get_nsec")).toBe(false);
  expect(
    commands.some((entry) => entry.command === "persist_current_identity"),
  ).toBe(false);
});

test("Get started is the only identity CTA — no create/import/backup affordances", async ({
  page,
}) => {
  await bootFreshOnboarding(page);
  await expect(page.getByTestId("machine-onboarding-gate")).toBeVisible();

  await expect(page.getByRole("button", { name: "Get started" })).toBeVisible();

  // The pre-M2 key-choice landing ("Create a new identity key" /
  // "Use an existing key") and the backup heading are gone.
  await expect(
    page.getByRole("button", { name: "Create a new identity key" }),
  ).toHaveCount(0);
  await expect(
    page.getByRole("button", { name: "Use an existing key" }),
  ).toHaveCount(0);
  await expect(
    page.getByRole("heading", {
      name: "Your unique identity key has been created",
    }),
  ).toHaveCount(0);
  await expectNoKeyUi(page);
});

test("setup back button returns to the daemon identity landing", async ({
  page,
}) => {
  await bootFreshOnboarding(page);
  await page.getByRole("button", { name: "Get started" }).click();
  await expect(page.getByTestId("onboarding-page-2")).toBeVisible();

  await page.getByTestId("onboarding-back").click();

  // Returning to the landing re-shows "Get started", not a key-choice screen.
  await expect(page.getByRole("button", { name: "Get started" })).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Create a new identity key" }),
  ).toHaveCount(0);
});

test("the renderer never surfaces npub, nsec, or backup step test IDs", async ({
  page,
}) => {
  await bootFreshOnboarding(page);
  await page.getByRole("button", { name: "Get started" }).click();
  await expect(page.getByTestId("onboarding-page-2")).toBeVisible();

  // Across the whole landing → setup flow the body must never contain Nostr
  // secret/public key material and none of the removed backup test IDs exist.
  await expectNoKeyUi(page);
});

test("reload mid-onboarding resumes the daemon identity without a client key", async ({
  page,
}) => {
  await bootFreshOnboarding(page);
  await page.getByRole("button", { name: "Get started" }).click();
  await expect(page.getByTestId("onboarding-page-2")).toBeVisible();

  await page.reload();

  // The daemon identity survives the reload: the onboarding gate returns (no
  // recovery screen, no key-import prompt) and the setup flow continues from
  // "Get started" — no client-side key had to survive the reload.
  await expect(page.getByTestId("machine-onboarding-gate")).toBeVisible();
  await expect(page.getByTestId("keyring-locked")).toHaveCount(0);
  await expect(page.getByTestId("relaunch-required")).toHaveCount(0);
  await expectNoKeyUi(page);

  await page.getByRole("button", { name: "Get started" }).click();
  await expect(page.getByTestId("onboarding-page-2")).toBeVisible();
});

test("locked daemon fails closed to the recovery screen with no private-key UI", async ({
  page,
}) => {
  await bootFreshOnboarding(page, { identityLocked: true });

  // A daemon that cannot resolve its identity routes to the fail-closed
  // recovery screen — never the onboarding gate, never a key import/backup.
  const screen = page.getByTestId("keyring-locked");
  await expect(screen).toBeVisible();
  await expect(page.getByTestId("machine-onboarding-gate")).toHaveCount(0);

  await expect(
    screen.getByRole("button", { name: "Relaunch Buzz" }),
  ).toBeVisible();
  await expect(screen).not.toContainText(/nsec1/i);
  await expect(screen).not.toContainText(/npub1/i);
  for (const id of REMOVED_KEY_UI_TEST_IDS) {
    await expect(page.getByTestId(id)).toHaveCount(0);
  }
});
