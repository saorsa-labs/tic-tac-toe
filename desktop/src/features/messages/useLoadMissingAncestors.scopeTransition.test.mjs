/**
 * useLoadMissingAncestors scope-hold + scope-transition retry contract.
 *
 * A group's missing thread ancestors are loaded against the daemon-resolved
 * durable-history scope. While that scope is unresolved the loader HOLDS — it
 * fetches nothing and, crucially, does NOT mark the ids requested (which would
 * poison the dedup set and silently swallow them on a later scope). When the
 * scope arrives the ancestors fetch.
 *
 * The residual fix under test: the dedup set (`requestedAncestorIdsRef`) is
 * cleared on every resolved-scope IDENTITY change — not only a channel change.
 * So an ancestor already requested under scope A is re-requested when the scope
 * rotates to B (null->value or A->B), instead of being suppressed as
 * "already requested" against the wrong scope.
 *
 * Rendered with the REAL production hook over a real QueryClientProvider
 * (useQueryClient + effects only — no useQuery, which is what made the sibling
 * render harness hang). The only intercepted boundary is the Tauri transport.
 */
import assert from "node:assert/strict";
import test, { beforeEach, afterEach } from "node:test";

// ── Minimal DOM shim (createRoot + commit subset; no jsdom) ─────────────────
function installDOMShim() {
  class MinimalEventTarget {
    constructor() {
      this._l = {};
    }
    addEventListener(t, f) {
      if (!this._l[t]) {
        this._l[t] = [];
      }
      this._l[t].push(f);
    }
    removeEventListener(t, f) {
      this._l[t] = (this._l[t] ?? []).filter((x) => x !== f);
    }
    dispatchEvent(e) {
      for (const fn of this._l[e.type] ?? []) fn(e);
      return true;
    }
  }
  class MinimalNode extends MinimalEventTarget {
    constructor(tag) {
      super();
      this.tagName = tag;
      this.children = [];
      this.childNodes = [];
      this.style = {};
      this.nodeType = 1;
      this.parentNode = null;
      this.nodeValue = null;
    }
    get firstChild() {
      return this.children[0] ?? null;
    }
    get lastChild() {
      return this.children[this.children.length - 1] ?? null;
    }
    get nextSibling() {
      return null;
    }
    get ownerDocument() {
      return globalThis.document;
    }
    appendChild(c) {
      this.children.push(c);
      this.childNodes.push(c);
      c.parentNode = this;
      return c;
    }
    removeChild(c) {
      this.children = this.children.filter((x) => x !== c);
      this.childNodes = this.childNodes.filter((x) => x !== c);
      return c;
    }
    insertBefore(n, ref) {
      if (!ref) return this.appendChild(n);
      const i = this.children.indexOf(ref);
      if (i < 0) return this.appendChild(n);
      this.children.splice(i, 0, n);
      this.childNodes.splice(i, 0, n);
      n.parentNode = this;
      return n;
    }
    contains(n) {
      if (!n) return false;
      return this === n || this.children.some((c) => c?.contains?.(n));
    }
  }
  class MinimalDocument extends MinimalEventTarget {
    constructor() {
      super();
      this.nodeType = 9;
    }
    createElement(t) {
      return new MinimalNode(t);
    }
    createTextNode(v) {
      const n = new MinimalNode("#text");
      n.nodeValue = v;
      n.nodeType = 3;
      return n;
    }
    get body() {
      if (!this._b) {
        this._b = this.createElement("body");
      }
      return this._b;
    }
    get activeElement() {
      return null;
    }
    contains(n) {
      return n != null;
    }
  }
  globalThis.document = new MinimalDocument();
  globalThis.HTMLIFrameElement = MinimalNode;
  globalThis.HTMLElement = MinimalNode;
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
  process.env.IS_REACT_ACT_ENVIRONMENT = "true";
  if (typeof globalThis.window === "undefined") {
    Object.defineProperty(globalThis, "window", {
      value: globalThis,
      configurable: true,
    });
  }
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
}
installDOMShim();

const ANCESTOR = "a".repeat(64);

