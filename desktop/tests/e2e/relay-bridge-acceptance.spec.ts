import { expect, test } from "@playwright/test";

import { installRelayBridge, TEST_IDENTITIES } from "../helpers/bridge";
import { assertRelaySeeded } from "../helpers/seed";
import { _internal } from "../helpers/seedRelay";

// =============================================================================
// M1a relay-mode acceptance — real x0x-nostr-bridge, no mock IPC.
// =============================================================================
//
// Covers the two relay-only surfaces the desktop leans on the bridge for, end
// to end through the public UI. Events are published through the REAL ingest
// path (POST /events) via the seedRelay signer/builders — never synthesized by
// the in-page mock bridge — so neither test can pass against mock IPC.
//
//   1. Live thread summaries. A NIP-10 root plus a marked reply must (a) land
//      the reply in the thread subtree and (b) drive the thread-summary reply
//      count via the kind-39005 WS summary push. A bridge missing the WS
//      summary broadcast leaves the count frozen at its page snapshot, so the
//      live "2 replies" assertion REDDENS — the gate this spec exists to hold.
//
//   2. Content search. A uniquely-tokened message must be findable through the
//      desktop global search (search_messages → bridge FTS5) and selecting the
//      hit must deep-link into the channel at that message. A bridge without
//      message search surfaces "No matches" and the hit never renders → REDDENS.
//
// Both tests are gated by assertRelaySeeded() (beforeAll) and route through
// installRelayBridge (relay mode). `general` is a shared channel that
// accumulates rows across runs, so every published message carries a per-run
// nonce and is located by its signed event id — never by positional or count
// assumptions about the shared seed.

const isCi = Boolean(process.env.CI);
const relaySeedHookTimeoutMs = isCi ? 90_000 : 30_000;

const RELAY_HTTP = process.env.BUZZ_E2E_RELAY_URL ?? "http://localhost:3000";

// uuid5(NAMESPACE_DNS, "buzz.channel.general") — the `general` channel on the
// real bridge demo seed (the same id parity-ancestor-island.spec.ts pins to).
// The mock bridge seeds a different general id; this spec never runs mock.
const GENERAL_CHANNEL_ID = "9f28288a-d724-587a-9709-92dc7f967110";

const { buildMessage, directReplyTags, signerFor, publishGroups } = _internal;

/**
 * A per-run lowercase-alphanumeric token. FTS5's unicode61 tokenizer treats it
 * as a single term, so searching the exact token is a deterministic match and a
 * clean probe for relay content search (a missing-search bridge returns none).
 */
function uniqueToken(label: string) {
  return `${label}${Date.now().toString(36)}${Math.random().toString(36).slice(2, 8)}`;
}

test.beforeAll(async () => {
  test.setTimeout(relaySeedHookTimeoutMs);
  await assertRelaySeeded();
});

// ---------------------------------------------------------------------------
// 1. NIP-10 thread: subtree + live summary count
// ---------------------------------------------------------------------------

