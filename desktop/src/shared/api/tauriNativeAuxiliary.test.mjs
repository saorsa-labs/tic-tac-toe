import assert from "node:assert/strict";
import test from "node:test";

const calls = [];
let responseFor = null;
globalThis.window = globalThis.window ?? {};
globalThis.window.__TAURI_INTERNALS__ = {
  invoke: async (cmd, args) => {
    calls.push({ cmd, args });
    return responseFor?.(cmd, args);
  },
  transformCallback: () => 0,
  unregisterCallback: () => {},
};

function respond(fn) {
  responseFor = fn;
  calls.length = 0;
}

const {
  x0xAddTask,
  x0xGetAgentCard,
  x0xGetStoreValue,
  x0xListStoreKeys,
  x0xListStores,
  x0xPutStoreValue,
  x0xUpdateTask,
} = await import("./tauriNativeAuxiliary.ts");

test("store list and key metadata map exact x0xd snake-case envelopes", async () => {
  respond((cmd) => {
    if (cmd === "x0x_list_stores") {
      return {
        stores: [
          {
            id: "topic",
            topic: "topic",
            owner: "aa".repeat(32),
            policy: "append_only",
            version: 3,
            policy_version: 2,
            ownership_status: "anchored",
            durability_degraded: false,
          },
        ],
      };
    }
    return {
      keys: [
        {
          key: "workflow-1",
          content_type: "application/json",
          content_hash: "bb".repeat(32),
          size: 7,
          updated_at: 123,
        },
      ],
    };
  });

  assert.deepEqual((await x0xListStores())[0], {
    id: "topic",
    topic: "topic",
    owner: "aa".repeat(32),
    policy: "append_only",
    version: 3,
    policyVersion: 2,
    ownershipStatus: "anchored",
    durabilityDegraded: false,
  });
  assert.deepEqual((await x0xListStoreKeys("topic"))[0], {
    key: "workflow-1",
    contentType: "application/json",
    contentHash: "bb".repeat(32),
    size: 7,
    updatedAtMs: 123,
  });
});

test("KV values cross the command seam as base64 but feature code gets bytes", async () => {
  respond((cmd) =>
    cmd === "x0x_get_store_value"
      ? {
          key: "k",
          value: "aGVsbG8=",
          content_hash: "cc".repeat(32),
          content_type: "text/plain",
          metadata: {},
          created_at: 1,
          updated_at: 2,
        }
      : { ok: true },
  );
  const entry = await x0xGetStoreValue("topic", "k");
  assert.equal(new TextDecoder().decode(entry.value), "hello");

  await x0xPutStoreValue({
    storeId: "topic",
    key: "k",
    value: new TextEncoder().encode("hello"),
    contentType: "text/plain",
  });
  assert.deepEqual(calls.at(-1), {
    cmd: "x0x_put_store_value",
    args: {
      storeId: "topic",
      key: "k",
      valueB64: "aGVsbG8=",
      contentType: "text/plain",
    },
  });
});

test("task mutation wrappers preserve local-only advisory semantics", async () => {
  respond((cmd) =>
    cmd === "x0x_add_task"
      ? { task_id: "dd".repeat(32), version: 4, committed: "local" }
      : {
          version: 5,
          fence_token: "epoch:5",
          committed: "local",
          resolution: {
            agent_id: "ee".repeat(32),
            locally_winning: true,
            current_winner: {
              agent_id: "ee".repeat(32),
              timestamp_ms: 99,
            },
            pending_convergence: true,
          },
          cas: { scope: "local_replica" },
          execution: { authorization: "advisory" },
          exclusive: false,
        },
  );
  assert.deepEqual(await x0xAddTask({ listId: "inbox", title: "Ship" }), {
    taskId: "dd".repeat(32),
    version: 4,
    committed: "local",
  });
  const receipt = await x0xUpdateTask({
    listId: "inbox",
    taskId: "dd".repeat(32),
    action: "claim",
    fenceToken: "epoch:4",
  });
  assert.equal(receipt.casScope, "local_replica");
  assert.equal(receipt.authorization, "advisory");
  assert.equal(receipt.exclusive, false);
  assert.equal(receipt.resolution.pendingConvergence, true);
});

test("AgentCard wrapper unwraps the signed card and maps KEM bytes", async () => {
  respond(() => ({
    link: "x0x://agent/card",
    card: {
      display_name: "Release bot",
      agent_id: "11".repeat(32),
      machine_id: "22".repeat(32),
      addresses: [],
      groups: [{ name: "Dev", invite_link: "x0x://invite/dev" }],
      stores: [{ name: "tasks", topic: "tasks" }],
      created_at: 7,
      dm_capabilities: {
        max_protocol_version: 1,
        gossip_inbox: true,
        kem_algorithm: "ML-KEM-768",
        max_envelope_bytes: 49152,
        kem_public_key: [1, 2, 3],
      },
      agent_public_key: "33",
      signature: "44",
    },
  }));
  const result = await x0xGetAgentCard({
    displayName: "Release bot",
    includeGroups: true,
  });
  assert.equal(result.link, "x0x://agent/card");
  assert.equal(result.card.agentId, "11".repeat(32));
  assert.deepEqual([...result.card.dmCapabilities.kemPublicKey], [1, 2, 3]);
  assert.deepEqual(calls[0], {
    cmd: "x0x_get_agent_card",
    args: { displayName: "Release bot", includeGroups: true },
  });
});
