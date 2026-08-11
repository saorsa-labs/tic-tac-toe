import assert from "node:assert/strict";
import test from "node:test";

import { shouldHideAgentFromMentions } from "../../agents/lib/agentAutocompleteEligibility.ts";
import { getMentionOffset, hasMention } from "./hasMention.ts";
import { isMentionCandidateManagedOrMember } from "./useMentions.ts";

const REMOTE_CHANNEL_AGENT_ID =
  "8f83d6b7f3d74f7d933ae3a54dd8c6cc85c7f98e531c16e5a827b953441a8d67";

function remoteAgentCandidate(overrides = {}) {
  return {
    kind: "identity",
    pubkey: REMOTE_CHANNEL_AGENT_ID,
    displayName: "mira",
    isAgent: true,
    isMember: true,
    ownerPubkey: "f".repeat(64),
    ...overrides,
  };
}

function includeCandidate(candidate, overrides = {}) {
  const policy = {
    managedAgentPubkeys: new Set(),
    mentionableAgentPubkeys: new Set(),
    directoryAgentPubkeys: new Set(),
    ...overrides,
  };
  if (
    !isMentionCandidateManagedOrMember(candidate, policy.managedAgentPubkeys)
  ) {
    return false;
  }
  return !shouldHideAgentFromMentions({
    isAgent: candidate.isAgent === true,
    isMember: candidate.isMember === true,
    pubkey: candidate.pubkey,
    mentionableAgentPubkeys: policy.mentionableAgentPubkeys,
    directoryAgentPubkeys: policy.directoryAgentPubkeys,
  });
}

test("a remote channel-member agent absent local directories stays eligible and emits its exact native mention id", async () => {
  const candidate = remoteAgentCandidate();
  assert.equal(includeCandidate(candidate), true);

  const calls = [];
  globalThis.window = globalThis.window ?? {};
  globalThis.window.__TAURI_INTERNALS__ = {
    invoke: async (cmd, args) => {
      calls.push({ cmd, args });
      return null;
    },
    transformCallback: () => 1,
    unregisterCallback: () => {},
  };
  const { sendNativeMessage } = await import("./nativeMessaging.ts");
  await sendNativeMessage(
    {
      channel: {
        id: "general-group",
        name: "general",
        channelType: "stream",
        visibility: "open",
        description: "",
        topic: null,
        purpose: null,
        memberCount: 2,
        memberPubkeys: [],
        lastMessageAt: null,
        archivedAt: null,
        participants: [],
        participantPubkeys: [],
        isMember: true,
        ttlSeconds: null,
        ttlDeadline: null,
      },
      content: "Ask @mira please reply",
      identity: { agentId: "d".repeat(64) },
      mentionPubkeys: [candidate.pubkey],
    },
    async (mentions) => mentions ?? [],
  );

  const sendCall = calls.find((call) => call.cmd === "x0x_send_group_message");
  assert.ok(sendCall);
  const envelope = JSON.parse(sendCall.args.input.body);
  assert.deepEqual(envelope.mentions, [REMOTE_CHANNEL_AGENT_ID]);
});

test("a remote non-member agent absent local directories stays hidden", () => {
  assert.equal(
    includeCandidate(remoteAgentCandidate({ isMember: false })),
    false,
  );
});

test("an explicitly excluded directory agent stays hidden even as a channel member", () => {
  assert.equal(
    includeCandidate(remoteAgentCandidate(), {
      directoryAgentPubkeys: new Set([REMOTE_CHANNEL_AGENT_ID]),
    }),
    false,
  );
});

// ── Plain @mention ────────────────────────────────────────────────────

test("matches @Name at start of string", () => {
  assert.equal(hasMention("@Alice hello", "Alice"), true);
});

test("matches @Name after whitespace", () => {
  assert.equal(hasMention("hey @Alice", "Alice"), true);
});

test("matches the first member in a parenthesized team expansion", () => {
  assert.equal(hasMention("Launch Team(@Planner @Builder)", "Planner"), true);
  assert.equal(hasMention("Launch Team(@Planner @Builder)", "Builder"), true);
});

test("matches @Name at end of string", () => {
  assert.equal(hasMention("hello @Alice", "Alice"), true);
});

