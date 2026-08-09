/**
 * Regression test for the lost-identity recovery CTA in `MachineOnboardingFlow`.
 *
 * ── Bug this pins ────────────────────────────────────────────────────────────
 * When the keyring signer was lost at boot (`identityLost`), the landing "Get
 * started" button still routed through `get_identity`. But `get_identity`
 * fail-closes when the daemon could not resolve an identity (that is exactly the
 * `lost` recovery state) — so the call rejected, the error was swallowed into
 * the inline `<p>`, and the button never advanced. The user was soft-bricked on
 * the landing screen with no way to invoke the recovery that clears `lost`.
 *
 * ── Fix under test ───────────────────────────────────────────────────────────
 * `loadFreshIdentity` now branches on `identityLost`: when true it invokes
 * `recoverLostIdentity` (the `recover_lost_identity` Tauri command) and returns
 * WITHOUT ever calling `get_identity`. First-run (identity present) still calls
 * `get_identity`.
 *
 * ── How this test proves it ──────────────────────────────────────────────────
 * We mount the REAL `MachineOnboardingFlow` against the Tauri IPC interceptor
 * (the same `window.__TAURI_INTERNALS__.invoke` recorder used by
 * `tauriIdentity.test.mjs`), then trigger the CTA and assert the EXACT command
 * surface hit:
 *   • lost mode  → ["recover_lost_identity"]  (get_identity never appears)
 *   • first run  → ["get_identity"]           (recover_lost_identity never appears)
 * An exact-array assertion catches a swapped branch, a removed guard, an
 * inverted condition, or a fallback that re-hits `get_identity` — any of which
 * resurrects the soft-brick.
 *
 * ── Why the handler is invoked directly ──────────────────────────────────────
 * React 19 dispatches click events through a single delegated listener on the
 * root container and relies on native-event bubbling to reach it. The repo's
 * unit-test DOM shim (no jsdom — see `MessageComposerDraftImagePersist.test.mjs`)
 * does not model event bubbling, so a synthetic click never reaches React's
 * delegate. We instead read the `onClick` React attached to the CTA `<button>`
 * (the host node's `__reactProps$…` expando — the same field React's own
 * test helpers read) and invoke it. This exercises the REAL wired handler
 * (`loadFreshIdentity`) end-to-end through the real `tauriIdentity` functions
 * into the mocked IPC transport.
 *
 * ── Why get_identity never settles ───────────────────────────────────────────
 * On the first-run path `loadFreshIdentity` calls `setPage("setup")` after
 * `getIdentity()` resolves, which renders `SetupStep` — a `useQuery` consumer
 * that needs a `QueryClientProvider` and fires its own runtime-catalog
 * commands. To observe the CTA's OWN command surface in isolation, the
 * `get_identity` handler returns a promise that never settles, freezing
 * `loadFreshIdentity` at `await getIdentity()` before the setup-page cascade
 * runs. (The command is still recorded once by the IPC interceptor at call
 * time.) The orphaned promise is abandoned on unmount and never rejects, so it
 * is not an unhandled rejection.
 *
 * ── CI surface ────────────────────────────────────────────────────────────────
 * Runs under `pnpm test` (node:test with the React dev build). Not Playwright.
 */
import assert from "node:assert/strict";
import { afterEach, describe, it } from "node:test";

// ── Tauri IPC interceptor ───────────────────────────────────────────────────
// @tauri-apps/api/core reads window.__TAURI_INTERNALS__.invoke(cmd, args).
// Alias window→globalThis so the alias resolves, then route by command name.
globalThis.window = globalThis;

/** Records every command invoked, in order — proves which surface was hit. */
const invokedCommands = [];
const ipcHandlers = new Map();

globalThis.__TAURI_INTERNALS__ = {
  invoke: (cmd, args) => {
    invokedCommands.push(cmd);
    const handler = ipcHandlers.get(cmd);
    if (handler) return Promise.resolve(handler(args));
    return Promise.reject(new Error(`unmocked Tauri command: ${cmd}`));
  },
  transformCallback: () => 0,
};

function setIpc(cmd, fn) {
  ipcHandlers.set(cmd, fn);
}

// ── Minimal DOM shim ─────────────────────────────────────────────────────────
// react-dom/client needs a subset of the DOM API to mount + commit. We provide
// exactly what MachineOnboardingFlow's identity-page render needs — buttons,
// divs, an <img>, the decorative LandingBees SVG, and the OnboardingFooter
// portal — without pulling in jsdom (not a project dep). This mirrors the shim
// in MessageComposerDraftImagePersist.test.mjs, extended with the attribute /
// SVG / text-node methods this heavier component touches.

