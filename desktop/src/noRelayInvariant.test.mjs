import assert from "node:assert/strict";
import { test } from "node:test";

import { findNoRelayViolations } from "../../scripts/no-relay-gate.mjs";

test("packaged native application has no compatibility relay transport", async () => {
  const violations = await findNoRelayViolations();
  assert.deepEqual(violations, []);
});
