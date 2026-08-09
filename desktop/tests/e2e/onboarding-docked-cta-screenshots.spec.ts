import { expect, test } from "@playwright/test";

import { waitForAnimations } from "../helpers/animations";
import { installMockBridge, TEST_IDENTITIES } from "../helpers/bridge";
import { seedActiveIdentity } from "../helpers/onboarding";

const BLANK_TYLER_IDENTITY = {
  ...TEST_IDENTITIES.tyler,
  username: "",
};

const SHOT_DIR = "test-results/onboarding-docked-cta";

test.use({ viewport: { width: 1280, height: 800 } });

/**
 * Only Claude Code and Codex surface in onboarding (see
 * `ONBOARDING_RUNTIME_ORDER`). A `logged_in` Claude counts as "ready", which
 * enables the docked Next CTA so the screenshot flow can advance from the
 * harness setup page into the default-config page without a real install.
 */
function onboardingRuntime(
  id: "claude" | "codex",
  authStatus: { status: "logged_in" | "logged_out" },
) {
  return {
    id,
    label: id === "claude" ? "Claude Code" : "Codex",
    avatar_url: "",
    availability: "available",
    command: id,
    binary_path: `/usr/local/bin/${id}`,
    default_args: [],
    mcp_command: null,
    install_hint: `Install ${id}`,
    install_instructions_url: "https://example.com",
    can_auto_install: true,
    underlying_cli_path: null,
    node_required: false,
    auth_status: authStatus,
    login_hint: `Sign in to ${id}`,
  };
}

test("machine onboarding: landing, setup, config docked CTAs", async ({
  page,
}) => {
  await installMockBridge(
    page,
    {
      acpRuntimesCatalog: [
        onboardingRuntime("claude", { status: "logged_in" }),
        onboardingRuntime("codex", { status: "logged_out" }),
      ],
    },
    { skipCommunitySeed: true, skipOnboardingSeed: true },
  );
  await page.goto("/");

  // This is the first test in the file, so it pays the cold-load cost
  // (fetching + evaluating the full bundle). Boot resolves recovery state +
  // identity before the machine-onboarding gate replaces the splash, so wait
  // for that handshake to settle with headroom for the bundle and for the
  // three screenshots that follow.
  test.setTimeout(60_000);
  const gate = page.getByTestId("machine-onboarding-gate");
  await expect(gate).toBeVisible({ timeout: 30_000 });

  // M2: the daemon owns identity. The landing screen offers a single
  // "Get started" CTA that loads the daemon AgentId — the user-key
  // import / create / backup path is retired. Recovery is fail-closed: with
  // no user-held key to import or back up, the only forward path is the
  // daemon resolving the identity, and no private-key material is exposed.
  await expect(page.getByRole("button", { name: "Get started" })).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Use an existing key" }),
  ).toHaveCount(0);
  await expect(
    page.getByRole("button", { name: "Create a new identity key" }),
  ).toHaveCount(0);
  await expect(page.getByLabel("Private key", { exact: true })).toHaveCount(0);
  await expect(page.getByTestId("nsec-value")).toHaveCount(0);
  await waitForAnimations(page);
  await page.screenshot({ path: `${SHOT_DIR}/01-landing.png` });

  // "Get started" resolves the daemon identity and advances to the harness
  // setup page — no key ever passes through the UI.
  await page.getByRole("button", { name: "Get started" }).click();
  await expect(page.getByTestId("onboarding-page-2")).toBeVisible();
  await expect(
    page.getByRole("heading", { name: "Set up your agent harnesses" }),
  ).toBeVisible();

  // Daemon-owned identity, made concrete: "Get started" advanced by invoking
  // the daemon's `get_identity` — no user-held key was imported or persisted
  // (the fail-closed guarantee: recovery can only ever come from the daemon).
  const commands = await page.evaluate(
    () =>
      (
        window as Window & {
          __BUZZ_E2E_COMMAND_PAYLOADS__?: Array<{ command: string }>;
        }
      ).__BUZZ_E2E_COMMAND_PAYLOADS__ ?? [],
  );
  expect(commands.some((entry) => entry.command === "get_identity")).toBe(true);
  expect(commands.some((entry) => entry.command === "import_identity")).toBe(
    false,
  );
  expect(
    commands.some((entry) => entry.command === "persist_current_identity"),
  ).toBe(false);

  // Docked CTA: Next / Skip / Back portal into the shell's bottom-fixed
  // footer slot, escaping the step slide's transform.
  const setupFooter = page.getByTestId("onboarding-footer-slot");
  await expect(setupFooter).toBeVisible();
  await expect(page.getByTestId("onboarding-setup-next")).toBeVisible();
  await expect(page.getByTestId("onboarding-setup-skip")).toBeVisible();
  await expect(page.getByTestId("onboarding-back")).toBeVisible();
  await waitForAnimations(page);
  await page.screenshot({ path: `${SHOT_DIR}/02-setup.png` });

  // Advancing to the default-config page keeps the docked CTA group.
  await expect(page.getByTestId("onboarding-setup-next")).toBeEnabled();
  await page.getByTestId("onboarding-setup-next").click();
  await expect(page.getByTestId("onboarding-page-config")).toBeVisible();
  await expect(
    page.getByRole("heading", {
      name: "Configure your default model settings",
    }),
  ).toBeVisible();
  await expect(page.getByTestId("onboarding-finish")).toBeVisible();
  await expect(page.getByTestId("onboarding-back")).toBeVisible();
  await waitForAnimations(page);
  await page.screenshot({ path: `${SHOT_DIR}/03-config.png` });
});

