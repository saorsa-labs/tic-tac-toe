import { expect, test, type Page } from "@playwright/test";

import { installMockBridge } from "../helpers/bridge";

// ──────────────────────────────────────────────────────────────────────────
// M3 native smoke — MOCK-IPC boundary regression gate.
//
// These specs install the in-page mock bridge (`installMockBridge`) and boot
// the production bundle against it. They prove the booted production code
// PATHS do not invoke any relay/Nostr command (URL resolution, signing,
// websocket transport, relay-backed reads/writes). That is a static-path
// regression gate over the real production module graph.
//
// They are NOT proof of real x0xd daemon behaviour: no daemon is booted, and
// the mock bridge only simulates the IPC boundary. Real native no-relay
// behaviour rests on the removed command registrations, the Rust transport
// returning empty/failing without a URL, and the static no-relay gate — not on
// this mock command log. Do not cite mock-only smoke as native transport
// acceptance against a live daemon.
// ──────────────────────────────────────────────────────────────────────────

async function commandLog(page: Page) {
  return page.evaluate(
    () =>
      (
        window as Window & {
          __BUZZ_E2E_COMMANDS__?: string[];
        }
      ).__BUZZ_E2E_COMMANDS__ ?? [],
  );
}

// Every relay/Nostr command that must never appear in a native boot's command
// log. Covers URL resolution, event signing/auth, the native websocket plugin
// transport, and the removed/relay-backed read+write commands.
const RELAY_DENY_COMMANDS = [
  // Relay URL resolution
  "get_relay_ws_url",
  "get_default_relay_url",
  "get_relay_http_url",
  // Nostr event signing / auth
  "sign_event",
  "create_auth_event",
  // Removed relay-backed channel / message / workspace commands
  "get_channels",
  "send_channel_message",
  "get_channel_window",
  "apply_workspace",
  // Relay-backed event fetch (still registered but relay-backed)
  "get_event",
  // Native websocket Nostr transport must never open/send/tear down
  "connect_websocket",
  "plugin:websocket|connect",
  "plugin:websocket|send",
  "plugin:websocket|disconnect",
  "plugin:websocket|disconnect_all",
] as const;

/** Asserts no relay/Nostr command was invoked over the mock IPC boundary. */
async function assertNoRelayCommands(page: Page) {
  const commands = await commandLog(page);
  const hit = RELAY_DENY_COMMANDS.filter((cmd) => commands.includes(cmd));
  expect(
    hit,
    `relay deny-list commands were invoked over the mock IPC boundary: ${hit.join(", ")}`,
  ).toEqual([]);
}

test("boots the native workspace without invoking any relay command", async ({
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

  // Comprehensive production-deny frontier: none of the relay/Nostr commands
  // may appear in the command log. Catches regressions where a production
  // path re-introduces a relay transport call (mock IPC, not a live daemon).
  await assertNoRelayCommands(page);
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

  // The Company/Symphony slice must also stay clear of the relay frontier.
  await assertNoRelayCommands(page);
});
