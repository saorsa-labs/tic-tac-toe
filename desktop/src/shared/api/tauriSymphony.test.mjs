import assert from "node:assert/strict";
import test from "node:test";

import {
  cancelCompanyRun,
  fromRawInstantiationResult,
  fromRawTask,
  fromRawTemplate,
  fromRawWorker,
  getSymphonyDaemonStatus,
  instantiateCompanyTemplate,
  listCompanyInstances,
  listSymphonyApprovals,
  listSymphonyProofs,
  resumeCompanyInstance,
} from "@/shared/api/tauriSymphony.ts";

// --- Tauri invoke mock ---------------------------------------------------
// invokeTauri resolves to window.__TAURI_INTERNALS__.invoke at CALL time, so a
// fresh table per test keeps each contract hermetic and order-independent.

/**
 * @param {Record<string, unknown | ((args: unknown) => unknown)>} table
 *   Maps a backend command name to either a fixed return value or a function
 *   of its args. Unknown commands throw so a stale/wrong command is loud.
 */
function mockTauri(table) {
  const calls = [];
  globalThis.window = globalThis.window ?? {};
  globalThis.window.__TAURI_INTERNALS__ = {
    invoke: async (cmd, args) => {
      const recorded = { cmd, args: args ?? {} };
      calls.push(recorded);
      if (!(cmd in table)) {
        throw new Error(`unexpected tauri command in test: ${cmd}`);
      }
      const handler = table[cmd];
      return typeof handler === "function" ? handler(args ?? {}) : handler;
    },
    transformCallback: () => 0,
    unregisterCallback: () => {},
  };
  return calls;
}

// --- Wire-shape mapping: tasks & workers ---------------------------------

test("fromRawTask maps every supervision state to a UI status", () => {
  // taskStatus drives both the task stream and the derived run status; the
  // "completed" alias and the unknown->backlog fallback are the regression-
  // prone boundaries. One assertion per equivalence class.
  const cases = [
    { state: "done", status: "done" },
    { state: "completed", status: "done" },
    { state: "blocked", status: "blocked" },
    { state: "in_progress", status: "running" },
    { state: "running", status: "running" },
    { state: "claimed", status: "claimed" },
    { state: "queued", status: "backlog" },
    { state: "", status: "backlog" },
  ];
  for (const { state, status } of cases) {
    assert.equal(
      fromRawTask({
        id: "t",
        identifier: "C-1",
        title: "x",
        state,
        labels: [],
      }).status,
      status,
      `state=${JSON.stringify(state)}`,
    );
  }
});

test("fromRawTask defaults absent assignee/priority to null, not undefined", () => {
  // The task card and run-derivation branch on === null for assigneeId; a
  // regression to undefined would silently mis-route both.
  assert.deepEqual(
    fromRawTask({
      id: "t",
      identifier: "C-1",
      title: "x",
      state: "backlog",
      labels: [],
    }),
    {
      id: "t",
      identifier: "C-1",
      title: "x",
      status: "backlog",
      assigneeId: null,
      labels: [],
      priority: null,
    },
  );
});

test("fromRawWorker maps load boundary to busy/idle and falls back role to null", () => {
  // current_load === 0 is the idle boundary; an empty preset list must yield a
  // null role, not undefined, so worker rendering never shows "undefined".
  assert.deepEqual(
    fromRawWorker({
      agent_id: "ab".repeat(32),
      current_load: 0,
      max_load: 2,
      runner_presets: [],
    }),
    {
      agentId: "ab".repeat(32),
      displayName: "ab".repeat(32),
      role: null,
      status: "idle",
      currentLoad: 0,
      maxLoad: 2,
    },
  );
  assert.equal(
    fromRawWorker({
      agent_id: "ab".repeat(32),
      current_load: 1,
      max_load: 2,
      runner_presets: ["codex"],
    }).status,
    "busy",
  );
});

