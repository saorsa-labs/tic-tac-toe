import assert from "node:assert/strict";
import test from "node:test";

import { getAgentMemory } from "./tauriEngrams.ts";

test("agent memory fails explicitly instead of invoking the relay fallback", async () => {
  await assert.rejects(
    getAgentMemory("legacy-relay-pubkey"),
    /encrypted owner-scoped engram store.*relay fallback is disabled/i,
  );
});
