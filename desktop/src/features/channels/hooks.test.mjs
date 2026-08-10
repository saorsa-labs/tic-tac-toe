import assert from "node:assert/strict";
import test from "node:test";

import {
  createChannelsQueryFn,
  createOpenDmCacheLifecycle,
  reconcileNativeChannelRefresh,
  reconcileRefreshedCachedChannel,
  upsertCachedChannel,
  upsertCachedChannelMember,
} from "./hooks.ts";

function makeChannel(
  id,
  name,
  channelType = "stream",
  { participantPubkeys = [], participants = [] } = {},
) {
  return {
    id,
    name,
    channelType,
    visibility: channelType === "dm" ? "private" : "open",
    description: "",
    topic: null,
    purpose: null,
    memberCount: participantPubkeys.length,
    memberPubkeys: [...participantPubkeys],
    lastMessageAt: null,
    archivedAt: null,
    participants,
    participantPubkeys,
    isMember: true,
    ttlSeconds: null,
    ttlDeadline: null,
  };
}

test("upsertCachedChannel_reseedsOpenedDmAfterStaleRefetch", () => {
  const staleChannels = [makeChannel("general", "General")];
  const openedDm = makeChannel("new-dm", "Alice", "dm");

  const repairedChannels = upsertCachedChannel(staleChannels, openedDm);

  assert.strictEqual(
    repairedChannels.find((channel) => channel.id === openedDm.id),
    openedDm,
    "the route must be able to resolve the exact relay-returned DM",
  );
});

test("upsertCachedChannel_replacesExistingChannelWithoutDuplicates", () => {
  const staleDm = makeChannel("new-dm", "Old name", "dm");
  const openedDm = makeChannel("new-dm", "Alice", "dm");

  const repairedChannels = upsertCachedChannel([staleDm], openedDm);

  assert.deepEqual(repairedChannels, [openedDm]);
});

test("upsertCachedChannelMember_doesNotDecorateImmutableDmSource", () => {
  const charliePubkey = "charlie-pubkey";
  const ownerPubkey = "owner-pubkey";
  const fizzPubkey = "fizz-pubkey";
  const openedDm = makeChannel("new-dm", "DM", "dm", {
    participantPubkeys: [charliePubkey, ownerPubkey],
    participants: ["charlie", "owner"],
  });

  const channels = upsertCachedChannelMember([openedDm], openedDm.id, {
    membershipAdded: true,
    name: "Fizz",
    pubkey: fizzPubkey,
  });
  assert.deepEqual(channels, [openedDm]);
});

test("upsertCachedChannelMember_recordsStreamMemberBeforeRefetch", () => {
  const fizzPubkey = "fizz-pubkey";
  const channel = makeChannel("general", "General");

  const channels = upsertCachedChannelMember([channel], channel.id, {
    membershipAdded: true,
    name: "Fizz",
    pubkey: fizzPubkey,
  });

  assert.deepEqual(channels?.[0].memberPubkeys, [fizzPubkey]);
  assert.equal(channels?.[0].memberCount, 1);
});

test("reconcileRefreshedCachedChannel_restoresOpenedDmAfterStaleRefresh", () => {
  const charliePubkey = "charlie-pubkey";
  const ownerPubkey = "owner-pubkey";
  const fizzPubkey = "fizz-pubkey";
  const openedDm = makeChannel("new-dm", "DM", "dm", {
    participantPubkeys: [charliePubkey, ownerPubkey],
    participants: ["charlie", "owner"],
  });
  const expandedDm = makeChannel("expanded-dm", "Group DM", "dm", {
    participantPubkeys: [charliePubkey, ownerPubkey, fizzPubkey],
    participants: ["charlie", "owner", "Fizz"],
  });

  const reconciled = reconcileRefreshedCachedChannel([openedDm], expandedDm);

  assert.deepEqual(reconciled[1].participantPubkeys, [
    charliePubkey,
    ownerPubkey,
    fizzPubkey,
  ]);
  assert.deepEqual(reconciled[0], openedDm);
});

