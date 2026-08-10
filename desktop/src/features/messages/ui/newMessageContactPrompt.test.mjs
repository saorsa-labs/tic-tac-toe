import assert from "node:assert/strict";
import { test } from "node:test";

globalThis.window = globalThis;
globalThis.__TAURI_INTERNALS__ = {
  invoke: (cmd) => Promise.reject(new Error(`unexpected command: ${cmd}`)),
  transformCallback: () => 0,
};

const {
  getAddedNativeContactStatus,
  getNewMessageContactPrompt,
  shouldPreferNewMessageContactPrompt,
} = await import("./newMessageContactPrompt.ts");
const { isEstablishedNativeContact } = await import(
  "../../profile/nativeSocialApi.ts"
);

test("an exact x0x Agent ID renders an add action instead of No matching users", () => {
  const prompt = getNewMessageContactPrompt("ab".repeat(32));
  assert.equal(prompt.kind, "action");
  assert.equal(prompt.actionLabel, "Add contact by Agent ID");
  assert.notEqual(prompt.description, "No matching users.");
});

test("four identity words explain why a signed card or full ID is required", () => {
  const prompt = getNewMessageContactPrompt("amber coast moon tree");
  assert.equal(prompt.kind, "explanation");
  assert.match(prompt.message, /not unique addresses/);
});

test("unknown and blocked exact IDs keep Add Contact precedence", () => {
  const unknownId = "12".repeat(32);
  const blockedId = "23".repeat(32);
  const knownId = "34".repeat(32);
  const trustedId = "45".repeat(32);
  const contacts = [
    {
      agentId: unknownId,
      trustLevel: "unknown",
      label: "amber coast moon tree",
      addedAt: 1,
      lastSeen: null,
    },
    {
      agentId: blockedId,
      trustLevel: "blocked",
      label: "blocked peer",
      addedAt: 1,
      lastSeen: null,
    },
    {
      agentId: knownId,
      trustLevel: "known",
      label: "Known laptop",
      addedAt: 1,
      lastSeen: null,
    },
    {
      agentId: trustedId,
      trustLevel: "trusted",
      label: "Trusted laptop",
      addedAt: 1,
      lastSeen: null,
    },
  ];
  const selectableAgentIds = contacts
    .filter(isEstablishedNativeContact)
    .map((contact) => contact.agentId);

  for (const agentId of [unknownId, blockedId]) {
    const prompt = getNewMessageContactPrompt(agentId, contacts);
    assert.equal(prompt.kind, "action");
    assert.equal(
      shouldPreferNewMessageContactPrompt(prompt, selectableAgentIds),
      true,
    );
  }
  for (const agentId of [knownId, trustedId]) {
    const prompt = getNewMessageContactPrompt(agentId, contacts);
    assert.equal(
      shouldPreferNewMessageContactPrompt(prompt, selectableAgentIds),
      false,
    );
  }
});

test("a unique four-word unknown discovery becomes an explicit full-ID add", () => {
  const agentId = "56".repeat(32);
  const prompt = getNewMessageContactPrompt("amber coast moon tree", [
    {
      agentId,
      trustLevel: "unknown",
      label: "amber-coast-moon-tree",
      addedAt: 1,
      lastSeen: null,
    },
  ]);

  assert.deepEqual(prompt, {
    kind: "action",
    actionLabel: "Add discovered contact",
    description:
      "One discovered x0x contact matches these words. Confirm it to save the full Agent ID as a known contact.",
    input: { kind: "agentId", agentId },
  });
  assert.equal(shouldPreferNewMessageContactPrompt(prompt, []), true);
});

test("an unreachable contact still reports a visible persistence success", () => {
  assert.equal(
    getAddedNativeContactStatus({
      agentId: "ab".repeat(32),
      displayName: null,
      connectionOutcome: "Unreachable",
      connectionError: null,
    }),
    "Contact saved. They are offline or not reachable yet; presence will update automatically.",
  );
  assert.equal(
    getAddedNativeContactStatus({
      agentId: "ab".repeat(32),
      displayName: null,
      connectionOutcome: "Direct",
      connectionError: null,
    }),
    "Contact saved and connected.",
  );
});
