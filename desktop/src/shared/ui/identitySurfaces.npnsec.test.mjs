/**
 * M3 identity-surface contract: the onboarding/profile/member UI surfaces that
 * render the viewer's identity contain NO bech32 (npub/nsec) form, and the
 * frontend identity seam carries NO Nostr relay signer — the x0x AgentId is
 * the sole identity on both sides of the IPC boundary.
 *
 * This is a focused static contract test (the production-wide npub/nsec gate
 * lives in `scripts/check-nostr-identity-ui.mjs`); here we pin the specific
 * identity-rendering surfaces and assert the identity seam is agentId-only. It
 * fails closed if a surface regresses to a bech32 identity or if the identity
 * seam reintroduces a relay signer (`relayPubkey` / `relay_pubkey`).
 */
import assert from "node:assert/strict";
import fs from "node:fs/promises";
import path from "node:path";
import { describe, it } from "node:test";

const SRC = path.resolve(new URL(".", import.meta.url).pathname, "..", "..");

async function readSrc(rel) {
  return fs.readFile(path.join(SRC, rel), "utf8");
}

// Identity-rendering surfaces the viewer sees during onboarding / profile /
// member flows. None may render a bech32 (npub1/nsec1) user identity.
const IDENTITY_SURFACES = [
  "features/onboarding/ui/KeyringLockedScreen.tsx",
  "features/onboarding/ui/ResetFailedScreen.tsx",
  "features/onboarding/ui/RecoveryScreen.tsx",
  "features/onboarding/ui/CommunityOnboardingFlow.tsx",
  "features/onboarding/ui/OnboardingFlow.tsx",
  "features/profile/ui/UserProfilePanelFields.tsx",
  "features/profile/ui/UserProfilePopover.tsx",
  "features/channels/ui/MembersSidebar.tsx",
  "features/community-members/ui/CommunityMembersCard.tsx",
  "features/community-members/ui/AddMemberDialog.tsx",
  "shared/ui/AgentIdentity.tsx",
];

// The identity seam: the TS Identity type and its IPC mapper expose the x0x
// AgentId ONLY — no Nostr relay signer field survives the M3 cutover.
const IDENTITY_SEAM = ["shared/api/types.ts", "shared/api/tauriIdentity.ts"];

describe("identity surfaces: no bech32 (npub/nsec) user identity", () => {
  for (const rel of IDENTITY_SURFACES) {
    it(`${rel} renders no npub1/nsec1 bech32 identity`, async () => {
      const src = await readSrc(rel);
      assert.ok(
        !src.includes("npub1"),
        `${rel} must not render an npub bech32 identity`,
      );
      assert.ok(
        !src.includes("nsec1"),
        `${rel} must not render an nsec bech32 identity`,
      );
    });
  }
});

describe("identity seam: agentId-only, no Nostr relay signer", () => {
  for (const rel of IDENTITY_SEAM) {
    it(`${rel} carries no relayPubkey/relay_pubkey signer field`, async () => {
      const src = await readSrc(rel);
      assert.ok(
        !src.includes("relayPubkey"),
        `${rel} must not carry a camelCase relayPubkey signer field`,
      );
      assert.ok(
        !src.includes("relay_pubkey"),
        `${rel} must not carry a snake_case relay_pubkey signer field`,
      );
    });
  }
});

describe("e2e mock identity: hex AgentId only, no relay signer, no bech32", () => {
  it("DEFAULT_MOCK_IDENTITY carries a 64-hex agent_id and no relay_pubkey", async () => {
    const src = await readSrc("testing/e2eBridge.ts");
    const blockStart = src.indexOf("DEFAULT_MOCK_IDENTITY");
    const identityBlock = src.slice(blockStart, blockStart + 400);

    // agent_id is an exact 64-hex literal (the sole displayed identity).
    const agentId = identityBlock.match(/agent_id:\s*["']([0-9a-f]{64})["']/);
    assert.ok(agentId, "mock identity must define a 64-hex agent_id");

    // The legacy Nostr relay signer must NOT survive the cutover.
    assert.ok(
      !/relay_pubkey:/.test(identityBlock),
      "mock identity must not carry a relay_pubkey signer field",
    );

    // No bech32 identity leaks into the identity payload constants.
    assert.ok(!identityBlock.includes("npub1"), "no npub in mock identity");
    assert.ok(!identityBlock.includes("nsec1"), "no nsec in mock identity");
  });
});
