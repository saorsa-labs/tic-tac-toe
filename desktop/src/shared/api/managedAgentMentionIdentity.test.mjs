import assert from "node:assert/strict";
import test from "node:test";

globalThis.window = globalThis.window ?? {};
globalThis.window.__TAURI_INTERNALS__ = {
  invoke: async () => null,
  transformCallback: () => 1,
  unregisterCallback: () => {},
};

const { resolveNativeMentionAgentIds } = await import(
  "./managedAgentMentionIdentity.ts"
);

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
