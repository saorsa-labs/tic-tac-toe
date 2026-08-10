import assert from "node:assert/strict";
import test from "node:test";

import { withAcknowledgedWelcomeGroup } from "./welcomeGroupActivation.ts";

test("previous group is acknowledged as Welcome before setup can run", async () => {
  const previousGroupId = "previous-group";
  const welcomeGroupId = "welcome-group";
  let activeGroupId = previousGroupId;
  const calls = [];

  const result = await withAcknowledgedWelcomeGroup(
    {
      groupId: welcomeGroupId,
      isCancelled: () => false,
      activate: async (groupId) => {
        calls.push(`activate:${groupId}`);
        activeGroupId = groupId;
      },
      readActiveGroupId: async () => {
        calls.push(`read:${activeGroupId}`);
        return activeGroupId;
      },
    },
    async () => {
      calls.push(`setup:${activeGroupId}`);
      return "ready";
    },
  );

  assert.equal(result, "ready");
  assert.deepEqual(calls, [
    `read:${previousGroupId}`,
    `activate:${welcomeGroupId}`,
    `read:${welcomeGroupId}`,
    `setup:${welcomeGroupId}`,
  ]);
});

test("cancelled Welcome navigation never reaches setup", async () => {
  let cancelled = false;
  let setupCalls = 0;

  const result = await withAcknowledgedWelcomeGroup(
    {
      groupId: "welcome-group",
      isCancelled: () => cancelled,
      activate: async () => {
        cancelled = true;
      },
      readActiveGroupId: async () => "previous-group",
    },
    async () => {
      setupCalls += 1;
    },
  );

  assert.equal(result, undefined);
  assert.equal(setupCalls, 0);
});

test("mismatched activation acknowledgment fails closed", async () => {
  let setupCalls = 0;

  await assert.rejects(
    withAcknowledgedWelcomeGroup(
      {
        groupId: "welcome-group",
        isCancelled: () => false,
        activate: async () => {},
        readActiveGroupId: async () => "previous-group",
      },
      async () => {
        setupCalls += 1;
      },
    ),
    /activation was not acknowledged/,
  );
  assert.equal(setupCalls, 0);
});
