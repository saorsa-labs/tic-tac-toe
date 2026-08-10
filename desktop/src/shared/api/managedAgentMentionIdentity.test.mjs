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
  wakeManagedAgentsForStructuredMention,
} = await import("./managedAgentMentionIdentity.ts");
const { wakeManagedAgentMentionFromLiveEvent } = await import(
  "../../features/channels/useLiveChannelUpdates.ts"
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

test("a managed child structured mention cold-starts the stopped target by record key", async () => {
  const guideRecord = "1".repeat(64);
  const guideChild = "2".repeat(64);
  const targetRecord = "3".repeat(64);
  const targetChild = "4".repeat(64);
  const starts = [];
  const agents = [
    {
      backend: { type: "local" },
      name: "Guide",
      pubkey: guideRecord,
      status: "running",
    },
    {
      backend: { type: "local" },
      name: "X",
      pubkey: targetRecord,
      status: "stopped",
    },
  ];

  const started = await wakeManagedAgentMentionFromLiveEvent(
    {
      id: "guide-mentions-x",
      kind: 9,
      pubkey: guideChild,
      created_at: 1,
      content: "Please take this, @X",
      sig: "verified",
      tags: [
        ["h", "welcome"],
        ["p", guideChild],
        ["p", targetChild],
      ],
    },
    {
      listManagedAgents: async () => agents,
      getManagedAgentNativeIdentity: async (record) =>
        record === guideRecord ? guideChild : targetChild,
      startManagedAgent: async (record) => {
        starts.push(record);
      },
    },
  );

  assert.deepEqual(started, [targetRecord]);
  assert.deepEqual(starts, [targetRecord]);
  assert.ok(!starts.includes(targetChild));
});

test("a stranger cannot cold-start a managed target with a structured mention", async () => {
  const guideRecord = "1".repeat(64);
  const guideChild = "2".repeat(64);
  const targetRecord = "3".repeat(64);
  const targetChild = "4".repeat(64);
  const starts = [];

  const started = await wakeManagedAgentsForStructuredMention(
    {
      pubkey: "9".repeat(64),
      tags: [["p", targetChild]],
    },
    {
      listManagedAgents: async () => [
        {
          backend: { type: "local" },
          name: "Guide",
          pubkey: guideRecord,
          status: "running",
        },
        {
          backend: { type: "local" },
          name: "X",
          pubkey: targetRecord,
          status: "stopped",
        },
      ],
      getManagedAgentNativeIdentity: async (record) =>
        record === guideRecord ? guideChild : targetChild,
      startManagedAgent: async (record) => {
        starts.push(record);
      },
    },
  );

  assert.deepEqual(started, []);
  assert.deepEqual(starts, []);
});

test("legacy record-key mention and already-running target do not start a child", async () => {
  const guideRecord = "1".repeat(64);
  const guideChild = "2".repeat(64);
  const targetRecord = "3".repeat(64);
  const targetChild = "4".repeat(64);
  const starts = [];
  const dependencies = {
    listManagedAgents: async () => [
      {
        backend: { type: "local" },
        name: "Guide",
        pubkey: guideRecord,
        status: "running",
      },
      {
        backend: { type: "local" },
        name: "X",
        pubkey: targetRecord,
        status: "running",
      },
    ],
    getManagedAgentNativeIdentity: async (record) =>
      record === guideRecord ? guideChild : targetChild,
    startManagedAgent: async (record) => {
      starts.push(record);
    },
  };

  assert.deepEqual(
    await wakeManagedAgentsForStructuredMention(
      { pubkey: guideChild, tags: [["p", targetRecord]] },
      dependencies,
    ),
    [],
  );
  assert.deepEqual(
    await wakeManagedAgentsForStructuredMention(
      { pubkey: guideChild, tags: [["p", targetChild]] },
      dependencies,
    ),
    [],
  );
  assert.deepEqual(starts, []);
});