function installDOMShim() {
  class MinimalEventTarget {
    constructor() {
      this._listeners = {};
    }
    addEventListener(type, fn) {
      this._listeners[type] = this._listeners[type] ?? [];
      this._listeners[type].push(fn);
    }
    removeEventListener(type, fn) {
      this._listeners[type] = (this._listeners[type] ?? []).filter(
        (f) => f !== fn,
      );
    }
    dispatchEvent(event) {
      for (const fn of this._listeners[event.type] ?? []) fn(event);
      return true;
    }
  }

  class MinimalNode extends MinimalEventTarget {
    constructor(tagName) {
      super();
      this.tagName = tagName;
      this.nodeName = tagName.toUpperCase();
      this.nodeType = 1;
      this.children = [];
      this.childNodes = [];
      this.style = {};
      this.parentNode = null;
      this._attrs = {};
      this.nodeValue = null;
      this.textContent = null;
    }

    get ownerDocument() {
      return globalThis.document;
    }
    get firstChild() {
      return this.children[0] ?? null;
    }
    get lastChild() {
      return this.children.at(-1) ?? null;
    }
    get nextSibling() {
      return null;
    }

    appendChild(child) {
      this.children.push(child);
      this.childNodes.push(child);
      child.parentNode = this;
      return child;
    }
    removeChild(child) {
      this.children = this.children.filter((c) => c !== child);
      this.childNodes = this.childNodes.filter((c) => c !== child);
      return child;
    }
    insertBefore(newNode, refNode) {
      if (!refNode) return this.appendChild(newNode);
      const i = this.children.indexOf(refNode);
      if (i < 0) return this.appendChild(newNode);
      this.children.splice(i, 0, newNode);
      this.childNodes.splice(i, 0, newNode);
      newNode.parentNode = this;
      return newNode;
    }
    contains(node) {
      if (!node) return false;
      return this === node || this.children.some((c) => c?.contains?.(node));
    }

    // React sets host attributes (data-testid, alt, type, viewBox, …) via
    // setAttribute; className mirrors `class`.
    setAttribute(name, value) {
      this._attrs[name] = `${value}`;
      if (name === "class") this.className = `${value}`;
    }
    removeAttribute(name) {
      delete this._attrs[name];
    }
    getAttribute(name) {
      return this._attrs[name] ?? null;
    }
  }

  class MinimalDocument extends MinimalEventTarget {
    constructor() {
      super();
      this.nodeType = 9;
    }
    createElement(tagName) {
      return new MinimalNode(tagName);
    }
    // LandingBees renders the BuzzMark/FlappingBee SVG; React creates those
    // host nodes through the namespaced factory.
    createElementNS(_ns, tagName) {
      return new MinimalNode(tagName);
    }
    createTextNode(value) {
      const node = new MinimalNode("#text");
      node.nodeType = 3;
      node.nodeValue = value;
      return node;
    }
    createComment(value) {
      const node = new MinimalNode("#comment");
      node.nodeType = 8;
      node.nodeValue = value;
      return node;
    }
    get body() {
      if (!this._body) this._body = this.createElement("body");
      return this._body;
    }
    get activeElement() {
      return null;
    }
    contains(node) {
      return node != null;
    }
  }

  globalThis.document = new MinimalDocument();
  // Referenced inside react-dom's focus/active-element helpers.
  globalThis.HTMLIFrameElement = MinimalNode;
  globalThis.HTMLElement = MinimalNode;
  // react uses IS_REACT_ACT_ENVIRONMENT to enable act() in non-browser envs.
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
  process.env.IS_REACT_ACT_ENVIRONMENT = "true";

  // `window` is already globalThis (set above for the IPC interceptor). Some
  // mounted effects call window.addEventListener directly (StartupWindowDrag
  // registers a drag handler); LandingBees' cleanup calls removeEventListener.
  // They are irrelevant to this contract, so accept and discard.
  globalThis.addEventListener = () => {};
  globalThis.removeEventListener = () => {};
  globalThis.dispatchEvent = () => true;

  if (!Object.getOwnPropertyDescriptor(globalThis, "navigator")?.value) {
    Object.defineProperty(globalThis, "navigator", {
      value: { userAgent: "node" },
      configurable: true,
    });
  }
  globalThis.MutationObserver = class {
    observe() {}
    disconnect() {}
    takeRecords() {
      return [];
    }
  };
  globalThis.requestAnimationFrame = (fn) => setTimeout(fn, 0);
  globalThis.cancelAnimationFrame = (id) => clearTimeout(id);
  // prefers-reduced-motion: reduce → LandingBees skips its rAF wander loop
  // (and the getBoundingClientRect / pointer tracking inside it) entirely.
  globalThis.window.matchMedia = (media) => ({
    matches: true,
    media,
    addEventListener() {},
    removeEventListener() {},
    addListener() {},
    removeListener() {},
  });
}

