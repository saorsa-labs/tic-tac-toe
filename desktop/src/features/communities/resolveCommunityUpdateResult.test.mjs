import assert from "node:assert/strict";
import test from "node:test";

import { resolveCommunityUpdateResult } from "./useCommunities.tsx";

const COMMUNITIES = [
  {
    id: "group-a",
    groupId: "group-a",
    name: "Community A",
    addedAt: "2024-01-01",
    reposDir: "~/repos/a",
  },
  {
    id: "group-b",
    groupId: "group-b",
    name: "Community B",
    addedAt: "2024-01-02",
  },
];

test("untouched native community update is unchanged", () => {
  const result = resolveCommunityUpdateResult(
    COMMUNITIES,
    "group-a",
    "group-a",
    {
      name: "Community A",
      reposDir: "~/repos/a",
    },
  );
  assert.deepEqual(result, { kind: "unchanged" });
});

test("name edit updates without rebinding the native group", () => {
  const result = resolveCommunityUpdateResult(
    COMMUNITIES,
    "group-a",
    "group-a",
    {
      name: "New Name",
    },
  );
  assert.deepEqual(result, { kind: "updated", requiresReinit: false });
});

test("repository directory edit reinitializes workspace-local agent state", () => {
  const result = resolveCommunityUpdateResult(
    COMMUNITIES,
    "group-a",
    "group-a",
    {
      reposDir: "~/repos/new",
    },
  );
  assert.deepEqual(result, { kind: "updated", requiresReinit: true });
});

test("missing native community returns not-found", () => {
  const result = resolveCommunityUpdateResult(
    COMMUNITIES,
    "group-a",
    "missing-group",
    { name: "Whatever" },
  );
  assert.deepEqual(result, { kind: "not-found" });
});
