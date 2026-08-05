import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

import {
  fromRawInstantiationResult,
  fromRawTask,
  fromRawTemplate,
  fromRawWorker,
} from "@/shared/api/tauriSymphony.ts";

test("maps the real symphonyd task and worker wire shapes", () => {
  assert.deepEqual(
    fromRawTask({
      id: "task-1",
      identifier: "CMP-1",
      title: "Ship",
      state: "in_progress",
      priority: 1,
      labels: ["company"],
      claim_by: "ab".repeat(32),
    }),
    {
      id: "task-1",
      identifier: "CMP-1",
      title: "Ship",
      status: "running",
      priority: 1,
      labels: ["company"],
      assigneeId: "ab".repeat(32),
    },
  );

  assert.deepEqual(
    fromRawWorker({
      agent_id: "cd".repeat(32),
      current_load: 1,
      max_load: 2,
      runner_presets: ["codex"],
    }),
    {
      agentId: "cd".repeat(32),
      displayName: "cd".repeat(32),
      role: "codex",
      status: "busy",
      currentLoad: 1,
      maxLoad: 2,
    },
  );
});

test("maps the built-in Company template picker", () => {
  const template = fromRawTemplate({
    id: "software-dev-and-sales",
    name: "Software Dev & Sales",
    description: "Native team",
    groups: [{ id: "engineering", name: "Engineering", kind: "engineering" }],
    agents: [
      {
        role: "staff-engineer",
        group_id: "engineering",
        runtime: "codex",
        model: null,
      },
    ],
    is_builtin: true,
  });
  assert.equal(template.isBuiltin, true);
  assert.equal(template.agents[0].groupId, "engineering");
});

test("preserves resumable Company outcome and native agent ids", () => {
  const result = fromRawInstantiationResult({
    instance_id: "acme-1",
    run_id: "task-1",
    groups: [{ id: "group-1", name: "Engineering", kind: "engineering" }],
    agents: [
      {
        agent_id: "ef".repeat(32),
        role: "staff-engineer",
        group_id: "engineering",
      },
    ],
    workflow_md: "---\ntracker:\n",
    errors: [{ kind: "agent", ref: "sales", message: "runtime missing" }],
  });
  assert.equal(result.instanceId, "acme-1");
  assert.equal(result.agents[0].agentId, "ef".repeat(32));
  assert.equal(result.errors[0].kind, "agent");
});

test("Company and Symphony types have no legacy wire identifiers", () => {
  const here = path.dirname(fileURLToPath(import.meta.url));
  const source = readFileSync(path.join(here, "symphonyTypes.ts"), "utf8");
  const code = source
    .replace(/\/\*[\s\S]*?\*\//g, "")
    .split("\n")
    .map((line) => line.split("//")[0])
    .join("\n")
    .toLowerCase();
  for (const forbidden of ["nostr", "npub", "nsec", "relayurl", "relay_url"]) {
    assert.equal(code.includes(forbidden), false, forbidden);
  }
});
