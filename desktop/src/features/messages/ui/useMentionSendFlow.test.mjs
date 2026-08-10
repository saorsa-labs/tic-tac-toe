import assert from "node:assert/strict";
import test from "node:test";

import { runMentionSendCommitBoundary } from "./useMentionSendFlow.ts";

test("committed send does not restore when post-send cleanup fails", async () => {
  const cleanupError = new Error("audience storage unavailable");
  let restoreCount = 0;
  let draftCleanupCount = 0;
  const reportedFailures = [];

  const result = await runMentionSendCommitBoundary({
    send: async () => {},
    restoreOnSendFailure: () => {
      restoreCount += 1;
    },
    postSendActions: [
      {
        label: "persistent agent audience update",
        run: () => {
          throw cleanupError;
        },
      },
      {
        label: "sent draft cleanup",
        run: () => {
          draftCleanupCount += 1;
        },
      },
    ],
    reportPostSendFailure: (label, error) => {
      reportedFailures.push({ label, error });
    },
  });

  assert.equal(result, "sent");
  assert.equal(restoreCount, 0, "committed content must not be restored");
  assert.equal(
    draftCleanupCount,
    1,
    "later cleanup must still run after an earlier cleanup failure",
  );
  assert.deepEqual(reportedFailures, [
    { label: "persistent agent audience update", error: cleanupError },
  ]);
});

test("definite send failure restores exactly once and skips post-send cleanup", async () => {
  let restoreCount = 0;
  let cleanupCount = 0;
  let reportCount = 0;

  const result = await runMentionSendCommitBoundary({
    send: async () => {
      throw new Error("send rejected");
    },
    restoreOnSendFailure: () => {
      restoreCount += 1;
    },
    postSendActions: [
      {
        label: "sent draft cleanup",
        run: () => {
          cleanupCount += 1;
        },
      },
    ],
    reportPostSendFailure: () => {
      reportCount += 1;
    },
  });

  assert.equal(result, "failed");
  assert.equal(restoreCount, 1);
  assert.equal(cleanupCount, 0);
  assert.equal(reportCount, 0);
});