test("fromRawTemplate maps the built-in Company template and its agents", () => {
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

// --- Provisioning outcome: status discrimination + structured errors ------

test("fromRawInstantiationResult maps every lifecycle status and never fakes complete", () => {
  // ManifestStatus serializes snake_case. The attempt is terminal in one of
  // complete, resumable (durable, retryable), or cancelled (terminal). Only an
  // explicit "complete" resolves to complete; "cancelled" stays cancelled; and
  // every other token (resumable, in_progress, a legacy fixture with no status
  // field, or any unrecognized value) is treated as resumable — NEVER silently
  // promoted to complete. The run-open effect keys off runId, the resumable
  // banner off status === "resumable", and a cancelled run must render
  // terminal; a flipped default would mislead the user into retrying a dead
  // run or missing a resumable one.
  const base = {
    instance_id: "acme-1",
    run_id: "task-1",
    groups: [],
    agents: [
      {
        agent_id: "ef".repeat(32),
        role: "staff-engineer",
        group_id: "engineering",
      },
    ],
    workflow_md: "---\ntracker:\n",
    errors: [],
  };
  const cases = [
    { status: "complete", expected: "complete" },
    { status: "cancelled", expected: "cancelled" },
    { status: "resumable", expected: "resumable" },
    { status: "in_progress", expected: "resumable" },
    { status: undefined, expected: "resumable" },
    { status: "garbage", expected: "resumable" },
  ];
  for (const { status, expected } of cases) {
    assert.equal(
      fromRawInstantiationResult({
        ...base,
        ...(status === undefined ? {} : { status }),
      }).status,
      expected,
      `status=${JSON.stringify(status)}`,
    );
  }
});

test("fromRawInstantiationResult preserves the full structured provisioning error set", () => {
  // The Company screen splits daemon/run soft-failures (kind "daemon", red)
  // from resumable provisioning step errors (amber) and renders ref+message.
  // Each error must round-trip intact — not just its kind.
  const result = fromRawInstantiationResult({
    instance_id: "acme-1",
    run_id: null,
    status: "resumable",
    groups: [],
    agents: [],
    workflow_md: null,
    errors: [
      { kind: "agent", ref: "sales", message: "runtime missing" },
      { kind: "daemon", ref: "run-9", message: "symphonyd exited" },
      {
        kind: "membership",
        ref: "staff-engineer->engineering",
        message: "denied",
      },
    ],
  });
  assert.deepEqual(result.errors, [
    { kind: "agent", ref: "sales", message: "runtime missing" },
    { kind: "daemon", ref: "run-9", message: "symphonyd exited" },
    {
      kind: "membership",
      ref: "staff-engineer->engineering",
      message: "denied",
    },
  ]);
  assert.deepEqual(result.agents, []);
});

// --- Daemon supervision status: available / activeInstanceId / error ------

test("getSymphonyDaemonStatus maps running, active instance, and owned signal", async () => {
  const calls = mockTauri({
    symphony_supervision_status: {
      running: true,
      base_url: "http://127.0.0.1:443",
      owned: true,
      active_instance_id: "acme-1",
      error: null,
    },
  });
  const status = await getSymphonyDaemonStatus();
  assert.deepEqual(calls, [{ cmd: "symphony_supervision_status", args: {} }]);
  assert.deepEqual(status, {
    available: true,
    baseUrl: "http://127.0.0.1:443",
    owned: true,
    activeInstanceId: "acme-1",
    error: null,
  });
});

test("getSymphonyDaemonStatus nulls activeInstanceId when no instance is bound", async () => {
  mockTauri({
    symphony_supervision_status: {
      running: true,
      base_url: null,
      owned: false,
      active_instance_id: null,
      error: null,
    },
  });
  const { activeInstanceId } = await getSymphonyDaemonStatus();
  assert.equal(activeInstanceId, null);
});

test("getSymphonyDaemonStatus synthesizes a down-label only when the daemon is down with no backend detail", async () => {
  // The fallback label is a last resort: a backend-captured error always wins,
  // and a running daemon is never mislabeled as down. Each row is one branch
  // of the `error ?? (running ? null : <label>)` expression.
  const cases = [
    { running: true, error: null, expectedError: null },
    {
      running: false,
      error: null,
      expectedError: "x0x-symphonyd is not running",
    },
    {
      running: false,
      error: "port 443 in use",
      expectedError: "port 443 in use",
    },
    { running: true, error: "degraded", expectedError: "degraded" },
  ];
  for (const { running, error, expectedError } of cases) {
    mockTauri({
      symphony_supervision_status: {
        running,
        base_url: null,
        owned: false,
        active_instance_id: null,
        error,
      },
    });
    const status = await getSymphonyDaemonStatus();
    assert.equal(status.available, running, `running=${running}`);
    assert.equal(
      status.error,
      expectedError,
      `running=${running} error=${JSON.stringify(error)}`,
    );
  }
});

// --- Cancel identity: the Company instance id, never a raw Symphony run id -

test("cancelCompanyRun forwards the instance id verbatim as { instanceId }", async () => {
  // The cancel mutation is invoked with the persisted Company instance id.
  // The backend cancel_company_run command keys on `instanceId` (camelCase);
  // forwarding a run id or snake_casing here would cancel the wrong thing or
  // nothing. Pin the exact arg name and the exact value.
  const calls = mockTauri({ cancel_company_run: {} });
  await cancelCompanyRun("acme-1");
  assert.deepEqual(calls, [
    { cmd: "cancel_company_run", args: { instanceId: "acme-1" } },
  ]);
});

// --- Resume identity: the Company instance id, never a minted id -----------

test("resumeCompanyInstance forwards the instance id verbatim as { instanceId }", async () => {
  // Resume re-runs the durable lifecycle for an EXISTING instance by its exact
  // id — it must never mint a new id. The backend resume_company_instance keys
  // on `instanceId` (camelCase); forwarding a run id, snake_casing, or dropping
  // the arg would resume/no-op the wrong instance. Pin the exact arg + value.
  const calls = mockTauri({
    resume_company_instance: {
      instance_id: "acme-1",
      run_id: "task-1",
      status: "complete",
      groups: [],
      agents: [],
      workflow_md: null,
      errors: [],
    },
  });
  await resumeCompanyInstance("acme-1");
  assert.deepEqual(calls, [
    { cmd: "resume_company_instance", args: { instanceId: "acme-1" } },
  ]);
});

// --- Instantiate wire: templateId + input forwarded under backend arg names -

test("instantiateCompanyTemplate forwards templateId and input verbatim", async () => {
  // The create path forwards the chosen template id and the operator input
  // (display name) under the exact backend arg names — the reservation and the
  // run-issue title both depend on them. A snake_cased or renamed arg would
  // start the wrong template or silently drop the display name.
  const calls = mockTauri({
    instantiate_company_template: {
      instance_id: "acme-1",
      run_id: null,
      status: "resumable",
      groups: [],
      agents: [],
      workflow_md: null,
      errors: [],
    },
  });
  await instantiateCompanyTemplate("software-dev-and-sales", {
    displayName: "Acme",
  });
  assert.deepEqual(calls, [
    {
      cmd: "instantiate_company_template",
      args: {
        templateId: "software-dev-and-sales",
        input: { displayName: "Acme" },
      },
    },
  ]);
});

// --- Persisted Company instances: camelCase wire + lifecycle normalization -

test("listCompanyInstances reads the camelCase wire and defaults active to false", async () => {
  // The Rust CompanyInstanceSummary serializes with rename_all="camelCase".
  // Reading snake_case here previously yielded undefined instance ids, so this
  // pins that the camelCase keys are the ones consumed. Distinct values ensure
  // a revert to snake_case reddens the assertion instead of passing silently.
  const calls = mockTauri({
    list_company_instances: [
      {
        instanceId: "acme-1",
        runId: "run-9",
        status: "in_progress",
        active: true,
      },
      { instanceId: "acme-2", runId: null, status: "complete" },
    ],
  });
  const instances = await listCompanyInstances();
  assert.deepEqual(calls, [{ cmd: "list_company_instances", args: {} }]);
  assert.deepEqual(instances, [
    {
      instanceId: "acme-1",
      runId: "run-9",
      status: "in_progress",
      active: true,
    },
    { instanceId: "acme-2", runId: null, status: "complete", active: false },
  ]);
});

test("listCompanyInstances normalizes known lifecycle statuses and defaults the rest to resumable", async () => {
  // resumable/in_progress/cancelled/complete are the live lifecycle states; any
  // absent or unrecognized status (e.g. undefined, a future "archived") resolves
  // to "resumable" — NEVER "complete", because complete is a lifecycle-proven
  // claim owned by the backend (the orchestrator never marks Complete without a
  // durable run_id). A flipped default would mislabel an unknown instance as
  // complete, hiding it from the resumable/cancel surface.
  mockTauri({
    list_company_instances: [
      { instanceId: "a", runId: null, status: "resumable" },
      { instanceId: "b", runId: null, status: "in_progress" },
      { instanceId: "c", runId: null, status: "cancelled" },
      { instanceId: "d", runId: null, status: undefined },
      { instanceId: "e", runId: "run-e", status: "complete" },
      { instanceId: "f", runId: null, status: "archived" },
    ],
  });
  const instances = await listCompanyInstances();
  const statuses = instances.map((i) => i.status);
  assert.deepEqual(statuses, [
    "resumable",
    "in_progress",
    "cancelled",
    "resumable",
    "complete",
    "resumable",
  ]);
});

// --- Global Symphony scoping: approvals & proofs exclude unrelated runs ----

test("listSymphonyApprovals drops approvals belonging to other runs", async () => {
  // symphony_approvals_pending returns the global pending set; the adapter
  // must narrow it to the requested run so a Company run never surfaces
  // another run's human-approval gate.
  const calls = mockTauri({
    symphony_approvals_pending: [
      {
        issue_id: "run-9",
        title: "Approve deploy",
        state: "blocked",
        content_hash: "deadbeef",
        signer_agent_id: "ab".repeat(32),
      },
      {
        issue_id: "run-other",
        title: "Unrelated",
        state: "blocked",
        content_hash: "00",
        signer_agent_id: "cd".repeat(32),
      },
    ],
  });
  const approvals = await listSymphonyApprovals("run-9");
  assert.equal(calls[0].cmd, "symphony_approvals_pending");
  assert.equal(approvals.length, 1);
  assert.deepEqual(approvals, [
    {
      token: "run-9",
      runId: "run-9",
      taskId: "run-9",
      title: "Approve deploy",
      contentHash: "deadbeef",
      signerAgentId: "ab".repeat(32),
      status: "pending",
    },
  ]);
});

test("listSymphonyProofs scopes the global proof list to the run and classifies kind", async () => {
  // symphony_proofs lists every proof name globally; only names mentioning the
  // run id are fetched and mapped, and the handoff/completion split keys off
  // the proof name so the two are never swapped.
  mockTauri({
    symphony_proofs: {
      proofs: ["run-9.handoff.md", "run-9.completion.md", "run-other.md"],
    },
    symphony_proof: (args) => {
      const name = args.name;
      return { name, content: `body of ${name}` };
    },
  });
  const proofs = await listSymphonyProofs("run-9");
  assert.deepEqual(proofs, [
    {
      id: "run-9.handoff.md",
      runId: "run-9",
      taskId: "run-9",
      kind: "handoff",
      summary: "body of run-9.handoff.md",
    },
    {
      id: "run-9.completion.md",
      runId: "run-9",
      taskId: "run-9",
      kind: "completion",
      summary: "body of run-9.completion.md",
    },
  ]);
});