// ── Tauri transport shim ────────────────────────────────────────────────────
const calls = [];
globalThis.window.__TAURI_INTERNALS__ = {
  invoke: async (cmd, args) => {
    calls.push({ cmd, args });
    // x0x_history_get: the ancestor is absent from the local store (404 → null),
    // so it stays "missing" and remains a retry candidate across scope changes.
    if (cmd === "x0x_history_get") return null;
    return null;
  },
  transformCallback: () => 1,
  unregisterCallback: () => {},
};

import React from "react";
import { createRoot } from "react-dom/client";
import { act } from "react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

import { useLoadMissingAncestors } from "./useLoadMissingAncestors.ts";
import {
  setResolvedHistoryScope,
  clearAllResolvedHistoryScopes,
} from "@/features/messages/lib/nativeHistoryScopeStore.ts";

function groupChannel(id) {
  return {
    id,
    name: "General",
    channelType: "stream",
    visibility: "private",
    description: "",
    topic: null,
    purpose: null,
    memberCount: 0,
    memberPubkeys: [],
    lastMessageAt: null,
    archivedAt: null,
    participants: [],
    participantPubkeys: [],
    isMember: true,
    ttlSeconds: null,
    ttlDeadline: null,
  };
}

function messageWithAncestor(id) {
  return {
    id,
    pubkey: "b".repeat(64),
    kind: 9,
    created_at: 1_700_000_000,
    content: "reply",
    tags: [["e", ANCESTOR, "", "reply"]],
    sig: "",
  };
}

function countAncestorLookups() {
  return calls.filter(
    (c) => c.cmd === "x0x_history_get" && c.args.msgId === ANCESTOR,
  ).length;
}

function mountLoader(channel, messages) {
  function Probe() {
    useLoadMissingAncestors(channel, messages);
    return null;
  }
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false, gcTime: 0, staleTime: 0 } },
  });
  const container = document.createElement("div");
  const root = createRoot(container);
  const render = () =>
    act(async () => {
      root.render(
        React.createElement(
          QueryClientProvider,
          { client: queryClient },
          React.createElement(Probe),
        ),
      );
    });
  return { render, root };
}

const tick = () => new Promise((r) => setTimeout(r, 0));

// Flush pending effect/fetch work inside act so async `.then` state updates
// (setQueryData) settle within the act boundary (no stray act warnings).
const flush = async () => {
  await act(async () => {
    await tick();
    await tick();
  });
};

// Apply a scope change AND flush the resulting re-render + fetch inside one act.
const setScope = async (channelId, scope) => {
  await act(async () => {
    setResolvedHistoryScope(channelId, scope);
    await tick();
    await tick();
  });
};

beforeEach(() => {
  calls.length = 0;
  clearAllResolvedHistoryScopes();
});
afterEach(() => {
  clearAllResolvedHistoryScopes();
});

test("an unresolved group holds: no ancestor lookup fires and the dedup set is not poisoned", async () => {
  const channel = groupChannel("g-hold");
  const { render } = mountLoader(channel, [messageWithAncestor("child-1")]);
  await render();
  await flush();

  assert.equal(
    countAncestorLookups(),
    0,
    "no lookup while the scope is unresolved",
  );
});

test("scope arrival (null -> value) requests the missing ancestor against the resolved scope", async () => {
  const channel = groupChannel("g-arrive");
  const { render } = mountLoader(channel, [messageWithAncestor("child-1")]);
  await render();
  assert.equal(countAncestorLookups(), 0, "held before the scope arrives");

  await setScope("g-arrive", "group:scope-a");

  assert.equal(
    countAncestorLookups(),
    1,
    "the ancestor is requested once the scope resolves",
  );
});

test("scope rotation (A -> B) clears the dedup set so a prior-scope ancestor retries under the new scope", async () => {
  const channel = groupChannel("g-rotate");
  const { render } = mountLoader(channel, [messageWithAncestor("child-1")]);
  await render();

  // Scope A arrives → ancestor requested once and marked in the dedup set.
  await setScope("g-rotate", "group:scope-a");
  assert.equal(countAncestorLookups(), 1, "requested under scope A");

  // A second render under the SAME scope must NOT re-request (dedup holds).
  await flush();
  assert.equal(countAncestorLookups(), 1, "same scope does not re-request");

  // Scope rotates to B → the dedup set is cleared → the ancestor retries.
  await setScope("g-rotate", "group:scope-b");
  assert.equal(
    countAncestorLookups(),
    2,
    "prior-scope ancestor is re-requested after the scope rotates",
  );
});
