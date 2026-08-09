import assert from "node:assert/strict";
import test from "node:test";

import {
  clearCommunityOnboardingTransaction,
  loadCommunityOnboardingTransaction,
  startCommunityOnboarding,
  updateCommunityOnboardingTransaction,
  updateCurrentCommunityOnboardingTransaction,
} from "./communityOnboarding.tsx";

const STORAGE_KEY = "buzz-community-onboarding-transaction.v2";

function createMemoryStorage(initial = {}) {
  const values = new Map(Object.entries(initial));
  return {
    getItem: (key) => values.get(key) ?? null,
    setItem: (key, value) => values.set(key, String(value)),
    removeItem: (key) => values.delete(key),
    clear: () => values.clear(),
    key: (index) => Array.from(values.keys())[index] ?? null,
    get length() {
      return values.size;
    },
  };
}

test("add-community onboarding starts a fresh transaction at connecting", () => {
  const storage = createMemoryStorage();
  const transaction = startCommunityOnboarding(
    {
      source: "add-community",
      communityName: "Research",
      groupId: "group:opaque/7",
    },
    storage,
    new Date("2026-07-16T00:00:00Z"),
  );

  assert.equal(transaction.stage, "connecting");
  assert.equal(transaction.groupId, "group:opaque/7");
  assert.equal(loadCommunityOnboardingTransaction(storage)?.id, transaction.id);
});

test("a different native group id replaces rather than resumes the transaction", () => {
  const storage = createMemoryStorage();
  const first = startCommunityOnboarding(
    {
      source: "first-community",
      communityName: "First",
      groupId: "group:opaque/1",
    },
    storage,
    new Date("2026-07-16T00:00:00Z"),
  );
  const progressed = updateCommunityOnboardingTransaction(
    first,
    { stage: "profile", communityId: "group:opaque/1" },
    storage,
    new Date("2026-07-16T00:01:00Z"),
  );

  const replacement = startCommunityOnboarding(
    {
      source: "add-community",
      communityName: "Second",
      groupId: "group:opaque/2",
    },
    storage,
    new Date("2026-07-16T00:02:00Z"),
  );

  assert.notEqual(replacement.id, progressed.id);
  assert.equal(replacement.stage, "connecting");
  assert.equal(replacement.groupId, "group:opaque/2");
  assert.equal(replacement.communityId, undefined);
  assert.equal(loadCommunityOnboardingTransaction(storage)?.id, replacement.id);
});

test("same native group id resumes rather than replacing progress", () => {
  const storage = createMemoryStorage();
  const first = startCommunityOnboarding(
    {
      source: "add-community",
      communityName: "Research",
      groupId: "group:opaque/7",
    },
    storage,
    new Date("2026-07-16T00:00:00Z"),
  );
  const progressed = updateCommunityOnboardingTransaction(
    first,
    { stage: "team-intro", communityId: "group:opaque/7" },
    storage,
    new Date("2026-07-16T00:01:00Z"),
  );
  const resumed = startCommunityOnboarding(
    {
      source: "add-community",
      communityName: "Renamed research",
      groupId: "group:opaque/7",
    },
    storage,
    new Date("2026-07-16T00:02:00Z"),
  );

  assert.equal(resumed.id, progressed.id);
  assert.equal(resumed.stage, "team-intro");
  assert.equal(resumed.communityId, "group:opaque/7");
  assert.equal(resumed.communityName, "Renamed research");
});

test("stale asynchronous updates cannot mutate a replacement transaction", () => {
  const storage = createMemoryStorage();
  const original = startCommunityOnboarding(
    {
      source: "add-community",
      communityName: "First",
      groupId: "group:opaque/1",
    },
    storage,
  );
  const replacement = startCommunityOnboarding(
    {
      source: "add-community",
      communityName: "Second",
      groupId: "group:opaque/2",
    },
    storage,
  );

  const result = updateCurrentCommunityOnboardingTransaction(
    replacement,
    { stage: "connecting", error: "stale error" },
    original.id,
    storage,
  );

  assert.equal(result, replacement);
  assert.equal(loadCommunityOnboardingTransaction(storage)?.id, replacement.id);
  assert.equal(loadCommunityOnboardingTransaction(storage)?.error, undefined);
});

test("acknowledgment persists but resets when the same group reopens", () => {
  const storage = createMemoryStorage();
  const transaction = startCommunityOnboarding(
    {
      source: "add-community",
      communityName: "Research",
      groupId: "group:opaque/7",
    },
    storage,
  );
  updateCommunityOnboardingTransaction(
    transaction,
    { acknowledged: true },
    storage,
  );
  assert.equal(loadCommunityOnboardingTransaction(storage)?.acknowledged, true);

  const reopened = startCommunityOnboarding(
    {
      source: "add-community",
      communityName: "Research",
      groupId: "group:opaque/7",
    },
    storage,
  );
  assert.equal(reopened.acknowledged, undefined);
});

test("stale invite-only persisted state is rejected and can be cleared", () => {
  // A persisted transaction from the removed invite-bootstrap shape carries the
  // old deep-link-join source and an invite link but no groupId, so the current
  // groupId-required validator must reject it as obsolete.
  const storage = createMemoryStorage({
    [STORAGE_KEY]: JSON.stringify({
      id: "legacy-invite",
      source: "deep-link-join",
      stage: "connecting",
      communityName: "Invited community",
      inviteLink: "x0x://invite/one-time-token",
      createdAt: "2026-01-01T00:00:00.000Z",
      updatedAt: "2026-01-01T00:00:00.000Z",
    }),
  });
  assert.equal(loadCommunityOnboardingTransaction(storage), null);

  // Malformed JSON or an object missing the required fields is also rejected.
  const malformed = createMemoryStorage({
    [STORAGE_KEY]: '{"stage":"profile"}',
  });
  assert.equal(loadCommunityOnboardingTransaction(malformed), null);

  clearCommunityOnboardingTransaction(storage);
  assert.equal(storage.length, 0);
});
