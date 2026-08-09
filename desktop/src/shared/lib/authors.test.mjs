import assert from "node:assert/strict";
import test from "node:test";

import { resolveEventAuthorPubkey } from "./authors.ts";

const SIGNER = "11".repeat(32);
const RELAY = "22".repeat(32);
const ATTRIBUTED_USER = "33".repeat(32);
const CHANNEL_ID = "36411e44-0e2d-4cfe-bd6e-567eb169db9f";

function resolve({
  signer = "user",
  tags,
  relaySelfPubkey = RELAY,
  preferActorTag = true,
}) {
  const event = {
    id: "44".repeat(32),
    pubkey: signer === "relay" ? RELAY : SIGNER,
    kind: 9,
    created_at: 1_700_000_000,
    content: "hello",
    tags,
    sig: "55".repeat(64),
  };

  return resolveEventAuthorPubkey({
    event,
    preferActorTag,
    relaySelfPubkey,
    requireChannelTagForPTags: true,
  });
}

test("user-signed actor tag cannot replace the visible author", () => {
  assert.equal(
    resolve({
      tags: [
        ["h", CHANNEL_ID],
        ["actor", ATTRIBUTED_USER],
      ],
    }),
    SIGNER,
  );
});

test("user-signed first p tag cannot replace the visible author", () => {
  assert.equal(
    resolve({
      tags: [
        ["p", ATTRIBUTED_USER],
        ["h", CHANNEL_ID],
      ],
    }),
    SIGNER,
  );
});

test("relay-signed actor tag resolves to the delegated author", () => {
  assert.equal(
    resolve({
      signer: "relay",
      tags: [
        ["h", CHANNEL_ID],
        ["actor", ATTRIBUTED_USER],
      ],
    }),
    ATTRIBUTED_USER,
  );
});

test("relay-signed author p tag resolves to the delegated author", () => {
  assert.equal(
    resolve({
      signer: "relay",
      preferActorTag: false,
      tags: [
        ["p", ATTRIBUTED_USER],
        ["h", CHANNEL_ID],
      ],
    }),
    ATTRIBUTED_USER,
  );
});

test("missing or malformed relay identity fails closed to the signer", () => {
  const tags = [
    ["h", CHANNEL_ID],
    ["actor", ATTRIBUTED_USER],
  ];

  assert.equal(resolve({ tags, relaySelfPubkey: null }), SIGNER);
  assert.equal(resolve({ tags, relaySelfPubkey: "not-a-pubkey" }), SIGNER);
});

test("malformed relay-signed attribution fails closed to the signer", () => {
  assert.equal(
    resolve({
      signer: "relay",
      tags: [
        ["h", CHANNEL_ID],
        ["actor", "not-a-pubkey"],
      ],
    }),
    RELAY,
  );
});

// Signature verification is enforced server-side by the Rust archive pipeline
// (archive/pipeline.rs::verify_signature at ingestion), not re-checked here.
// The client attribution layer therefore trusts the relay-self-signed event
// as-delivered; it cannot be reached with an invalid signature in production.
test("client does not re-verify signatures — attribution trusts the archive pipeline", () => {
  // A tampered event would be rejected by the archive pipeline before reaching
  // the frontend; resolveEventAuthorPubkey attributes purely on signer ===
  // relaySelf plus tag presence, matching that precondition.
  assert.equal(
    resolve({
      signer: "relay",
      tags: [
        ["h", CHANNEL_ID],
        ["actor", ATTRIBUTED_USER],
      ],
    }),
    ATTRIBUTED_USER,
  );
});
