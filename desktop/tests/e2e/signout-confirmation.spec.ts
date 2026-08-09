/**
 * E2E tests for the destructive sign-out confirmation flow.
 *
 * Signing out wipes the identity and all local app data, so the confirm
 * dialog gates "Delete My Data" behind a single typed gate: the user must
 * type the exact phrase "wipe all my data". These specs pin that gate, the
 * sign_out invocation, the cancel/reset behavior, and the backend failure
 * path (error toast + the section recovers so the user can retry).
 */
import { expect, type Page, test } from "@playwright/test";

import { installMockBridge } from "../helpers/bridge";
import { openSettings } from "../helpers/settings";

const CONFIRM_PHRASE = "wipe all my data";

async function openSignOutDialog(page: Page) {
  await openSettings(page, "profile");
  const section = page.getByTestId("settings-signout");
  await section.scrollIntoViewIfNeeded();
  await page.getByTestId("signout-open-dialog").click();
  await expect(page.getByRole("alertdialog")).toBeVisible({ timeout: 5_000 });
}

test("delete button unlocks only after the exact typed phrase", async ({
  page,
}) => {
  await installMockBridge(page);
  await page.goto("/");
  await openSignOutDialog(page);

  const deleteButton = page.getByTestId("signout-confirm");
  const phraseInput = page.getByTestId("signout-confirm-phrase");

  // Locked before any typing.
  await expect(deleteButton).toBeDisabled();

  // A near-miss keeps it locked.
  await phraseInput.fill("wipe my data");
  await expect(deleteButton).toBeDisabled();

  // The exact phrase — case- and whitespace-tolerant — unlocks it.
  await phraseInput.fill(`  ${CONFIRM_PHRASE.toUpperCase()}  `);
  await expect(deleteButton).toBeEnabled();

  // Clearing the input re-locks it.
  await phraseInput.fill("");
  await expect(deleteButton).toBeDisabled();
});

test("confirming the phrase invokes sign_out and holds the wipe in flight", async ({
  page,
}) => {
  await installMockBridge(page);
  await page.goto("/");
  await openSignOutDialog(page);

  await page.getByTestId("signout-confirm-phrase").fill(CONFIRM_PHRASE);
  const deleteButton = page.getByTestId("signout-confirm");
  await expect(deleteButton).toBeEnabled();
  await deleteButton.click();

  // Production keeps the pending state sticky on success until the process
  // exits, so the wipe stays visibly in flight.
  await expect(deleteButton).toBeDisabled();
  await expect(deleteButton).toHaveText(/signing out/i);

  // The backend wipe command fired.
  await expect
    .poll(async () => {
      const commands = await page.evaluate(() => {
        const w = window as unknown as Record<string, unknown>;
        const recorded = w.__BUZZ_E2E_COMMANDS__;
        return Array.isArray(recorded)
          ? recorded.filter((c): c is string => typeof c === "string")
          : [];
      });
      return commands.includes("sign_out");
    })
    .toBe(true);
});

test("cancel resets the gate for the next open", async ({ page }) => {
  await installMockBridge(page);
  await page.goto("/");
  await openSignOutDialog(page);

  await page.getByTestId("signout-confirm-phrase").fill(CONFIRM_PHRASE);
  await page.getByRole("button", { name: "Cancel" }).click();
  await expect(page.getByRole("alertdialog")).not.toBeVisible();

  // Reopen — the phrase must be cleared and the button locked again.
  await page.getByTestId("signout-open-dialog").click();
  await expect(page.getByRole("alertdialog")).toBeVisible();
  await expect(page.getByTestId("signout-confirm-phrase")).toHaveValue("");
  await expect(page.getByTestId("signout-confirm")).toBeDisabled();
});

test("a sign_out failure surfaces an error toast and recovers the section", async ({
  page,
}) => {
  await installMockBridge(page);
  await page.goto("/");

  await openSignOutDialog(page);

  // Force the backend wipe to fail: wrap the Tauri invoke bridge so only
  // sign_out rejects, leaving every other command on the real mock. Done
  // after the dialog opens — boot is complete and the bridge is installed.
  await page.evaluate(() => {
    function isInvokeHolder(
      v: unknown,
    ): v is { invoke: (...args: unknown[]) => unknown } {
      return (
        typeof v === "object" &&
        v !== null &&
        "invoke" in v &&
        typeof v.invoke === "function"
      );
    }
    const w = window as unknown as Record<string, unknown>;
    if (!isInvokeHolder(w.__TAURI_INTERNALS__)) {
      throw new Error("Tauri invoke bridge unavailable");
    }
    const internals = w.__TAURI_INTERNALS__;
    const original = internals.invoke;
    internals.invoke = (...args: unknown[]) => {
      if (args[0] === "sign_out") {
        return Promise.reject(new Error("Keychain wipe refused"));
      }
      return original(...args);
    };
  });
  await page.getByTestId("signout-confirm-phrase").fill(CONFIRM_PHRASE);
  await page.getByTestId("signout-confirm").click();

  // The surfaced contract on failure: an error toast, the dialog closed, and
  // the section button back to its idle label so the user can retry.
  await expect(
    page
      .locator("[data-sonner-toast]")
      .filter({ hasText: "Keychain wipe refused" }),
  ).toBeVisible();
  await expect(page.getByRole("alertdialog")).not.toBeVisible();
  const openButton = page.getByTestId("signout-open-dialog");
  await expect(openButton).toBeEnabled();
  await expect(openButton).toHaveText("Sign Out");
});
