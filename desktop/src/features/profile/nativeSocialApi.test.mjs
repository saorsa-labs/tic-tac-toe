import assert from "node:assert/strict";
import { afterEach, test } from "node:test";

globalThis.window = globalThis;
const calls = [];
const handlers = new Map();
globalThis.__TAURI_INTERNALS__ = {
  invoke: (cmd, args) => {
    calls.push({ cmd, args });
    const handler = handlers.get(cmd);
    return handler
      ? Promise.resolve(handler(args))
      : Promise.reject(new Error(`unmocked command: ${cmd}`));
  },
  transformCallback: () => 0,
};

afterEach(() => {
  calls.length = 0;
  handlers.clear();
});

const {
  addNativeContactFromInput,
  addNativeContact,
  classifyNativeContactInput,
  getNativePresence,
  listNativeAgents,
  setNativePresence,
} = await import("./nativeSocialApi.ts");

test("native first-contact input distinguishes exact IDs, signed cards, and lossy words", () => {
  const agentId = "AB".repeat(32);
  assert.deepEqual(classifyNativeContactInput(agentId), {
    kind: "agentId",
    agentId: agentId.toLowerCase(),
  });
  assert.deepEqual(classifyNativeContactInput("x0x://agent/signed-card"), {
    kind: "agentCard",
    card: "x0x://agent/signed-card",
  });
  assert.deepEqual(classifyNativeContactInput("amber-coast-moon-tree"), {
    kind: "fourWords",
    words: ["amber", "coast", "moon", "tree"],
  });
});

test("signed agent card import persists and connects a reachable contact", async () => {
  const agentId = "12".repeat(32);
  const card = "x0x://agent/signed-card";
  handlers.set("x0x_import_agent_card", () => ({
    agent_id: agentId,
    display_name: "Work Mac",
    trust_level: "known",
    trust_change_ignored: false,
    groups: 0,
    stores: 0,
  }));
  handlers.set("x0x_connect_agent", () => ({ outcome: "Direct" }));

  assert.deepEqual(await addNativeContactFromInput(card), {
    agentId,
    displayName: "Work Mac",
    connectionOutcome: "Direct",
    connectionError: null,
  });
  assert.deepEqual(calls, [
    {
      cmd: "x0x_import_agent_card",
      args: { card, trustLevel: "known" },
    },
    { cmd: "x0x_connect_agent", args: { agentId } },
  ]);
});

test("unreachable exact-ID contact remains persisted and selectable", async () => {
  const agentId = "34".repeat(32);
  handlers.set("x0x_add_contact", () => null);
  handlers.set("x0x_connect_agent", () => ({ outcome: "Unreachable" }));

  assert.deepEqual(await addNativeContactFromInput(agentId), {
    agentId,
    displayName: null,
    connectionOutcome: "Unreachable",
    connectionError: null,
  });
  assert.deepEqual(
    calls.map(({ cmd }) => cmd),
    ["x0x_add_contact", "x0x_connect_agent"],
  );
});

test("four-word prefix cannot be mistaken for a unique contact address", async () => {
  await assert.rejects(
    () => addNativeContactFromInput("amber coast moon tree"),
    /display prefixes, not unique addresses/,
  );
  assert.equal(calls.length, 0);
});

test("active x0x community members are messageable native agents", async () => {
  const groupId = "11".repeat(32);
  const localAgentId = "22".repeat(32);
  const remoteAgentId = "33".repeat(32);
  handlers.set("x0x_get_active_group_id", () => groupId);
  handlers.set("x0x_get_group_members", () => ({
    members: [
      {
        agentId: localAgentId,
        displayName: "Laptop",
        state: "active",
      },
      {
        agentId: remoteAgentId,
        displayName: null,
        state: "active",
      },
      {
        agentId: "44".repeat(32),
        displayName: "Former member",
        state: "removed",
      },
    ],
  }));
  handlers.set("x0x_get_presence", () => ({
    [localAgentId]: "online",
    [remoteAgentId]: "offline",
  }));

  const agents = await listNativeAgents();
  assert.deepEqual(
    agents.map(({ pubkey, respondTo, channelIds }) => ({
      pubkey,
      respondTo,
      channelIds,
    })),
    [
      { pubkey: localAgentId, respondTo: "anyone", channelIds: [groupId] },
      { pubkey: remoteAgentId, respondTo: "anyone", channelIds: [groupId] },
    ],
  );
});

test("contact add sends only a validated AgentId to authenticated x0xd", async () => {
  handlers.set("x0x_add_contact", () => null);
  const agentId = "ab".repeat(32);
  await addNativeContact(agentId, "Ada");
  assert.deepEqual(calls, [
    {
      cmd: "x0x_add_contact",
      args: {
        input: { agentId, trustLevel: "known", label: "Ada" },
      },
    },
  ]);
  assert.equal(
    calls.some(({ cmd }) => cmd.includes("relay")),
    false,
  );
});

test("presence reads daemon state and manual RelayEvent fallback is disabled", async () => {
  const agentId = "cd".repeat(32);
  handlers.set("x0x_get_presence", () => ({ [agentId]: "online" }));
  assert.deepEqual(await getNativePresence([agentId]), { [agentId]: "online" });
  await assert.rejects(() => setNativePresence("away"), /does not expose/);
  assert.deepEqual(
    calls.map(({ cmd }) => cmd),
    ["x0x_get_presence"],
  );
});

test("npub is never decoded into an AgentId", async () => {
  await assert.rejects(
    () => addNativeContact(`npub1${"q".repeat(59)}`),
    /x0x Agent ID/,
  );
  assert.equal(calls.length, 0);
});
