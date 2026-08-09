import { expect, test } from "@playwright/test";

import { installMockBridge } from "../helpers/bridge";

// M2 identity model: the x0x daemon owns the AgentId (64-hex) plus four
// speakable words. There is no npub/nsec, no private-key import, no "create a
// new identity key", and no in-app key backup. Recovery is fail-closed:
//   • lost   — the daemon's recovery probe reports the keyring empty after a
//               migration; the app re-runs first-launch onboarding under the
//               daemon's identity. Nothing is persisted or imported from the UI.
//   • locked — the keyring is unreachable this boot; the only recovery action
//               is to unlock externally and relaunch. No in-app import exists.
// These specs assert that contract: failures stay fail-closed, the identity is
// never rotated by the UI (no persist/import), and no private-key surface leaks.

test("normal first launch loads the daemon-owned identity without persisting a key", async ({
  page,
}) => {
  await page.emulateMedia({ colorScheme: "dark" });
  await installMockBridge(page, undefined, {
    skipCommunitySeed: true,
    skipOnboardingSeed: true,
  });
  await page.goto("/");

  const gate = page.getByTestId("machine-onboarding-gate");
  await expect(gate).toBeVisible();
  // Landing is a flat chartreuse field with a subtle dot-grid pattern.
  await expect(gate).toHaveCSS("background-color", "rgb(215, 215, 46)");
  await expect(gate).toHaveCSS("background-image", /radial-gradient/);
  await expect(gate).toHaveCSS("color", "rgb(23, 23, 23)");
  // The single landing CTA loads the daemon identity — it does not mint a key.
  const cta = page.getByRole("button", { name: "Get started" });
  await expect(cta).toHaveCSS("background-color", "rgb(23, 23, 23)");
  await cta.click();

  // Advancing past the landing layers the dot grid over the
  // chartreuse→light-blue gradient (the step-page shell).
  await expect(gate).toHaveCSS(
    "background-image",
    /radial-gradient\(.*\), linear-gradient\(.*rgb\(215, 215, 46\).*rgb\(215, 231, 246\)\)/s,
  );
  await expect(gate).toHaveCSS("color", "rgb(23, 23, 23)");
  // No private-key creation/backup/import surface is exposed on the way in.
  await expect(page.getByTestId("nostr-import-nsec-input")).toHaveCount(0);
  await expect(
    page.getByRole("heading", { name: "Re-import your key" }),
  ).toHaveCount(0);

  const commands = await page.evaluate(
    () =>
      (
        window as Window & {
          __BUZZ_E2E_COMMAND_PAYLOADS__?: Array<{ command: string }>;
        }
      ).__BUZZ_E2E_COMMAND_PAYLOADS__ ?? [],
  );
  // The daemon identity is read (`get_identity`); nothing is written back —
  // M2 never persists or imports a key from the UI.
  expect(commands.some((entry) => entry.command === "get_identity")).toBe(true);
  expect(
    commands.some((entry) => entry.command === "persist_current_identity"),
  ).toBe(false);
  expect(commands.some((entry) => entry.command === "import_identity")).toBe(
    false,
  );
});

test("lost boot opens the onboarding gate, not a key-import or recovery surface", async ({
  page,
}) => {
  await installMockBridge(
    page,
    { identityLost: true },
    { skipOnboardingSeed: true },
  );
  await page.goto("/");

  // Lost boot re-runs first-launch onboarding under the daemon identity. The
  // removed "Re-import your key" / nsec-import / "Start new identity" surface
  // must not appear — recovery is daemon-owned, not user-keyed.
  await expect(page.getByTestId("machine-onboarding-gate")).toBeVisible();
  await expect(
    page.getByRole("heading", { name: "Re-import your key" }),
  ).toHaveCount(0);
  await expect(page.getByTestId("nostr-import-nsec-input")).toHaveCount(0);
  await expect(page.getByTestId("nostr-import-submit")).toHaveCount(0);
  await expect(page.getByTestId("nostr-import-npub-preview")).toHaveCount(0);
  await expect(
    page.getByRole("button", { name: "Start new identity" }),
  ).toHaveCount(0);
  await expect(page.getByTestId("relaunch-required")).toHaveCount(0);
});