test("relay bridge: a marked NIP-10 reply renders in the thread subtree and advances the live summary count", async ({
  page,
}) => {
  test.setTimeout(60_000);

  const token = uniqueToken("m1athread");
  const rootContent = `${token} root`;
  const replyOneContent = `${token} reply one`;
  const replyTwoContent = `${token} reply two`;

  const tyler = signerFor("tyler");
  const alice = signerFor("alice");
  const baseTime = Math.floor(Date.now() / 1000);

  // Root: top-level kind-9 (no e-tag → depth NULL). Reply: a MARKED NIP-10
  // direct reply — ["e", root, "", "reply"] — whose root resolves to its parent.
  const rootEvent = buildMessage(
    tyler,
    GENERAL_CHANNEL_ID,
    rootContent,
    baseTime,
  );
  const replyOne = buildMessage(
    alice,
    GENERAL_CHANNEL_ID,
    replyOneContent,
    baseTime + 1,
    directReplyTags(rootEvent.id),
  );

  // Essential signed-event metadata: the events we publish are well-formed
  // NIP-10 — kind 9, authored by the signing identity, and the reply carries
  // the "reply" marker bound to the root id (not a bare/legacy e-tag).
  expect(rootEvent.kind).toBe(9);
  expect(rootEvent.pubkey).toBe(TEST_IDENTITIES.tyler.pubkey);
  expect(replyOne.pubkey).toBe(TEST_IDENTITIES.alice.pubkey);
  expect(replyOne.tags).toContainEqual(["e", rootEvent.id, "", "reply"]);

  // Publish through the real ingest path (POST /events); parent before child.
  await publishGroups([[rootEvent], [replyOne]], RELAY_HTTP, 1);

  await installRelayBridge(page, "tyler");
  // Deep-link straight to the root message. The messageId route forces the
  // virtualizer to mount the row even though the shared `general` channel
  // accumulates rows across runs — no positional assumption about the window.
  await page.goto(
    `/#/channels/${GENERAL_CHANNEL_ID}?messageId=${rootEvent.id}`,
  );
  await expect(page.getByTestId("chat-title")).toHaveText("general");
  // The messageId deep-link opens the thread panel ONTO the root (the route
  // target handler sets openThreadHeadId = messageId), so the root then renders
  // as BOTH the timeline row and the panel head. That made the unscoped
  // locator match two elements, and a later summary click would TOGGLE an
  // already-open panel closed instead of opening it. Dismiss the pre-open
  // panel so every assertion below targets exactly one element in the timeline.
  const timeline = page.getByTestId("message-timeline");
  const threadPanel = page.getByTestId("message-thread-panel");
  await threadPanel
    .waitFor({ state: "visible", timeout: 5_000 })
    .catch(() => {});
  if (await threadPanel.isVisible()) {
    await page.getByTestId("auxiliary-panel-close").click();
    await expect(threadPanel).not.toBeVisible();
  }

  // The signed event id IS the cryptographic binding between what we published
  // and what rendered: data-message-id is the relay-stored event id.
  const rootRow = timeline.locator(
    `[data-testid="message-row"][data-message-id="${rootEvent.id}"]`,
  );
  await expect(rootRow).toBeVisible();
  await expect(rootRow).toContainText(rootContent);

  // Ingest computes thread_metadata at POST /events time; the channel-window
  // snapshot attaches that summary to THIS root. Scope by data-thread-head-id
  // so unrelated threads in `general` can never mask the assertion.
  const summary = timeline.locator(
    `[data-testid="message-thread-summary"][data-thread-head-id="${rootEvent.id}"]`,
  );
  await expect(summary).toBeVisible();
  await expect(summary).toContainText("1 reply");

  // Open the thread panel: the marked reply must render in the subtree under
  // the root, carrying the reply's own signed event id.
  await summary.click();
  await expect(page.getByTestId("message-thread-panel")).toBeVisible();
  const replies = page.getByTestId("message-thread-replies");
  await expect(
    replies.locator(`[data-message-id="${replyOne.id}"]`),
  ).toBeVisible();
  await expect(replies).toContainText(replyOneContent);
  // Close back to the bare timeline so the live-count assertion targets the
  // timeline badge, independent of the open auxiliary panel.
  await page.getByTestId("auxiliary-panel-close").click();
  await expect(page.getByTestId("message-thread-panel")).not.toBeVisible();

  // LIVE UPDATE: publish a second marked reply through the real bridge while
  // the channel is open. The kind-39005 WS summary push MUST advance the badge
  // — this is the WS-summary support the gate defends. A bridge that doesn't
  // broadcast live summaries leaves the count stuck at "1 reply" → timeout.
  const replyTwo = buildMessage(
    alice,
    GENERAL_CHANNEL_ID,
    replyTwoContent,
    baseTime + 2,
    directReplyTags(rootEvent.id),
  );
  expect(replyTwo.tags).toContainEqual(["e", rootEvent.id, "", "reply"]);
  await publishGroups([[replyTwo]], RELAY_HTTP, 1);

  await expect(summary).toContainText("2 replies");

  // The new reply is delivered live into the subtree as well.
  await summary.click();
  await expect(page.getByTestId("message-thread-panel")).toBeVisible();
  await expect(
    replies.locator(`[data-message-id="${replyTwo.id}"]`),
  ).toBeVisible();
  await expect(replies).toContainText(replyTwoContent);
});

// ---------------------------------------------------------------------------
// 2. Prefix search: result renders + deep-links into the channel
// ---------------------------------------------------------------------------

test("relay bridge: a uniquely-tokened message is findable via search and the hit deep-links into the channel", async ({
  page,
}) => {
  test.setTimeout(60_000);

  const token = uniqueToken("m1asearch");
  const content = `${token} relay acceptance search probe`;

  const tyler = signerFor("tyler");
  const searchEvent = buildMessage(
    tyler,
    GENERAL_CHANNEL_ID,
    content,
    Math.floor(Date.now() / 1000),
  );
  // Essential signed-event metadata: a kind-9 message authored by the signer.
  expect(searchEvent.kind).toBe(9);
  expect(searchEvent.pubkey).toBe(TEST_IDENTITIES.tyler.pubkey);

  await publishGroups([[searchEvent]], RELAY_HTTP, 1);

  await installRelayBridge(page, "tyler");
  await page.goto("/");
  await expect(page.getByTestId("app-sidebar")).toBeVisible();

  await page.getByTestId("open-search").click();
  const searchInput = page.getByTestId("search-dialog-input");
  await expect(searchInput).toBeVisible();
  // The token is one FTS5 term; the bridge's search_messages must return it.
  await searchInput.fill(token);

  const results = page.getByTestId("search-results");
  const hit = results.getByTestId(`search-result-${searchEvent.id}`);
  await expect(hit).toBeVisible();
  await expect(results).toContainText(content);

  // Selecting the hit resolves the destination and deep-links into general at
  // the message. A search backend that omits channelId leaves navigation a
  // no-op and this URL never advances → REDDENS.
  await hit.click();
  await expect(page).toHaveURL(
    new RegExp(`#/channels/${GENERAL_CHANNEL_ID}.*messageId=${searchEvent.id}`),
  );
  await expect(page.getByTestId("chat-title")).toHaveText("general");
  await expect(page.getByTestId("message-timeline")).toContainText(content);
});
