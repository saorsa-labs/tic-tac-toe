import assert from "node:assert/strict";
import test from "node:test";

import { resolveManagedAgentAvatarUrl } from "./managedAgentAvatar.ts";

test("resolveManagedAgentAvatarUrl trims and returns hosted URLs", async () => {
  assert.equal(
    await resolveManagedAgentAvatarUrl(
      " https://relay.example/already-hosted.png ",
    ),
    "https://relay.example/already-hosted.png",
  );
});

test("resolveManagedAgentAvatarUrl passes emoji svg data URLs through", async () => {
  const emojiUrl =
    "data:image/svg+xml,%3Csvg%20xmlns%3D%22http%3A%2F%2Fwww.w3.org%2F2000%2Fsvg%22%3E%3C%2Fsvg%3E";
  assert.equal(await resolveManagedAgentAvatarUrl(emojiUrl), emojiUrl);
});

test("resolveManagedAgentAvatarUrl passes image data URLs through", async () => {
  const dataUrl = "data:image/png;base64,aGVsbG8=";
  assert.equal(await resolveManagedAgentAvatarUrl(dataUrl), dataUrl);
});

test("resolveManagedAgentAvatarUrl returns undefined for empty input", async () => {
  assert.equal(await resolveManagedAgentAvatarUrl(undefined), undefined);
  assert.equal(await resolveManagedAgentAvatarUrl(null), undefined);
  assert.equal(await resolveManagedAgentAvatarUrl(""), undefined);
  assert.equal(await resolveManagedAgentAvatarUrl("   "), undefined);
});
