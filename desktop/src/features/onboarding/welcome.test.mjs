import assert from "node:assert/strict";
import test from "node:test";

import {
  consumePendingWelcomeChannel,
  ensureStarterChannels,
  ensureWelcomeChannel,
  findPersonalWelcomeChannel,
  isPersonalWelcomeChannel,
  hasEnsuredWelcomeChannel,
  isWelcomeExperienceChannel,
  markWelcomeChannelEnsured,
  rememberPendingWelcomeChannel,
  WELCOME_CHANNEL_DESCRIPTION,
  WELCOME_CHANNEL_NAME,
} from "./welcome.ts";

function makeChannel(overrides = {}) {
  return {
    id: "welcome-channel",
    name: WELCOME_CHANNEL_NAME,
    channelType: "stream",
    visibility: "open",
    description: WELCOME_CHANNEL_DESCRIPTION,
    topic: null,
    purpose: null,
    memberCount: 1,
    memberPubkeys: ["current-user"],
    lastMessageAt: null,
    archivedAt: null,
    participants: [],
    participantPubkeys: [],
    isMember: true,
    ttlSeconds: null,
    ttlDeadline: null,
    ...overrides,
  };
}

function installWindowSessionStorage() {
  const storage = new Map();
  const previousWindow = globalThis.window;

  globalThis.window = {
    sessionStorage: {
      getItem(key) {
        return storage.get(key) ?? null;
      },
      setItem(key, value) {
        storage.set(key, String(value));
      },
      removeItem(key) {
        storage.delete(key);
      },
    },
    localStorage: {
      getItem(key) {
        return storage.get(key) ?? null;
      },
      setItem(key, value) {
        storage.set(key, String(value));
      },
    },
  };

  return {
    restore() {
      if (previousWindow === undefined) {
        delete globalThis.window;
      } else {
        globalThis.window = previousWindow;
      }
    },
  };
}

test("ensureWelcomeChannel creates a personal open Welcome channel when one is missing", async () => {
  const createdChannel = makeChannel();
  const createInputs = [];

  const result = await ensureWelcomeChannel({
    getChannels: async () => [],
    createChannel: async (input) => {
      createInputs.push(input);
      return createdChannel;
    },
  });

  assert.equal(result, createdChannel);
  assert.deepEqual(createInputs, [
    {
      name: WELCOME_CHANNEL_NAME,
      channelType: "stream",
      visibility: "open",
      description: WELCOME_CHANNEL_DESCRIPTION,
    },
  ]);
});

test("ensureWelcomeChannel replaces existing Welcome in forced-fresh development mode", async () => {
  const existingChannel = makeChannel({ id: "old-welcome" });
  const calls = [];

  const result = await ensureWelcomeChannel(
    {
      getChannels: async () => [existingChannel],
      deleteChannel: async (channelId) => {
        calls.push(["delete", channelId]);
      },
      createChannel: async () => {
        calls.push(["create"]);
        return makeChannel({ id: "fresh-welcome" });
      },
    },
    { replaceExisting: true },
  );

  assert.equal(result.id, "fresh-welcome");
  assert.deepEqual(calls, [["delete", "old-welcome"], ["create"]]);
});

test("ensureWelcomeChannel clears ttl on an existing ephemeral Welcome channel", async () => {
  const existingChannel = makeChannel({
    description:
      "An open ephemeral channel for getting oriented in this community.",
    id: "existing-welcome",
    ttlDeadline: "2026-06-11T00:00:00.000Z",
    ttlSeconds: 86400,
  });
  const updateInputs = [];

  const result = await ensureWelcomeChannel({
    getChannels: async () => [existingChannel],
    updateChannel: async (input) => {
      updateInputs.push(input);
      return makeChannel({
        description: input.description,
        id: input.channelId,
        ttlDeadline: null,
        ttlSeconds: null,
      });
    },
    createChannel: async () => makeChannel({ id: "created-welcome" }),
  });

  assert.equal(result.id, "existing-welcome");
  assert.equal(result.ttlSeconds, null);
  assert.deepEqual(updateInputs, [
    {
      channelId: "existing-welcome",
      description: WELCOME_CHANNEL_DESCRIPTION,
      ttlSeconds: null,
    },
  ]);
});

