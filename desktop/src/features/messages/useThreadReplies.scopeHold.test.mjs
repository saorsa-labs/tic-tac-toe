/**
 * useThreadReplies scope-hold + requery-on-arrival contract.
 *
 * A group thread is HELD (its useQuery stays disabled / idle) while the
 * channel's durable-history scope is unresolved, so no fetch fires and no error
 * flickers against an unresolved scope. The moment the daemon-resolved scope
 * arrives (the live subscription populating the registry), the hook's
 * useSyncExternalStore re-renders, the scope-aware key recomputes
 * (threadRepliesKey carries the scope — see messageQueryKeys.test.mjs), the
 * query enables, and the thread subtree fetches against the resolved scope. DM
 * threads are deterministic and never hold.
 *
 * Rendered with the REAL production hook over a real QueryClientProvider. The
 * QueryClient uses stable cache settings (staleTime/gcTime Infinity,
 * refetchOnMount false) so useQuery settles deterministically under act without
 * refetch churn. Only the Tauri transport is intercepted.
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

const ROOT_MSG_ID = "1".repeat(64);
const REPLY_MSG_ID = "2".repeat(64);

// ── Tauri transport shim ────────────────────────────────────────────────────
const calls = [];
globalThis.window.__TAURI_INTERNALS__ = {
  invoke: async (cmd, args) => {
    calls.push({ cmd, args });
    if (cmd === "x0x_history_list") {
      return {
        rows: [
          {
            id: 11,
            msg_id: REPLY_MSG_ID,
            scope: args.scope,
            author_agent: "b".repeat(64),
            author_machine: null,
            sent_at_ms: 1_700_000_000_000,
            seen_at_ms: 1_700_000_001_000,
            direction: "Inbound",
            content_type: "text/plain",
            payload: btoa(
              JSON.stringify({
                text: "reply",
                clientId: "c-r",
                createdAt: 1_700_000_000_000,
              }),
            ),
            signed: true,
            provenance: "VerifiedEnvelope",
            replace_key: null,
            thread_root: ROOT_MSG_ID,
            thread_parent: ROOT_MSG_ID,
          },
        ],
        has_more: false,
      };
    }
    return null;
  },
  transformCallback: () => 1,
  unregisterCallback: () => {},
};

import React from "react";
import { createRoot } from "react-dom/client";
import { act } from "react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

import { useThreadReplies } from "./useThreadReplies.ts";
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
function dmChannel() {
  const peer = "c".repeat(64);
  return { ...groupChannel(peer), id: peer, channelType: "dm" };
}

let activeRoot = null;
let activeClient = null;
function mountThread(channel, rootId) {
  const ref = { current: { fetchStatus: "idle", status: "pending" } };
  function Probe() {
    ref.current = useThreadReplies(channel, rootId);
    return null;
  }
  // Stable cache settings so useQuery settles under act without refetch churn.
  const queryClient = new QueryClient({
    defaultOptions: {
      queries: {
        retry: false,
        networkMode: "always",
        staleTime: Infinity,
        gcTime: Infinity,
        refetchOnMount: false,
        refetchOnWindowFocus: false,
        refetchOnReconnect: false,
      },
    },
  });
  const container = document.createElement("div");
  const root = createRoot(container);
  activeRoot = root;
  activeClient = queryClient;
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
  return { ref, render, queryClient, root };
}

const tick = () => new Promise((r) => setTimeout(r, 0));
const flush = async () => {
  await act(async () => {
    await tick();
    await tick();
  });
};
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
afterEach(async () => {
  // Tear down the rendered root + QueryClient so no query/timer handle keeps
  // the test process alive (useQuery holds GC/retry handles otherwise).
  if (activeRoot) {
    await act(async () => {
      activeRoot.unmount();
    });
    activeRoot = null;
  }
  activeClient?.clear();
  activeClient = null;
  clearAllResolvedHistoryScopes();
});

// ── Hold: unresolved group thread does not fetch ────────────────────────────

test("an unresolved group thread is held: the query stays idle and no history fetch fires", async () => {
  const { ref, render } = mountThread(groupChannel("group-1"), ROOT_MSG_ID);
  await render();

  assert.equal(
    ref.current.fetchStatus,
    "idle",
    "no fetch while the group scope is unresolved",
  );
  assert.equal(
    ref.current.status,
    "pending",
    "held thread stays pending, not error",
  );
  assert.equal(
    calls.length,
    0,
    "no x0x_history_list request is issued while held",
  );
});

test("a DM thread is never held: it fetches on the deterministic dm scope", async () => {
  const { ref, render } = mountThread(dmChannel(), ROOT_MSG_ID);
  await render();
  await flush();

  assert.ok(calls.length > 0, "a DM thread fetches immediately");
  assert.equal(calls[0].args.scope, `dm:${"c".repeat(64)}`);
  assert.equal(ref.current.status, "success");
});

// ── Requery on scope arrival ────────────────────────────────────────────────

test("scope arrival enables the thread fetch: held → fetches once the stable scope lands", async () => {
  const { ref, render } = mountThread(
    groupChannel("group-arrives"),
    ROOT_MSG_ID,
  );
  await render();
  assert.equal(calls.length, 0, "held before the scope arrives");

  await setScope("group-arrives", "group:stable-arrives");

  assert.ok(calls.length > 0, "the thread fetches once the scope arrives");
  assert.equal(calls[0].args.scope, "group:stable-arrives");
  assert.equal(ref.current.status, "success");
  assert.ok(
    (ref.current.data ?? []).some((e) => e.id === REPLY_MSG_ID),
    "the reply subtree is loaded against the resolved scope",
  );
});
