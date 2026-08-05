import assert from "node:assert/strict";
import test from "node:test";

const calls = [];
globalThis.window = globalThis.window ?? {};
globalThis.window.__TAURI_INTERNALS__ = {
  invoke: async (cmd, args) => {
    calls.push({ cmd, args });
    if (cmd === "x0x_mint_group_invite") {
      return {
        invite_link: "x0x://invite/one-time",
        group_id: "g1",
        group_name: "Engineering",
        expires_at: 123,
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
  x0xMintGroupInvite,
  x0xUpdateGroupPolicy,
} = await import("@/shared/api/tauriNativeX0x");

function takeCall() {
  assert.equal(calls.length, 1);
  return calls.pop();
}

test("group create uses the registered Rust input request shape", async () => {
  await x0xCreateGroup({
    name: "Engineering",
    description: "Build",
    displayName: "Ada",
    preset: "private_secure",
  });
  assert.deepEqual(takeCall(), {
    cmd: "x0x_create_group",
    args: {
      input: {
        name: "Engineering",
        description: "Build",
        displayName: "Ada",
        preset: "private_secure",
      },
    },
  });
});

test("member mutations nest camelCase fields under input", async () => {
  await x0xAddGroupMember({
    groupId: "g1",
    agentId: "aa".repeat(32),
    displayName: "Ada",
    treekemKeyPackageB64: "a2V5",
  });
  assert.deepEqual(takeCall(), {
    cmd: "x0x_add_group_member",
    args: {
      input: {
        groupId: "g1",
        agentId: "aa".repeat(32),
        displayName: "Ada",
        treekemKeyPackageB64: "a2V5",
      },
    },
  });

  await x0xBanGroupMember("g1", "bb".repeat(32));
  assert.deepEqual(takeCall(), {
    cmd: "x0x_ban_group_member",
    args: { input: { groupId: "g1", agentId: "bb".repeat(32) } },
  });
});

test("policy update and invite mint match request structs and map response", async () => {
  await x0xUpdateGroupPolicy("g1", {
    preset: "public_request_secure",
    readAccess: "members",
  });
  assert.deepEqual(takeCall(), {
    cmd: "x0x_update_group_policy",
    args: {
      input: {
        groupId: "g1",
        preset: "public_request_secure",
        discoverability: null,
        admission: null,
        confidentiality: null,
        readAccess: "members",
        writeAccess: null,
      },
    },
  });

  const invite = await x0xMintGroupInvite({ groupId: "g1", expirySecs: 60 });
  assert.deepEqual(takeCall(), {
    cmd: "x0x_mint_group_invite",
    args: { input: { groupId: "g1", expirySecs: 60 } },
  });
  assert.deepEqual(invite, {
    inviteLink: "x0x://invite/one-time",
    groupId: "g1",
    groupName: "Engineering",
    expiresAtMs: 123,
  });
});