test("ensureWelcomeChannel reuses an existing personal open solo-member Welcome channel", async () => {
  const existingChannel = makeChannel({ id: "existing-welcome" });
  let createCalls = 0;

  const result = await ensureWelcomeChannel({
    getChannels: async () => [existingChannel],
    createChannel: async () => {
      createCalls += 1;
      return makeChannel({ id: "created-welcome" });
    },
  });

  assert.equal(result, existingChannel);
  assert.equal(createCalls, 0);
});

test("ensureWelcomeChannel reuses a Welcome channel with the guide bot", async () => {
  const existingChannel = makeChannel({
    id: "existing-welcome",
    memberCount: 2,
    memberPubkeys: ["current-user", "guide-agent"],
  });
  let createCalls = 0;

  const result = await ensureWelcomeChannel(
    {
      getChannels: async () => [existingChannel],
      createChannel: async () => {
        createCalls += 1;
        return makeChannel({ id: "created-welcome" });
      },
    },
    {
      allowedMemberPubkeys: ["guide-agent"],
    },
  );

  assert.equal(result, existingChannel);
  assert.equal(createCalls, 0);
});

test("ensureWelcomeChannel uses member details to allow bot-only extras", async () => {
  const existingChannel = makeChannel({
    id: "existing-welcome",
    memberCount: 2,
    memberPubkeys: ["current-user", "guide-agent"],
  });
  let createCalls = 0;

  const result = await ensureWelcomeChannel({
    getChannels: async () => [existingChannel],
    getChannelMembers: async () => [
      { pubkey: "current-user", role: "owner", isAgent: false },
      { pubkey: "guide-agent", role: "bot", isAgent: true },
    ],
    createChannel: async () => {
      createCalls += 1;
      return makeChannel({ id: "created-welcome" });
    },
  });

  assert.equal(result, existingChannel);
  assert.equal(createCalls, 0);
});

test("findPersonalWelcomeChannel selects a solo open Welcome channel and ignores shared ones", () => {
  const solo = makeChannel({ id: "solo-welcome" });
  const shared = makeChannel({
    id: "shared-welcome",
    memberCount: 2,
    memberPubkeys: ["current-user", "other-user"],
  });

  // A solo-member open Welcome channel is the personal orientation channel.
  assert.equal(isPersonalWelcomeChannel(solo), true);
  assert.equal(findPersonalWelcomeChannel([solo]).id, "solo-welcome");

  // An open Welcome channel shared with a foreign member is not personal.
  assert.equal(isPersonalWelcomeChannel(shared), false);
  assert.equal(findPersonalWelcomeChannel([shared]), null);
});

test("ensureWelcomeChannel creates an open Welcome channel that survives a list/reload as personal", async () => {
  // welcomeChannelInput is module-private; prove its shape and the list/reload
  // classification consistency end-to-end through ensureWelcomeChannel.
  const createdInputs = [];

  // First pass: no existing channel → create from welcomeChannelInput.
  const first = await ensureWelcomeChannel({
    getChannels: async () => [],
    createChannel: async (input) => {
      createdInputs.push(input);
      // Build the resulting channel from the SAME input ensureWelcomeChannel
      // produced, so the reload path classifies exactly what the create path
      // emits — no independent hardcoding of visibility.
      return makeChannel({
        id: "welcome-channel",
        visibility: input.visibility,
      });
    },
  });

  // The created channel is built from welcomeChannelInput, which MUST be open;
  // the pre-containment `private` label would no longer classify as personal.
  assert.equal(createdInputs.length, 1);
  assert.equal(createdInputs[0].visibility, "open");
  assert.equal(first.visibility, "open");

  // Reload: the created channel now appears in the list (built from the same
  // captured input) and MUST classify as the personal Welcome channel again —
  // reused, never re-created/orphaned. A non-open welcomeChannelInput would
  // fail to reclassify here and trigger a second create.
  let reloadCreateCalls = 0;
  const reloaded = await ensureWelcomeChannel({
    getChannels: async () => [
      makeChannel({ id: first.id, visibility: createdInputs[0].visibility }),
    ],
    createChannel: async () => {
      reloadCreateCalls += 1;
      return makeChannel({ id: "should-not-create" });
    },
  });

  assert.equal(reloaded.id, first.id);
  assert.equal(reloadCreateCalls, 0);
});

