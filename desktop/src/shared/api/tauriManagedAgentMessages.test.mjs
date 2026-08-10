import assert from "node:assert/strict";
import test from "node:test";

const calls = [];
globalThis.window = globalThis.window ?? {};
globalThis.window.__TAURI_INTERNALS__ = {
  invoke: async (cmd, args) => {
    calls.push({ cmd, args });
    return {
      event_id: "event-1",
      parent_event_id: null,
      root_event_id: null,
      depth: 0,
      created_at: 1_700_000_000,
    };
  },
  transformCallback: () => 1,
  unregisterCallback: () => {},
};

const { sendManagedAgentChannelMessage } = await import(
  "./tauriManagedAgentMessages.ts"
);

test("welcome kickoff transports child AgentIds without changing its human mention", async () => {
  const senderRecordPubkey = "1".repeat(64);
  const teammateRecordPubkey = "2".repeat(64);
  const teammateChildAgentId = "3".repeat(64);
  const humanAgentId = "4".repeat(64);

  await sendManagedAgentChannelMessage(
    {
      agentPubkey: senderRecordPubkey,
      channelId: "welcome-channel",
      content: "Welcome to the team",
      mentionPubkeys: [teammateRecordPubkey, humanAgentId],
    },
    async (mentions) => {
      assert.deepEqual(mentions, [teammateRecordPubkey, humanAgentId]);
      return [teammateChildAgentId, humanAgentId];
    },
  );

  const sendCall = calls.find(
    (call) => call.cmd === "send_managed_agent_channel_message",
  );
  assert.ok(sendCall);
  assert.deepEqual(sendCall.args.input.mentionPubkeys, [
    teammateChildAgentId,
    humanAgentId,
  ]);
  assert.ok(!sendCall.args.input.mentionPubkeys.includes(teammateRecordPubkey));
});