test("machine onboarding setup stays usable in a short viewport", async ({
  page,
}) => {
  await page.setViewportSize({ width: 900, height: 620 });
  await installMockBridge(
    page,
    {
      acpRuntimesCatalog: [
        onboardingRuntime("claude", { status: "logged_in" }),
        onboardingRuntime("codex", { status: "logged_out" }),
      ],
    },
    { skipCommunitySeed: true, skipOnboardingSeed: true },
  );
  await page.goto("/");
  await page.getByRole("button", { name: "Get started" }).click();

  const heading = page.getByRole("heading", {
    name: "Set up your agent harnesses",
  });
  const footer = page.getByTestId("onboarding-footer-slot");
  const next = page.getByTestId("onboarding-setup-next");
  await expect(page.getByTestId("onboarding-page-2")).toBeVisible();
  await expect(heading).toBeVisible();
  await expect(footer).toBeVisible();
  await expect(next).toBeEnabled();

  const layout = await page.evaluate(() => {
    const heading = document.querySelector("h1")?.getBoundingClientRect();
    const footer = document
      .querySelector('[data-testid="onboarding-footer-slot"]')
      ?.getBoundingClientRect();
    return {
      clientHeight: document.documentElement.clientHeight,
      clientWidth: document.documentElement.clientWidth,
      scrollHeight: document.documentElement.scrollHeight,
      scrollWidth: document.documentElement.scrollWidth,
      footerBottom: footer?.bottom ?? 0,
      footerTop: footer?.top ?? 0,
      headingTop: heading?.top ?? 0,
    };
  });
  // The docked CTA is fixed to the viewport bottom (not in normal flow) and
  // sits below the heading; tall content scrolls under its scrim rather than
  // colliding with the buttons. No horizontal overflow / scrollbar.
  expect(layout.footerBottom).toBeLessThanOrEqual(layout.clientHeight);
  expect(layout.footerTop).toBeGreaterThan(layout.headingTop);
  expect(layout.scrollHeight).toBeGreaterThanOrEqual(620);
  expect(layout.scrollWidth).toBe(layout.clientWidth);
});

test("relay onboarding: profile and avatar docked CTAs", async ({ page }) => {
  await seedActiveIdentity(page, BLANK_TYLER_IDENTITY);
  await installMockBridge(page, undefined, { skipOnboardingSeed: true });
  await page.goto("/");

  await expect(page.getByTestId("onboarding-page-1")).toBeVisible();
  await page.getByTestId("onboarding-display-name").fill("Ada Lovelace");
  await waitForAnimations(page);
  await page.screenshot({ path: `${SHOT_DIR}/04-profile.png` });

  await page.getByTestId("onboarding-next").click();
  await expect(page.getByTestId("onboarding-page-avatar")).toBeVisible();
  await page
    .getByTestId("onboarding-avatar-url")
    .fill("https://example.com/onboarding-avatar.png");
  await waitForAnimations(page);
  await page.screenshot({ path: `${SHOT_DIR}/05-avatar.png` });
});
