import assert from "node:assert/strict";
import test from "node:test";

const calls = [];
globalThis.window = globalThis.window ?? {};
globalThis.window.__TAURI_INTERNALS__ = {
  invoke: async (cmd, args) => {
    calls.push({ cmd, args });
    if (cmd === "x0x_create_group") {
      return {
        groupId: "group-1",
        name: args.input?.name ?? "",
        description: args.input?.description ?? "",
        memberCount: 1,
      };
    }
    return {};
  },
  transformCallback: () => 0,
  unregisterCallback: () => {},
};

const {
  x0xAddGroupMember,
  x0xBanGroupMember,
  x0xCreateGroup,
  x0xRemoveGroupMember,
} = await import("@/shared/api/tauriNativeX0x");

const { createChannel } = await import("@/shared/api/tauriChannels");

function takeCall() {
  assert.equal(calls.length, 1);
  return calls.pop();
}

test("group create forwards the public_open preset the Rust gate accepts", async () => {
  await x0xCreateGroup({
    name: "Engineering",
    description: "Build",
    displayName: "Ada",
    preset: "public_open",
  });
  assert.deepEqual(takeCall(), {
    cmd: "x0x_create_group",
    args: {
      input: {
        name: "Engineering",
        description: "Build",
        displayName: "Ada",
        preset: "public_open",
      },
    },
  });
});

test("member mutations nest camelCase fields under input and carry no treekem material", async () => {
  // A caller may still attempt to attach secure-group key material; the TS
  // seam MUST drop it so no TreeKEM/MLS key package ever crosses Tauri.
  await x0xAddGroupMember({
    groupId: "g1",
    agentId: "aa".repeat(32),
    displayName: "Ada",
    treekemKeyPackageB64: "a2V5",
  });
  const forwarded = takeCall();
  assert.deepEqual(forwarded, {
    cmd: "x0x_add_group_member",
    args: {
      input: {
        groupId: "g1",
        agentId: "aa".repeat(32),
        displayName: "Ada",
      },
    },
  });
  // Defense: no treekem / key-package key of any casing crosses the seam.
  const inputKeys = Object.keys(forwarded.args.input);
  assert.ok(
    inputKeys.every((k) => !/treekem|keypackage|key_package/i.test(k)),
    `treekem/key-package material leaked into add-member input: ${inputKeys}`,
  );

  await x0xBanGroupMember("g1", "bb".repeat(32));
  assert.deepEqual(takeCall(), {
    cmd: "x0x_ban_group_member",
    args: { input: { groupId: "g1", agentId: "bb".repeat(32) } },
  });
});

test("createChannel ignores a stale private visibility request and projects an open public_open channel", async () => {
  // A caller may still attach the legacy `private` visibility; the seam MUST
  // drop it — the policy-mutation axis is removed — and back the channel with
  // the daemon's public_open group, projecting visibility "open" regardless.
  const channel = await createChannel({
    name: "Welcome",
    description: "get oriented",
    channelType: "stream",
    visibility: "private",
  });

  // Only the public_open preset is forwarded; no visibility/policy axis leaks.
  assert.deepEqual(takeCall(), {
    cmd: "x0x_create_group",
    args: {
      input: {
        name: "Welcome",
        description: "get oriented",
        displayName: null,
        preset: "public_open",
      },
    },
  });

  // The returned channel is open and backed by the daemon group id, not the
  // requested private label.
  assert.equal(channel.id, "group-1");
  assert.equal(channel.channelType, "stream");
  assert.equal(channel.visibility, "open");
});

test("remove member targets the delete-member endpoint, distinct from ban", async () => {
  await x0xRemoveGroupMember("g1", "cc".repeat(32));
  assert.deepEqual(takeCall(), {
    cmd: "x0x_remove_group_member",
    args: { input: { groupId: "g1", agentId: "cc".repeat(32) } },
  });
});
