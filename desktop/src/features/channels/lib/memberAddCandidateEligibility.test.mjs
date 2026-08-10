import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

import { rankUserCandidatesBySearch } from "@/features/profile/lib/userCandidateSearch";
import {
  getEstablishedNativeContactPubkeys,
  isEligibleMemberAddCandidate,
  isEstablishedNativeContactCandidate,
} from "./memberAddCandidateEligibility.ts";

const UNKNOWN_ID = "11".repeat(32);
const BLOCKED_ID = "22".repeat(32);
const KNOWN_ID = "33".repeat(32);
const TRUSTED_ID = "44".repeat(32);
const MANAGED_ID = "55".repeat(32);

const contacts = [
  {
    agentId: UNKNOWN_ID,
    trustLevel: "unknown",
    label: "Unknown device",
    addedAt: 1,
    lastSeen: null,
  },
  {
    agentId: BLOCKED_ID,
    trustLevel: "blocked",
    label: "Blocked device",
    addedAt: 1,
    lastSeen: null,
  },
  {
    agentId: KNOWN_ID,
    trustLevel: "known",
    label: "Known laptop",
    addedAt: 1,
    lastSeen: null,
  },
  {
    agentId: TRUSTED_ID.toUpperCase(),
    trustLevel: "trusted",
    label: "Trusted laptop",
    addedAt: 1,
    lastSeen: null,
  },
];

function candidate(pubkey, displayName) {
  return {
    pubkey,
    displayName,
    avatarUrl: null,
    nip05Handle: null,
    ownerPubkey: null,
    isAgent: true,
  };
}

test("member add eligibility includes known/trusted native contacts and excludes unknown/blocked", () => {
  const established = getEstablishedNativeContactPubkeys(contacts);
  const managed = new Set([MANAGED_ID]);
  const candidates = [
    candidate(UNKNOWN_ID, "Unknown device"),
    candidate(BLOCKED_ID, "Blocked device"),
    candidate(KNOWN_ID, "Known laptop"),
    candidate(TRUSTED_ID, "Trusted laptop"),
    candidate(MANAGED_ID, "Managed agent"),
  ];

  assert.deepEqual(
    candidates
      .filter((entry) =>
        isEligibleMemberAddCandidate(entry, managed, established),
      )
      .map(({ pubkey }) => pubkey),
    [KNOWN_ID, TRUSTED_ID, MANAGED_ID],
  );
  assert.equal(
    isEstablishedNativeContactCandidate(
      { pubkey: BLOCKED_ID, isAgent: false },
      established,
    ),
    false,
    "a stale human-shaped result must not re-introduce a blocked native contact",
  );
});

test("member add search finds an established contact by exact AgentId", () => {
  const established = getEstablishedNativeContactPubkeys(contacts);
  const eligible = [
    candidate(UNKNOWN_ID, "Unknown device"),
    candidate(KNOWN_ID, "Known laptop"),
    candidate(TRUSTED_ID, "Trusted laptop"),
  ].filter((entry) =>
    isEligibleMemberAddCandidate(entry, new Set(), established),
  );

  assert.deepEqual(
    rankUserCandidatesBySearch({
      candidates: eligible,
      getLabel: (entry) => entry.displayName ?? entry.pubkey,
      limit: 50,
      query: TRUSTED_ID.toUpperCase(),
    }).map(({ pubkey }) => pubkey),
    [TRUSTED_ID],
  );
});

test("MembersSidebar applies native trust eligibility to its add candidates", async () => {
  const sidebar = await readFile(
    new URL("../ui/MembersSidebar.tsx", import.meta.url),
    "utf8",
  );

  assert.match(sidebar, /useNativeContactsQuery/);
  assert.match(sidebar, /isEstablishedNativeContactCandidate/);
  assert.match(sidebar, /isEligibleMemberAddCandidate/);
});
