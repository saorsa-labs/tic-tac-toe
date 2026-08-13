import assert from "node:assert/strict";
import test from "node:test";

globalThis.window = globalThis.window ?? {};
globalThis.window.__TAURI_INTERNALS__ = {
  invoke: async () => null,
  transformCallback: () => 1,
  unregisterCallback: () => {},
};

const { attachManagedAgentToChannel } = await import("./channelAgents.ts");

const recordPubkey = "a".repeat(64);
const childAgentId = "b".repeat(64);
const groupId = "welcome";

function agent(status = "running") {
  return {
    pubkey: recordPubkey,
    name: "Guide",
    status,
    backend: { type: "local" },
  };
}

test("attach recognizes an existing child AgentId and never adds the record key", async () => {
  const additions = [];
  const result = await attachManagedAgentToChannel(
    groupId,
    { agent: agent() },
    {
      getManagedAgentNativeIdentity: async (pubkey) => {
        assert.equal(pubkey, recordPubkey);
        return childAgentId;
      },
      getChannelMembers: async () => [{ pubkey: childAgentId }],
      startManagedAgent: async () => {
        throw new Error("running agent must not restart");
      },
      x0xAddGroupMember: async (input) => additions.push(input),
    },
  );

  assert.equal(result.memberAgentId, childAgentId);
  assert.equal(result.membershipAdded, false);
  assert.equal(result.started, false);
  assert.deepEqual(additions, []);
});

test("attach starts a stopped agent before adding only its child AgentId", async () => {
  const calls = [];
  const runningAgent = agent("running");
  const result = await attachManagedAgentToChannel(
    groupId,
    { agent: agent("stopped") },
    {
      startManagedAgent: async (pubkey) => {
        calls.push(["start", pubkey]);
        return runningAgent;
      },
      getManagedAgentNativeIdentity: async (pubkey) => {
        calls.push(["identity", pubkey]);
        return childAgentId;
      },
      getChannelMembers: async () => {
        calls.push(["members"]);
        return [];
      },
      x0xAddGroupMember: async (input) => {
        calls.push(["add", input.agentId]);
      },
    },
  );

  assert.equal(result.started, true);
  assert.equal(result.membershipAdded, true);
  assert.equal(result.memberAgentId, childAgentId);
  assert.deepEqual(calls, [
    ["start", recordPubkey],
    ["identity", recordPubkey],
    ["members"],
    ["add", childAgentId],
  ]);
  assert.ok(
    !calls.some((call) => call[0] === "add" && call[1] === recordPubkey),
  );
});

test("attach fails closed when the child identity is unresolved", async () => {
  let readMembers = false;
  const additions = [];
  await assert.rejects(
    attachManagedAgentToChannel(
      groupId,
      { agent: agent("stopped") },
      {
        startManagedAgent: async () => agent("running"),
        getManagedAgentNativeIdentity: async () => null,
        getChannelMembers: async () => {
          readMembers = true;
          return [];
        },
        x0xAddGroupMember: async (input) => additions.push(input),
      },
    ),
    /Guide.*no native identity.*Start or restart/i,
  );

  assert.equal(readMembers, false);
  assert.deepEqual(additions, []);
});

test("attach rejects a record key returned as its own child before roster access", async () => {
  let memberReads = 0;
  let additions = 0;
  await assert.rejects(
    attachManagedAgentToChannel(
      groupId,
      { agent: agent() },
      {
        startManagedAgent: async () => {
          throw new Error("running agent must not restart");
        },
        getManagedAgentNativeIdentity: async () => recordPubkey,
        getChannelMembers: async () => {
          memberReads += 1;
          return [];
        },
        x0xAddGroupMember: async () => {
          additions += 1;
        },
      },
    ),
    /Guide.*no native identity.*Start or restart/i,
  );

  assert.equal(memberReads, 0);
  assert.equal(additions, 0);
});