test("lost boot stays fail-closed — it never persists or imports a private key", async ({
  page,
}) => {
  await installMockBridge(
    page,
    { identityLost: true },
    { skipOnboardingSeed: true },
  );
  await page.goto("/");

  await expect(page.getByTestId("machine-onboarding-gate")).toBeVisible();
  // Proceeding under the daemon identity must not rotate it: the old lost-mode
  // flow persisted an ephemeral key (`persist_current_identity`) or imported a
  // secret (`import_identity`). M2 does neither — the current AgentId stays
  // stable because the daemon owns it end to end.
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (
            window as Window & {
              __BUZZ_E2E_COMMAND_PAYLOADS__?: Array<{ command: string }>;
            }
          ).__BUZZ_E2E_COMMAND_PAYLOADS__?.some(
            (entry) => entry.command === "persist_current_identity",
          ) ?? false,
      ),
    )
    .toBe(false);
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (
            window as Window & {
              __BUZZ_E2E_COMMAND_PAYLOADS__?: Array<{ command: string }>;
            }
          ).__BUZZ_E2E_COMMAND_PAYLOADS__?.some(
            (entry) => entry.command === "import_identity",
          ) ?? false,
      ),
    )
    .toBe(false);
});

test("locked boot shows the keyring-locked screen without the onboarding gate or key-import UI", async ({
  page,
}) => {
  await installMockBridge(
    page,
    { identityLocked: true },
    { skipOnboardingSeed: true },
  );
  await page.goto("/");

  await expect(page.getByTestId("keyring-locked")).toBeVisible();
  await expect(page.getByTestId("machine-onboarding-gate")).toHaveCount(0);
  await expect(page.getByTestId("onboarding-gate")).toHaveCount(0);
  // No private-key import surface: locked recovery is external (unlock +
  // relaunch), never an in-app key re-entry.
  await expect(
    page.getByRole("heading", { name: "Re-import your key" }),
  ).toHaveCount(0);
  await expect(page.getByTestId("nostr-import-nsec-input")).toHaveCount(0);
  await expect(page.getByTestId("nostr-import-submit")).toHaveCount(0);
});

test("locked recovery screen exposes only relaunch — no key re-import surface", async ({
  page,
}) => {
  await installMockBridge(
    page,
    { identityLocked: true },
    { skipOnboardingSeed: true },
  );
  await page.goto("/");

  await expect(page.getByTestId("keyring-locked")).toBeVisible();
  // The removed "Re-import your key instead" path is gone; relaunch is the sole
  // recovery action, so a locked keyring cannot be bypassed from the UI.
  await expect(
    page.getByRole("button", { name: "Re-import your key instead" }),
  ).toHaveCount(0);
  await expect(page.getByTestId("nostr-import-nsec-input")).toHaveCount(0);
  await expect(
    page.getByRole("button", { name: "Relaunch Buzz" }),
  ).toBeVisible();
});

test("locked screen relaunch button records the process-restart invoke", async ({
  page,
}) => {
  await installMockBridge(
    page,
    { identityLocked: true },
    { skipOnboardingSeed: true },
  );
  await page.goto("/");

  await expect(page.getByTestId("keyring-locked")).toBeVisible();
  await page.getByTestId("relaunch-app").click();

  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (
            window as Window & {
              __BUZZ_E2E_COMMAND_PAYLOADS__?: Array<{ command: string }>;
            }
          ).__BUZZ_E2E_COMMAND_PAYLOADS__?.some(
            (entry) => entry.command === "plugin:process|restart",
          ) ?? false,
      ),
    )
    .toBe(true);
});
