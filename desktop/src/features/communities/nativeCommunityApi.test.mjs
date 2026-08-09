import assert from "node:assert/strict";
import test from "node:test";

const { nativeGroupToCommunity, requireAgentId } = await import(
  "./nativeCommunityApi.ts"
);

test("native community adapter preserves the daemon group id", () => {
  const community = nativeGroupToCommunity({
    groupId: "group:opaque/7",
    name: "Research",
    description: "",
    memberCount: 3,
  });
  assert.equal(community.id, "group:opaque/7");
  assert.equal(community.groupId, "group:opaque/7");
  assert.deepEqual(Object.keys(community).sort(), [
    "addedAt",
    "groupId",
    "id",
    "name",
  ]);
});

test("AgentId validation fails closed on malformed identifiers", () => {
  const agentId = "AB".repeat(32);
  assert.equal(requireAgentId(agentId), agentId.toLowerCase());
  assert.throws(() => requireAgentId("q".repeat(63)), /x0x Agent ID/);
  assert.throws(() => requireAgentId("deadbeef"), /x0x Agent ID/);
});