test("reconcileRefreshedCachedChannel_preservesRefreshedDmRecency", () => {
  const charliePubkey = "charlie-pubkey";
  const ownerPubkey = "owner-pubkey";
  const openedDm = makeChannel("new-dm", "DM", "dm", {
    participantPubkeys: [charliePubkey, ownerPubkey],
    participants: ["charlie", "owner"],
  });
  const refreshedDm = {
    ...openedDm,
    lastMessageAt: "2026-07-14T11:21:26Z",
    name: "Group DM (3)",
  };

  const reconciled = reconcileRefreshedCachedChannel([refreshedDm], openedDm);

  assert.equal(reconciled[0].lastMessageAt, refreshedDm.lastMessageAt);
  assert.equal(reconciled[0].name, refreshedDm.name);
  assert.deepEqual(reconciled[0].participantPubkeys, [
    charliePubkey,
    ownerPubkey,
  ]);
});

test("profile Message keeps an empty native DM route resolvable after refresh", async () => {
  const peerAgentId = "ab".repeat(32);
  const openedDm = makeChannel(peerAgentId, "Direct message", "dm", {
    participantPubkeys: [peerAgentId],
    participants: ["Remote contact"],
  });
  const nativeChannelsWithoutHistory = [makeChannel("general", "General")];
  const calls = [];
  let cachedChannels;
  const queryClient = {
    async invalidateQueries() {
      calls.push("invalidate");
    },
    async refetchQueries() {
      calls.push("refresh");
      cachedChannels = nativeChannelsWithoutHistory;
    },
    setQueryData(_key, update) {
      calls.push("cache");
      cachedChannels = update(cachedChannels);
    },
  };
  const lifecycle = createOpenDmCacheLifecycle(queryClient);

  lifecycle.onSuccess(openedDm);
  await lifecycle.onSettled(openedDm);

  assert.deepEqual(calls, ["cache", "refresh", "cache"]);
  assert.strictEqual(
    cachedChannels.find((channel) => channel.id === peerAgentId),
    openedDm,
    "the #/channels/<AgentId> target must still resolve to its native DM composer",
  );
});

test("later live channel refresh retains the active native DM and drops stale streams", async () => {
  const peerAgentId = "cd".repeat(32);
  const activeDm = makeChannel(peerAgentId, "Remote contact", "dm", {
    participantPubkeys: [peerAgentId],
    participants: ["Remote contact"],
  });
  const staleStream = makeChannel("stale", "Removed stream");
  const refreshedStream = {
    ...makeChannel("general", "General"),
    lastMessageAt: "2026-08-10T15:12:00Z",
  };
  const cachedChannels = [activeDm, staleStream];
  const snapshots = [];
  const queryFn = createChannelsQueryFn(
    {
      getQueryData() {
        return cachedChannels;
      },
    },
    "community-a",
    async () => [refreshedStream],
    (groupId, channels) => snapshots.push({ groupId, channels }),
  );

  const refreshed = await queryFn();

  assert.strictEqual(
    refreshed.find((channel) => channel.id === peerAgentId),
    activeDm,
    "B's live reply refresh must not evict the AgentId DM route or sidebar row",
  );
  assert.strictEqual(
    refreshed.find((channel) => channel.id === refreshedStream.id),
    refreshedStream,
  );
  assert.equal(
    refreshed.some((channel) => channel.id === staleStream.id),
    false,
  );
  assert.deepEqual(snapshots, [
    { groupId: "community-a", channels: refreshed },
  ]);
});

test("native channel refresh preserves only cached DMs", () => {
  const dm = makeChannel("dm", "DM", "dm");
  const removedStream = makeChannel("removed", "Removed stream");

  assert.deepEqual(reconcileNativeChannelRefresh([dm, removedStream], []), [
    dm,
  ]);
});
