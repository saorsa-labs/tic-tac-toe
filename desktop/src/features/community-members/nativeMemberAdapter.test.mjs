import assert from "node:assert/strict";
import test from "node:test";

const { nativeMemberToRelayShape } = await import("./hooks.ts");

test("native roster adapter uses daemon AgentId and display name", () => {
  const agentId = "ab".repeat(32);
  const member = nativeMemberToRelayShape({
    agentId,
    userId: null,
    role: "admin",
    state: "active",
    displayName: "Ada",
    joinedAtMs: 1_700_000_000_000,
    updatedAtMs: 1_700_000_000_001,
    addedBy: null,
    removedBy: null,
  });
  assert.equal(member.pubkey, agentId);
  assert.equal(member.displayName, "Ada");
  assert.equal(member.role, "admin");
  assert.equal("npub" in member, false);
  assert.equal("relay" in member, false);
});

test("native legacy roster roles render without inventing admin authority", () => {
  const member = nativeMemberToRelayShape({
    agentId: "cd".repeat(32),
    role: "moderator",
    state: "active",
    displayName: null,
    joinedAtMs: 0,
    updatedAtMs: 0,
    addedBy: null,
    removedBy: null,
  });
  assert.equal(member.role, "member");
});