test("match is case-insensitive", () => {
  assert.equal(hasMention("@alice", "Alice"), true);
  assert.equal(hasMention("@ALICE", "Alice"), true);
});

test("does not match without @ prefix", () => {
  assert.equal(hasMention("Alice hello", "Alice"), false);
});

test("does not match @Name embedded in a word (email-style)", () => {
  assert.equal(hasMention("user@Alice.com", "Alice"), false);
});

// ── Bold-wrapped mentions (**@Name**) ─────────────────────────────────

test("matches **@Name** (bold-wrapped)", () => {
  assert.equal(hasMention("**@Alice**", "Alice"), true);
});

test("matches **@Name** after whitespace", () => {
  assert.equal(hasMention("hey **@Alice**", "Alice"), true);
});

test("matches *@Name* (italic-wrapped)", () => {
  assert.equal(hasMention("*@Alice*", "Alice"), true);
});

test("matches ***@Name*** (bold+italic-wrapped)", () => {
  assert.equal(hasMention("***@Alice***", "Alice"), true);
});

test("matches __@Name__ (underscore bold-wrapped)", () => {
  assert.equal(hasMention("__@Alice__", "Alice"), true);
});

test("matches _@Name_ (underscore italic-wrapped)", () => {
  assert.equal(hasMention("_@Alice_", "Alice"), true);
});

test("matches ||@Name|| (spoiler-wrapped)", () => {
  assert.equal(hasMention("||@Alice||", "Alice"), true);
});

test("matches @Name at the end of spoiler content", () => {
  assert.equal(hasMention("||hi @Alice||", "Alice"), true);
});

// ── Boundary conditions ───────────────────────────────────────────────

test("matches @Name followed by punctuation", () => {
  assert.equal(hasMention("@Alice, hello", "Alice"), true);
  assert.equal(hasMention("@Alice!", "Alice"), true);
  assert.equal(hasMention("@Alice.", "Alice"), true);
  assert.equal(hasMention("@Alice?", "Alice"), true);
});

test("matches multi-word display name", () => {
  assert.equal(hasMention("@John Doe said hi", "John Doe"), true);
});

test("matches multi-word display name bold-wrapped", () => {
  assert.equal(hasMention("**@John Doe**", "John Doe"), true);
});

test("handles regex special characters in name", () => {
  assert.equal(hasMention("@alice (admin)", "alice (admin)"), true);
});

test("does not false-positive on partial name match", () => {
  // "Al" should not match inside "@Alice"
  assert.equal(hasMention("@Alice", "Al"), false);
});

// ── Markdown code ─────────────────────────────────────────────────────

test("ignores mentions in inline code", () => {
  assert.equal(hasMention("run `notify @Alice now`", "Alice"), false);
  assert.equal(hasMention("run ``notify `x` @Alice``", "Alice"), false);
});

test("ignores mentions in fenced code blocks", () => {
  assert.equal(
    hasMention("before\n```ts\nnotify(@Alice)\n```\nafter", "Alice"),
    false,
  );
  assert.equal(hasMention("~~~\r\n@Alice\r\n~~~", "Alice"), false);
});

test("ignores mentions in indented code blocks", () => {
  assert.equal(hasMention("before\n    @Alice\nafter", "Alice"), false);
  assert.equal(hasMention("before\n\t@Alice\nafter", "Alice"), false);
});

test("still matches prose mentions around code", () => {
  assert.equal(hasMention("`@Alice` then @Alice", "Alice"), true);
  assert.equal(hasMention("```\n@Alice\n```\n@Alice", "Alice"), true);
  assert.equal(hasMention("    @Alice\n@Alice", "Alice"), true);
});

test("preserves the original offset after masked code", () => {
  const text = "`@Alice` then @Alice";
  assert.equal(getMentionOffset(text, "Alice"), text.lastIndexOf("@Alice"));
});

test("does not treat escaped or unclosed backticks as code", () => {
  assert.equal(hasMention("\\` @Alice", "Alice"), true);
  assert.equal(hasMention("` @Alice", "Alice"), true);
});

test("requires matching inline-code delimiter lengths", () => {
  assert.equal(hasMention("`` @Alice ` still code ``", "Alice"), false);
  assert.equal(hasMention("`` @Alice `", "Alice"), true);
});