installDOMShim();

// ── Imports (after the shim + IPC interceptor are in place) ──────────────────
import React from "react";
import { createRoot } from "react-dom/client";
import { act } from "react";
import { QueryClient } from "@tanstack/react-query";

// Production component under test — owns the recover-vs-getIdentity branch.
import { MachineOnboardingFlow } from "./MachineOnboardingFlow.tsx";

afterEach(() => {
  ipcHandlers.clear();
  invokedCommands.length = 0;
});

// ── Mount + trigger helpers ──────────────────────────────────────────────────

/**
 * recover_lost_identity resolves void (the daemon restarts the app; the test
 * only cares that the command was invoked). get_identity returns a promise that
 * NEVER settles — see the file header for why this freezes the first-run path
 * before the setup-page cascade. Both commands are still recorded once each by
 * the IPC interceptor at call time.
 */
function installIdentityFixtures() {
  setIpc("recover_lost_identity", () => undefined);
  setIpc("get_identity", () => new Promise(() => {}));
}

/** Depth-first search for the first host node whose tag matches (case-insensitive). */
function findFirstByTag(root, tag) {
  const lower = tag.toLowerCase();
  function walk(node) {
    if (!node) return null;
    if (String(node.tagName).toLowerCase() === lower) return node;
    for (const child of node.children ?? []) {
      const hit = walk(child);
      if (hit) return hit;
    }
    return null;
  }
  return walk(root);
}

/**
 * React 19 attaches the props it renders a host node with to that node under a
 * `__reactProps$…` key (random suffix per root). Return the onClick React wired
 * to the CTA button — the real `loadFreshIdentity` handler.
 */
function readOnClick(buttonNode) {
  const propKey = Object.keys(buttonNode).find((k) =>
    k.startsWith("__reactProps"),
  );
  if (!propKey) {
    throw new Error(
      "CTA button has no __reactProps$ expando — React did not attach host props; " +
        "the click handler cannot be read. The DOM shim or React version may have changed.",
    );
  }
  const onClick = buttonNode[propKey]?.onClick;
  if (typeof onClick !== "function") {
    throw new Error(
      "CTA button rendered without an onClick handler — MachineOnboardingFlow's " +
        "Get-started wiring has changed.",
    );
  }
  return onClick;
}

/**
 * Mount MachineOnboardingFlow in the given recovery mode, trigger the landing
 * "Get started" CTA, and resolve once the async `loadFreshIdentity` has recorded
 * its command — i.e. once exactly one Tauri command has been recorded (or a
 * bounded number of microtask flushes have passed). Returns the recorded list.
 */
async function mountAndTriggerCta({ identityLost }) {
  installIdentityFixtures();
  const container = document.createElement("div");
  const root = createRoot(container);
  try {
    await act(async () => {
      root.render(
        React.createElement(MachineOnboardingFlow, {
          complete() {},
          identityLost,
          queryClient: new QueryClient(),
        }),
      );
    });

    const button = findFirstByTag(container, "button");
    assert.ok(button, "landing CTA button must render on the identity page");

    // The IPC handlers resolve on the next microtask; flush within act so the
    // recorded command (and React's pending state updates) settle before we read.
    await act(async () => {
      readOnClick(button)();
      for (let i = 0; i < 50 && invokedCommands.length === 0; i++) {
        await new Promise((resolve) => setTimeout(resolve, 0));
      }
    });

    return [...invokedCommands];
  } finally {
    await act(async () => {
      root.unmount();
    });
  }
}

// ── Tests ────────────────────────────────────────────────────────────────────

describe("MachineOnboardingFlow: lost-mode CTA routes to recovery, never get_identity", () => {
  it("lost mode: Get started invokes recover_lost_identity and never get_identity", async () => {
    const commands = await mountAndTriggerCta({ identityLost: true });

    // Exact-surface assertion: only the recovery command is hit. This fails if
    // the branch is removed/inverted/swapped, OR if a fallback re-hits
    // get_identity (the original soft-brick).
    assert.deepEqual(commands, ["recover_lost_identity"]);
    assert.ok(
      !commands.includes("get_identity"),
      "lost mode must NEVER call get_identity — it fail-closes without a resolved identity",
    );
  });

  it("first run (identity present): Get started invokes get_identity, never recover_lost_identity", async () => {
    const commands = await mountAndTriggerCta({ identityLost: false });

    assert.deepEqual(commands, ["get_identity"]);
    assert.ok(
      !commands.includes("recover_lost_identity"),
      "normal first-run must not invoke the destructive recovery path",
    );
  });
});
