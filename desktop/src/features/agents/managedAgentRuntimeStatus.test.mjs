import assert from "node:assert/strict";
import test from "node:test";

import {
  agentCommunityAvailability,
  agentCommunityStatusDetail,
  findManagedAgentRuntime,
  managedAgentRuntimeKey,
} from "./managedAgentRuntimeStatus.ts";

const runtime = (overrides = {}) => ({
  pubkey: "aa",
  groupId: "group.example",
  localSetup: true,
  lifecycle: "ready",
  pid: 1,
  error: null,
  logPath: null,
  ...overrides,
});

test("projects every backend lifecycle to the four product labels", () => {
  assert.equal(agentCommunityAvailability(runtime()), "Here");
  for (const lifecycle of ["starting", "listening", "waking"]) {
    assert.equal(agentCommunityAvailability(runtime({ lifecycle })), "Waking");
  }
  for (const lifecycle of ["failed", "stopped"]) {
    assert.equal(
      agentCommunityAvailability(runtime({ lifecycle })),
      "Unavailable",
    );
  }
});

test("backend-authoritative local setup takes precedence", () => {
  assert.equal(
    agentCommunityAvailability(
      runtime({ localSetup: false, lifecycle: "ready" }),
    ),
    "Needs setup on this device",
  );
});

test("unavailable detail distinguishes stopped and failed", () => {
  assert.equal(
    agentCommunityStatusDetail(runtime({ lifecycle: "stopped" })),
    "Stopped by you",
  );
  assert.equal(
    agentCommunityStatusDetail(
      runtime({ lifecycle: "failed", error: "Relay timed out" }),
    ),
    "Relay timed out",
  );
});

test("pair key cannot collide at component boundaries", () => {
  assert.notEqual(
    managedAgentRuntimeKey(runtime({ pubkey: "ab", groupId: "c" })),
    managedAgentRuntimeKey(runtime({ pubkey: "a", groupId: "bc" })),
  );
});

test("selects one group without collapsing same-pubkey pairs", () => {
  const runtimes = [
    runtime({ groupId: "group-a", lifecycle: "ready" }),
    runtime({ groupId: "group-b", lifecycle: "failed" }),
  ];
  assert.equal(
    findManagedAgentRuntime(runtimes, "AA", "group-b")?.lifecycle,
    "failed",
  );
  assert.equal(findManagedAgentRuntime(runtimes, "aa", "group-c"), undefined);
});

test("matches a requested group id against reconciled runtime rows", () => {
  // On startup reconcile a runtime row carries the exact submitted group id
  // (requestedGroupId) alongside the daemon-resolved canonical groupId.
  const runtimes = [
    runtime({
      groupId: "group-canonical",
      requestedGroupId: "group-submitted",
      lifecycle: "ready",
    }),
  ];
  assert.equal(
    findManagedAgentRuntime(runtimes, "aa", "group-submitted")?.lifecycle,
    "ready",
  );
  assert.equal(
    findManagedAgentRuntime(runtimes, "aa", "group-canonical")?.lifecycle,
    "ready",
  );
  assert.equal(
    findManagedAgentRuntime(runtimes, "aa", "group-other"),
    undefined,
  );
});
