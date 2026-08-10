import assert from "node:assert/strict";
import test from "node:test";

globalThis.window = globalThis.window ?? {};
globalThis.window.__TAURI_INTERNALS__ = {
  invoke: async () => null,
  transformCallback: () => 1,
  unregisterCallback: () => {},
};

const {
  expandManagedAgentMemberPubkeys,
  resolveManagedAgentNativeIdentityMap,
  resolveNativeMentionAgentIds,
} = await import("./managedAgentMentionIdentity.ts");

const recordPubkey = "a".repeat(64);
const childAgentId = "b".repeat(64);
const humanAgentId = "c".repeat(64);

test("managed mentions become child AgentIds while human contact mentions are preserved", async () => {
  const lookups = [];
  const mentions = await resolveNativeMentionAgentIds(
    [recordPubkey.toUpperCase(), humanAgentId],
    {
      listManagedAgents: async () => [{ name: "Guide", pubkey: recordPubkey }],
      getManagedAgentNativeIdentity: async (pubkey) => {
        lookups.push(pubkey);
        return childAgentId.toUpperCase();
      },
    },
  );

  assert.deepEqual(mentions, [childAgentId, humanAgentId]);
  assert.deepEqual(lookups, [recordPubkey]);
  assert.ok(!mentions.includes(recordPubkey));
});

test("an unresolved managed mention fails closed with a start-or-restart error", async () => {
  await assert.rejects(
    resolveNativeMentionAgentIds([recordPubkey], {
      listManagedAgents: async () => [{ name: "Guide", pubkey: recordPubkey }],
      getManagedAgentNativeIdentity: async () => null,
    }),
    /Managed agent "Guide" has no native identity.*Start or restart/i,
  );
});

test("translation deduplicates a legacy record key and its child AgentId", async () => {
  const mentions = await resolveNativeMentionAgentIds(
    [recordPubkey, childAgentId],
    {
      listManagedAgents: async () => [{ name: "Guide", pubkey: recordPubkey }],
      getManagedAgentNativeIdentity: async () => childAgentId,
    },
  );

  assert.deepEqual(mentions, [childAgentId]);
});

test("native identity map resolves live children without letting one failure hide the rest", async () => {
  const secondRecord = "d".repeat(64);
  const thirdRecord = "e".repeat(64);
  const identities = await resolveManagedAgentNativeIdentityMap(
    [
      { name: "Guide", pubkey: recordPubkey },
      { name: "Offline", pubkey: secondRecord },
      { name: "Broken", pubkey: thirdRecord },
    ],
    async (pubkey) => {
      if (pubkey === recordPubkey) return childAgentId.toUpperCase();
      if (pubkey === secondRecord) return null;
      throw new Error("lookup unavailable");
    },
  );

  assert.deepEqual(identities, { [recordPubkey]: childAgentId });
});

test("native child roster membership expands to its managed record alias", () => {
  const humanAgentId = "c".repeat(64);
  const expanded = expandManagedAgentMemberPubkeys(
    [childAgentId.toUpperCase(), humanAgentId],
    [recordPubkey],
    { [recordPubkey.toUpperCase()]: childAgentId },
  );

  assert.deepEqual(
    expanded,
    new Set([childAgentId, humanAgentId, recordPubkey]),
  );
});

test("legacy record roster entry cannot stand in for a distinct child", () => {
  const expanded = expandManagedAgentMemberPubkeys(
    [recordPubkey],
    [recordPubkey],
    { [recordPubkey]: childAgentId },
  );

  assert.deepEqual(expanded, new Set());
});

test("unresolved legacy record roster entry does not count as membership", () => {
  const humanAgentId = "c".repeat(64);
  const expanded = expandManagedAgentMemberPubkeys(
    [recordPubkey, humanAgentId],
    [recordPubkey],
    {},
  );

  assert.deepEqual(expanded, new Set([humanAgentId]));
});

test("translation rejects a managed record returned as its own child identity", async () => {
  await assert.rejects(
    resolveNativeMentionAgentIds([recordPubkey], {
      listManagedAgents: async () => [{ name: "Guide", pubkey: recordPubkey }],
      getManagedAgentNativeIdentity: async () => recordPubkey,
    }),
    /Managed agent "Guide" has no native identity.*Start or restart/i,
  );
});
