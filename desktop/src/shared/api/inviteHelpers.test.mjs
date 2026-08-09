import assert from "node:assert/strict";
import test from "node:test";

import { inviteErrorMessage, isInviteExpiredError } from "./inviteHelpers.ts";

test("invite expiry sentinel is recognized without hiding other errors", () => {
  assert.equal(isInviteExpiredError(new Error("invite_expired")), true);
  assert.equal(isInviteExpiredError(new Error("invite_invalid")), false);
  assert.equal(
    inviteErrorMessage("network unavailable"),
    "network unavailable",
  );
});