test("pending Welcome channel is consumed only after it appears in the channel list", () => {
  const { restore } = installWindowSessionStorage();
  try {
    rememberPendingWelcomeChannel("welcome-channel");

    assert.equal(consumePendingWelcomeChannel(new Set(["general"])), null);
    assert.equal(
      consumePendingWelcomeChannel(new Set(["general", "welcome-channel"])),
      "welcome-channel",
    );
    assert.equal(
      consumePendingWelcomeChannel(new Set(["general", "welcome-channel"])),
      null,
    );
  } finally {
    restore();
  }
});

test("Welcome ensured marker is scoped to the current identity and community", () => {
  const { restore } = installWindowSessionStorage();
  try {
    markWelcomeChannelEnsured("pubkey-a", "wss://community-a.example");

    assert.equal(
      hasEnsuredWelcomeChannel("pubkey-a", "wss://community-a.example"),
      true,
    );
    assert.equal(
      hasEnsuredWelcomeChannel("pubkey-a", "wss://community-b.example"),
      false,
    );
    assert.equal(
      hasEnsuredWelcomeChannel("pubkey-b", "wss://community-a.example"),
      false,
    );
    assert.equal(hasEnsuredWelcomeChannel("pubkey-a", null), false);
    assert.equal(
      hasEnsuredWelcomeChannel(null, "wss://community-a.example"),
      false,
    );
  } finally {
    restore();
  }
});

test("ensureStarterChannels reuses existing open starter channels", async () => {
  const general = makeChannel({
    id: "general-channel",
    name: "general",
    visibility: "open",
  });
  const welcomeEveryone = makeChannel({
    id: "welcome-everyone-channel",
    name: "welcome-everyone",
    visibility: "open",
  });
  let ensureCalls = 0;

  const result = await ensureStarterChannels({
    getChannels: async () => [general, welcomeEveryone],
    ensureStarterChannels: async () => {
      ensureCalls += 1;
      return [];
    },
  });

  assert.equal(result.generalChannel, general);
  assert.equal(result.welcomeChannel, welcomeEveryone);
  assert.deepEqual(result.channels, [general, welcomeEveryone]);
  assert.equal(ensureCalls, 0);
});

test("ensureStarterChannels resumes when one starter channel is missing", async () => {
  const general = makeChannel({
    id: "general-channel",
    name: "general",
    visibility: "open",
  });
  const welcomeEveryone = makeChannel({
    id: "welcome-everyone-channel",
    name: "welcome-everyone",
    visibility: "open",
  });
  let ensureCalls = 0;

  const result = await ensureStarterChannels({
    getChannels: async () => [general],
    ensureStarterChannels: async () => {
      ensureCalls += 1;
      return [general, welcomeEveryone];
    },
  });

  assert.equal(result.generalChannel, general);
  assert.equal(result.welcomeChannel, welcomeEveryone);
  assert.deepEqual(result.channels, [general, welcomeEveryone]);
  assert.equal(ensureCalls, 1);
});

test("isWelcomeExperienceChannel matches legacy Welcome and starter welcome-everyone", () => {
  assert.equal(isWelcomeExperienceChannel(makeChannel()), true);
  assert.equal(
    isWelcomeExperienceChannel(
      makeChannel({ name: "welcome-everyone", visibility: "open" }),
    ),
    true,
  );
  assert.equal(
    isWelcomeExperienceChannel(
      makeChannel({ name: "Welcome-Everyone", visibility: "open" }),
    ),
    true,
  );
  assert.equal(
    isWelcomeExperienceChannel(
      makeChannel({ name: "welcome-everyone", visibility: "private" }),
    ),
    false,
  );
  assert.equal(
    isWelcomeExperienceChannel(
      makeChannel({ name: "general", visibility: "open" }),
    ),
    false,
  );
  assert.equal(isWelcomeExperienceChannel(null), false);
});
