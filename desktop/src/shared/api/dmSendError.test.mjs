import assert from "node:assert/strict";
import test from "node:test";

import { formatDmSendError } from "./dmSendError.ts";

test("the two 409s prescribe opposite repairs", () => {
  const upgrade = formatDmSendError(
    'x0xd returned HTTP 409: /direct/send: {"error":"recipient_ack_semantics_unavailable"}',
  );
  const conflict = formatDmSendError(
    'x0xd returned HTTP 409: /direct/send: {"error":"idempotency_conflict"}',
  );
  assert.match(upgrade, /upgrading/);
  assert.match(conflict, /Retrying won't help/);
  assert.notEqual(upgrade, conflict);
});

test("a 504 tells the user to retry the same id, not mint a new one", () => {
  const message = formatDmSendError(
    'x0xd returned HTTP 504: /direct/send: {"error":"timeout"}',
  );
  assert.match(message, /same id/);
});

test("already-mapped product copy is left alone", () => {
  const copy = "Peer needs upgrading — it can't confirm durable delivery yet.";
  assert.equal(formatDmSendError(copy), copy);
});

test("unknown errors keep their text", () => {
  assert.equal(formatDmSendError("composer empty"), "composer empty");
});
