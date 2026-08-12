import assert from "node:assert/strict";
import { test } from "node:test";

globalThis.window = globalThis;
globalThis.__TAURI_INTERNALS__ = {
  invoke: (cmd) => Promise.reject(new Error(`unexpected command: ${cmd}`)),
  transformCallback: () => 0,
};

const { getAddedNativeContactStatus, getNewMessageContactPrompt } =
  await import("./newMessageContactPrompt.ts");

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
