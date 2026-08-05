import assert from "node:assert/strict";
import test from "node:test";

const { nativeGroupToCommunity, requireAgentId } = await import(
  "./nativeCommunityApi.ts"
);

test("native community adapter preserves Buzz shape with daemon group id", () => {
  const community = nativeGroupToCommunity({
    groupId: "group:opaque/7",
    name: "Research",
    description: "",
    memberCount: 3,
  });
  assert.equal(community.id, "group:opaque/7");
  assert.equal(community.groupId, "group:opaque/7");
  assert.equal(community.relayUrl, "x0x://group/group%3Aopaque%2F7");
  assert.equal("token" in community, false);
  assert.equal("pubkey" in community, false);
});

test("AgentId validation fails closed on Nostr and malformed identifiers", () => {
  const agentId = "AB".repeat(32);
  assert.equal(requireAgentId(agentId), agentId.toLowerCase());
  assert.throws(() => requireAgentId(`npub1${"q".repeat(59)}`), /x0x Agent ID/);
  assert.throws(() => requireAgentId("deadbeef"), /x0x Agent ID/);
});
