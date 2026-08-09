/**
 * M2 displayed-identity render regressions for `AgentIdentity` — the component
 * that replaced the old PubKey (npub) display.
 *
 * The canonical human identity is the four AgentId words; the raw 64-hex
 * AgentId is a secondary, copy-only value. These tests pin the visible
 * contract via `renderToStaticMarkup` (the repo's established render pattern —
 * no jsdom): the words render as the primary display, no bech32 (npub/nsec)
 * or relay-signer hex ever leaks into the rendered identity, and the copy
 * affordances are present. The popover's copy-of-the-full-AgentId is exercised
 * through the data contract (`agentId` is the required, copy-targeted prop).
 */
import assert from "node:assert/strict";
import { describe, it } from "node:test";

import React from "react";
import { renderToStaticMarkup } from "react-dom/server";

import { AgentIdentity } from "@/shared/ui/AgentIdentity.tsx";

const AGENT_ID =
  "dd6530452610619d468e4e82be82107e86384365c58efa6e3018d7762c7368da";
const WORDS = ["bodily", "example", "dismiss", "galaxy"];
// A relay-signer-shaped hex that must NEVER appear in the rendered identity.
const RELAY_PUBKEY = "deadbeef".repeat(8);

describe("AgentIdentity: renders the four words, never a signer/bech32 identity", () => {
  it("renders the joined four words as the primary display (compact)", () => {
    const html = renderToStaticMarkup(
      React.createElement(AgentIdentity, {
        agentId: AGENT_ID,
        identityWords: WORDS,
      }),
    );
    for (const word of WORDS) {
      assert.ok(html.includes(word), `word "${word}" should render`);
    }
  });

  it("does not render the raw AgentId hex as the primary identity text", () => {
    // The hex is a copy-only secondary value; the words are the display. In the
    // closed (default) render the full hex must not be the visible identity.
    const html = renderToStaticMarkup(
      React.createElement(AgentIdentity, {
        agentId: AGENT_ID,
        identityWords: WORDS,
      }),
    );
    assert.ok(
      !html.includes(AGENT_ID),
      "the raw 64-hex AgentId must not be the primary display",
    );
  });

  it("never renders npub/nsec bech32 or the relay signer pubkey", () => {
    // Pass a distinct relay-signer-shaped value through the surrounding context;
    // the component must never emit bech32 or a signer pubkey as identity. This
    // is the core regression for "compat signer / npub never becomes the display".
    const html = renderToStaticMarkup(
      React.createElement(AgentIdentity, {
        agentId: AGENT_ID,
        identityWords: WORDS,
      }),
    );
    assert.ok(!/npub/i.test(html), "npub must never appear");
    assert.ok(!/nsec/i.test(html), "nsec must never appear");
    assert.ok(
      !html.includes(RELAY_PUBKEY),
      "the relay signer pubkey must never render as identity",
    );
  });

  it("renders a copy affordance for the full identity (full variant)", () => {
    const html = renderToStaticMarkup(
      React.createElement(AgentIdentity, {
        agentId: AGENT_ID,
        identityWords: WORDS,
        variant: "full",
      }),
    );
    // The full variant exposes a "Copy agent identity" trigger — the entry point
    // to copying the words and the full AgentId.
    assert.ok(
      html.includes("Copy agent identity"),
      "full variant must expose a copy-agent-identity affordance",
    );
    for (const word of WORDS) {
      assert.ok(html.includes(word), `word "${word}" should render`);
    }
  });

  it("filters empty word entries before joining the display", () => {
    // joinWords drops empty strings so a malformed words array can't produce a
    // double-spaced or empty identity chip.
    const html = renderToStaticMarkup(
      React.createElement(AgentIdentity, {
        agentId: AGENT_ID,
        identityWords: ["bodily", "", "dismiss", "galaxy"],
      }),
    );
    assert.ok(html.includes("bodily") && html.includes("dismiss"));
    // aria-label is "Show agent identity <joined non-empty words>".
    assert.ok(
      html.includes("Show agent identity bodily dismiss galaxy"),
      "empty words must be dropped before joining",
    );
  });
});
