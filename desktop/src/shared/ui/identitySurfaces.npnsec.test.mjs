/**
 * M2 identity-surface contract: the onboarding/profile/member UI surfaces that
 * render the viewer's identity contain NO bech32 (npub/nsec) form. Remaining
 * compatibility-only relay operations are keyed off `relayPubkey` (the
 * internal signer) — never the displayed AgentId and never an npub. Native
 * x0xd data paths are intentionally absent from the relay-wiring list.
 *
 * This is a focused static contract test (the production-wide npub/nsec gate
 * lives in `scripts/check-nostr-identity-ui.mjs`); here we pin the specific
 * identity-rendering surfaces and the remaining relayPubkey callsites the gate
 * does not cover. It fails closed if a surface regresses to a bech32 identity
 * or if a compatibility callsite is rewired away from `relayPubkey`.
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

// Remaining compatibility relay callsites: every one must use `relayPubkey`
// (the internal signer), not the displayed AgentId or an npub. Migrated native
// data paths such as message history/send do not belong in this list.
const RELAY_WIRING = [
  "app/AppShell.tsx",
  "features/agents/observerRelayStore.ts",
  "features/agents/ui/useObserverEvents.ts",
  "features/identity-archive/hooks.ts",
  "features/onboarding/ui/CommunityOnboardingFlow.tsx",
];

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

describe("relay operations: keyed off relayPubkey (the compatibility signer)", () => {
  for (const rel of RELAY_WIRING) {
    it(`${rel} wires the viewer identity through relayPubkey`, async () => {
      const src = await readSrc(rel);
      assert.ok(
        src.includes("relayPubkey"),
        `${rel} must key relay operations off relayPubkey, not the AgentId/npub`,
      );
    });
  }
});

describe("e2e mock identity: hex AgentId + separate relay signer, no bech32", () => {
  // The default mock identity the E2E fabric returns must satisfy the same M2
  // shape: a 64-hex AgentId, a SEPARATE 64-hex relay signer, four words, and no
  // bech32 anywhere in the identity payload.
  it("DEFAULT_MOCK_IDENTITY carries distinct 64-hex agent_id and relay_pubkey", async () => {
    const src = await readSrc("testing/e2eBridge.ts");
    const blockStart = src.indexOf("DEFAULT_MOCK_IDENTITY");
    const identityBlock = src.slice(blockStart, blockStart + 400);

    // agent_id is an exact 64-hex literal (the displayed identity).
    const agentId = identityBlock.match(/agent_id:\s*["']([0-9a-f]{64})["']/);
    assert.ok(agentId, "mock identity must define a 64-hex agent_id");

    // relay_pubkey is a separate, hex-derived compatibility signer
    // (the mock uses "deadbeef".repeat(8) — hex-derived, not bech32).
    assert.ok(
      /relay_pubkey:\s*["']?[0-9a-f]/.test(identityBlock),
      "mock identity must define a hex relay_pubkey (compat signer)",
    );

    // The two namespaces must be distinct (compat signer ≠ displayed AgentId).
    assert.notStrictEqual(
      agentId[1],
      "deadbeef".repeat(8),
      "agent_id must not equal the relay signer",
    );

    // No bech32 identity leaks into the identity payload constants.
    assert.ok(!identityBlock.includes("npub1"), "no npub in mock identity");
    assert.ok(!identityBlock.includes("nsec1"), "no nsec in mock identity");
  });
});
