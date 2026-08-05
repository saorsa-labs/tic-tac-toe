import { expect, test } from "@playwright/test";

import { installMockBridge } from "../helpers/bridge";

async function commandLog(page: import("@playwright/test").Page) {
  return page.evaluate(
    () =>
      (
        window as Window & {
          __BUZZ_E2E_COMMANDS__?: string[];
        }
      ).__BUZZ_E2E_COMMANDS__ ?? [],
  );
}

test("boots the native workspace without applying a relay transport", async ({
  page,
}) => {
  await installMockBridge(page);
  await page.goto("/");

  await expect(page.getByTestId("open-company-view")).toBeVisible();
  await expect
    .poll(() => commandLog(page))
    .toEqual(
      expect.arrayContaining(["x0x_list_groups", "x0x_set_active_group_id"]),
    );

  const commands = await commandLog(page);
  expect(commands).not.toContain("apply_workspace");
  expect(commands).not.toContain("connect_websocket");
});

test("runs the built-in Company workflow through approval and cancel", async ({
  page,
}) => {
  await installMockBridge(page);
  await page.goto("/");
  await page.getByTestId("open-company-view").click();

  const company = page.getByTestId("company-screen");
  await expect(company).toBeVisible();
  await expect(company).toContainText("Software Dev & Sales");
  await expect(company).toContainText("Symphony online");

  await page.getByRole("button", { name: "Instantiate & run" }).click();
  const run = page.getByTestId("symphony-run-detail");
  await expect(run).toContainText("Waiting for approval");
  await expect(run).toContainText("Approve managed-agent dispatch");

  await run.getByRole("button", { name: "Approve" }).click();
  await expect
    .poll(() => commandLog(page))
    .toEqual(expect.arrayContaining(["symphony_approve"]));

  await run.getByRole("button", { name: "Cancel" }).click();
  await expect(company).toBeVisible();
  await expect
    .poll(() => commandLog(page))
    .toEqual(expect.arrayContaining(["cancel_company_run"]));
});
