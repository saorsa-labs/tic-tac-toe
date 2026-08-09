import { expect, test } from "@playwright/test";

import {
  installMockBridge,
  openNewMessagePage,
  TEST_IDENTITIES,
} from "../helpers/bridge";
import { waitForAnimations } from "../helpers/animations";

const SHOTS = "test-results/pubkey-display";

const AGENT_PUBKEY = "cafef00d".repeat(8);

// M2 identity cutover: the bech32 key popover and profile "public key" row are
// gone (the displayed identity is the AgentId + four speakable words; no key
// material is surfaced). What remains worth pinning here is the new-DM
// recipient identity-hover swap (name -> truncated recognition key) and the
// selected-recipient de-duplication contract.

test("new-DM agent name swaps to its public key on name hover", async ({
  page,
}) => {
  // Agent rows only surface when the agent is mentionable, so seed a managed
  // agent alongside its search profile.
  await installMockBridge(page, {
    managedAgents: [
      {
        name: "Pinky",
        pubkey: AGENT_PUBKEY,
        status: "running",
      },
    ],
    searchProfiles: [
      {
        displayName: "Pinky",
        isAgent: true,
        ownerPubkey: "deadbeef".repeat(8),
        pubkey: AGENT_PUBKEY,
      },
    ],
  });
  await page.goto("/");

  await openNewMessagePage(page);
  await expect(page.getByTestId("new-message-page")).toBeVisible();

  const agentResult = page.getByTestId(`new-dm-result-${AGENT_PUBKEY}`);
  await expect(agentResult).toBeVisible();
  await expect(page.getByTestId("new-dm-loading")).toBeHidden();
  await expect
    .poll(async () => {
      const marker = crypto.randomUUID();
      await agentResult.evaluate((element, value) => {
        element.dataset.e2eSettleMarker = value;
      }, marker);
      await page.waitForTimeout(250);
      return agentResult
        .evaluate(
          (element, value) => element.dataset.e2eSettleMarker === value,
          marker,
        )
        .catch(() => false);
    })
    .toBe(true);

  const agentName = agentResult.getByTestId(`new-dm-name-${AGENT_PUBKEY}`);
  const agentKey = agentResult.getByTestId(`new-dm-key-${AGENT_PUBKEY}`);
  await expect(agentResult).toContainText("managed by you");
  await expect(agentName).toContainText("Pinky");
  await expect(agentKey).toHaveCSS("opacity", "0");

  const agentNameBox = await agentName.boundingBox();
  const agentResultBox = await agentResult.boundingBox();
  expect(agentNameBox).not.toBeNull();
  expect(agentResultBox).not.toBeNull();
  if (!agentNameBox || !agentResultBox) return;
  expect(agentNameBox.width).toBeLessThan(agentResultBox.width / 2);
  await page.mouse.move(
    agentResultBox.x + agentResultBox.width - 12,
    agentNameBox.y + agentNameBox.height / 2,
  );
  await expect(agentKey).toHaveCSS("opacity", "0");

  // Acquire fresh locators after the directory queries settle: the result row
  // can be replaced while the initial loading skeleton is transitioning out.
  const settledAgentResult = page.getByTestId(`new-dm-result-${AGENT_PUBKEY}`);
  const settledAgentName = settledAgentResult.getByTestId(
    `new-dm-name-${AGENT_PUBKEY}`,
  );
  const settledAgentKey = settledAgentResult.getByTestId(
    `new-dm-key-${AGENT_PUBKEY}`,
  );
  await settledAgentName.hover();
  await expect
    .poll(async () =>
      settledAgentName.evaluate((element) => element.matches(":hover")),
    )
    .toBe(true);
  await expect(settledAgentKey).not.toHaveCSS("opacity", "0");
  await expect(settledAgentKey).toHaveText("cafef00d…f00d");
  await expect(
    settledAgentName.getByText("Pinky", { exact: true }),
  ).not.toHaveCSS("opacity", "1");
  await expect(settledAgentResult).toContainText("managed by you");
  await waitForAnimations(page);
  await page.getByTestId("new-message-page").screenshot({
    path: `${SHOTS}/new-dm-agent-name-hover.png`,
  });
});

test("selected new-DM recipient is marked Already added on re-search", async ({
  page,
}) => {
  await installMockBridge(page);
  await page.goto("/");

  await openNewMessagePage(page);
  await expect(page.getByTestId("new-message-page")).toBeVisible();

  const search = page.getByTestId("new-dm-search");
  const charliePubkey = TEST_IDENTITIES.charlie.pubkey;
  const charlieResult = page.getByTestId(`new-dm-result-${charliePubkey}`);

  // Select Charlie from the directory.
  await search.fill("charlie");
  await expect(charlieResult).toBeVisible();
  await page.keyboard.press("Enter");

  // Charlie becomes a selected recipient chip and is cleared from the list.
  await expect(
    page.getByTestId(`new-dm-selected-${charliePubkey}`),
  ).toBeVisible();
  await expect(search).toHaveValue("");
  await expect(charlieResult).toHaveCount(0);

  // Searching for Charlie again still surfaces him, but the picker marks him
  // Already added — selected recipients are de-duplicated, never re-added.
  await search.fill("charlie");
  await expect(charlieResult).toBeVisible();
  await expect(charlieResult).toHaveAttribute(
    "aria-label",
    "Already added charlie",
  );
  await page.keyboard.press("Enter");
  await expect(search).toHaveValue("");
  await expect(
    page.locator("button[data-testid^='new-dm-selected-']"),
  ).toHaveCount(1);
});
