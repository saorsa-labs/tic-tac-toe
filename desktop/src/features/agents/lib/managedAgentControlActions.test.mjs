import assert from "node:assert/strict";
import test from "node:test";

import {
  getManagedAgentPrimaryActionLabel,
  startManagedAgentWithRules,
  stopManagedAgentWithRules,
} from "./managedAgentControlActions.ts";

function agent(overrides = {}) {
  return {
    pubkey: "deadbeef".repeat(8),
    name: "Mesh Agent",
    personaId: null,
    acpCommand: "buzz-acp",
    agentCommand: "goose",
    agentArgs: [],
    mcpCommand: "",
    turnTimeoutSeconds: 320,
    idleTimeoutSeconds: null,
    maxTurnDurationSeconds: null,
    parallelism: 1,
    systemPrompt: null,
    model: "hf://demo/model.gguf",
    envVars: {},
    status: "stopped",
    pid: null,
    createdAt: new Date(0).toISOString(),
    updatedAt: new Date(0).toISOString(),
    lastStartedAt: null,
    lastStoppedAt: null,
    lastExitCode: null,
    lastError: null,
    logPath: null,
    startOnAppLaunch: false,
    backend: { type: "local" },
    backendAgentId: null,
    respondTo: "owner-only",
    respondToAllowlist: [],
    ...overrides,
  };
}

test("shared-compute agents delegate start to the backend preflight", async () => {
  const meshAgent = agent({
    envVars: {
      BUZZ_AGENT_PROVIDER: "openai",
      OPENAI_COMPAT_BASE_URL: "http://127.0.0.1:9337/v1/",
    },
  });

  let calledWith = null;
  await startManagedAgentWithRules({
    agent: meshAgent,
    startManagedAgent: async (pubkey) => {
      calledWith = pubkey;
    },
  });
  assert.equal(calledWith, meshAgent.pubkey);

  // Backend preflight failures (e.g. no live serve target) propagate as-is.
  await assert.rejects(
    startManagedAgentWithRules({
      agent: meshAgent,
      startManagedAgent: async () => {
        throw new Error("no live serve target is available for this model");
      },
    }),
    /no live serve target/,
  );
});

test("ordinary local agents still start normally", async () => {
  let calledWith = null;
  await startManagedAgentWithRules({
    agent: agent(),
    startManagedAgent: async (pubkey) => {
      calledWith = pubkey;
    },
  });
  assert.equal(calledWith, "deadbeef".repeat(8));
});

// ── Primary action label: provider agents expose no stop action ─────────────
// A provider (remote) agent has no native undeploy API. An already-active one
// must surface NO primary action (undefined) so the UI omits the control
// instead of rendering a button that can only error.

test("an active provider agent exposes no primary action (no stop button)", () => {
  for (const status of ["running", "deployed"]) {
    assert.equal(
      getManagedAgentPrimaryActionLabel(
        agent({
          status,
          backend: { type: "provider", id: "prov-1", config: {} },
        }),
      ),
      undefined,
      `provider agent with status ${status} must have no primary action`,
    );
  }
});

test("an inactive provider agent offers Deploy, never Stop/Respawn", () => {
  assert.equal(
    getManagedAgentPrimaryActionLabel(
      agent({
        status: "stopped",
        backend: { type: "provider", id: "prov-1", config: {} },
      }),
    ),
    "Deploy",
  );
});

test("an active local agent offers Stop; a stopped local agent offers Respawn", () => {
  assert.equal(
    getManagedAgentPrimaryActionLabel(agent({ status: "running" })),
    "Stop",
  );
  assert.equal(
    getManagedAgentPrimaryActionLabel(agent({ status: "stopped" })),
    "Respawn",
  );
});

// ── stopManagedAgentWithRules ───────────────────────────────────────────────

test("stopping a provider agent is refused visibly, with no stop dispatched", async () => {
  let stops = 0;
  await assert.rejects(
    stopManagedAgentWithRules({
      agent: agent({
        status: "running",
        backend: { type: "provider", id: "prov-1", config: {} },
      }),
      stopManagedAgent: async () => {
        stops++;
      },
    }),
    /cannot be stopped/,
  );
  assert.equal(
    stops,
    0,
    "no native stop must be dispatched for a provider agent",
  );
});

test("stopping a local agent delegates the pubkey to the registered stop call", async () => {
  const local = agent({ status: "running" });
  let stoppedPubkey = null;
  await stopManagedAgentWithRules({
    agent: local,
    stopManagedAgent: async (pubkey) => {
      stoppedPubkey = pubkey;
    },
  });
  assert.equal(stoppedPubkey, local.pubkey);
});

test("stop failures propagate rather than reporting false success", async () => {
  await assert.rejects(
    stopManagedAgentWithRules({
      agent: agent({ status: "running" }),
      stopManagedAgent: async () => {
        throw new Error("process already exited");
      },
    }),
    /process already exited/,
  );
});
