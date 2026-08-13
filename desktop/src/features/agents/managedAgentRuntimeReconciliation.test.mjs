import assert from "node:assert/strict";
import test from "node:test";

import { mergeManagedAgentRuntimeStatuses } from "./managedAgentRuntimeHooks.ts";
import {
  canonicalCommunityGroupIds,
  classifyReconcileResult,
  pendingReconcileGroupIds,
  reconcileRetryDelayMs,
} from "./useManagedAgentRuntimeReconciliation.ts";

const runtime = (overrides = {}) => ({
  pubkey: "aa",
  groupId: "group-a",
  localSetup: true,
  lifecycle: "starting",
  pid: 1,
  error: null,
  logPath: null,
  ...overrides,
});

test("community group IDs are trimmed, exact-deduped, and case-sensitive", () => {
  assert.deepEqual(
    canonicalCommunityGroupIds([
      { groupId: " group-b " },
      { groupId: "group-a" },
      { groupId: "group-b" },
      { groupId: "  " },
      { groupId: "GROUP-A" },
    ]),
    ["group-b", "group-a", "GROUP-A"],
  );
});

test("pending selection keeps new groups immediate and failed groups on backoff", () => {
  assert.deepEqual(
    pendingReconcileGroupIds(
      ["done", "busy", "retry-later", "exhausted", "new"],
      new Set(["done"]),
      new Set(["busy"]),
      new Map([["retry-later", 10_000]]),
      new Set(["exhausted"]),
      5_000,
    ),
    ["new"],
  );
  assert.deepEqual(
    pendingReconcileGroupIds(
      ["retry-later", "new"],
      new Set(),
      new Set(),
      new Map([["retry-later", 10_000]]),
      new Set(),
      10_000,
    ),
    ["retry-later", "new"],
  );
});

test("retry delay walks the bounded 5s, 30s, 2m schedule", () => {
  assert.equal(reconcileRetryDelayMs(1), 5_000);
  assert.equal(reconcileRetryDelayMs(2), 30_000);
  assert.equal(reconcileRetryDelayMs(3), 120_000);
  assert.equal(reconcileRetryDelayMs(4), null);
  assert.equal(reconcileRetryDelayMs(0), null);
});

test("only failed or error rows retry, classified by requested group ID", () => {
  const rows = [
    runtime({ requestedGroupId: "group-a", lifecycle: "ready" }),
    runtime({
      groupId: "daemon-canonical-b",
      requestedGroupId: "group-b",
      lifecycle: "failed",
      error: "listener failed",
    }),
    runtime({
      groupId: "daemon-canonical-c",
      requestedGroupId: "group-c",
      lifecycle: "starting",
      error: "startup warning promoted to failure",
    }),
  ];

  assert.deepEqual(
    classifyReconcileResult(["group-a", "group-b", "group-c"], rows),
    { succeeded: ["group-a"], failed: ["group-b", "group-c"] },
  );
  assert.deepEqual(classifyReconcileResult(["group-a"], null), {
    succeeded: [],
    failed: ["group-a"],
  });
});

test("cache merge preserves newer lifecycle rows and newly discovered groups", () => {
  const baselineA = runtime();
  const currentA = runtime({ lifecycle: "ready", pid: 9 });
  const currentB = runtime({
    pubkey: "bb",
    groupId: "group-b",
    lifecycle: "ready",
  });
  const reconciledA = runtime({ requestedGroupId: "group-a", pid: 2 });
  const reconciledC = runtime({
    pubkey: "cc",
    groupId: "group-c",
    requestedGroupId: "group-c",
  });

  assert.deepEqual(
    mergeManagedAgentRuntimeStatuses(
      [baselineA],
      [currentA, currentB],
      [reconciledA, reconciledC],
    ),
    [{ ...reconciledA, ...currentA }, reconciledC, currentB],
  );
});
