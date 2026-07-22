# tic-tac-toe — Buzz deep-copy plan

**Status:** Design for review
**Date:** 2026-07-22
**Decision (David):** deep-copy Buzz's desktop UX and work from it; extend for symphony/swarm/templates. End state remains pure x0x (no Nostr on any wire beyond the process boundary, and eventually none at all).
**Ground truth:** seam analysis of `block/buzz` @ main (2026-07-22, Apache-2.0, desktop v0.4.22). File references below are to `buzz-upstream/desktop`.

## 1. What Buzz desktop actually is (fork-relevant facts)

- **Stack: Tauri 2 + React 19/TypeScript** (Vite, Tailwind, TanStack Router +
  React Query), ~1,200 TS/TSX files. **Not Dioxus** — adopting Buzz's UX
  means adopting this stack. That is the stack decision, stated plainly.
- **Split protocol stack:** raw WS socket + TLS in Rust
  (`src-tauri/src/native_websocket.rs`, a Tauri plugin), the Nostr protocol
  state machine in TS (`src/shared/api/relayClientSession.ts`, ~1,050 lines,
  plus ~30 `relay*` modules), event signing in Rust (`sign_event` /
  `create_auth_event` commands), private keys in the **OS keychain**
  (`secret_store.rs`, keyring crate).
- **A service layer exists and is disciplined:** UI features consume typed
  domain objects (`types.ts`, ~1,000 lines) through the `relayClient`
  singleton choke point; only 5 files import `nostr-tools` directly. But the
  **event-kind vocabulary (~100 `KIND_*` constants) and tag-threading
  semantics permeate hooks and lib code**, and thread metadata is computed
  server-side at relay ingest.
- **Local persistence already matches our ADR-0023 choice:** rusqlite
  (bundled), WAL, in `src-tauri/src/archive/` — Buzz caches saved/observer
  events locally in SQLite.
- **Agent orchestration is first-class and reusable:**
  `src-tauri/src/managed_agents/` (46 files; 2,140-line runtime) spawns
  ACP agents (`claude-code-acp`, `codex-acp`, goose) as stdio subprocesses;
  personas/teams/observer frames are published as relay events; rich UI in
  `src/features/agents/ui/` + `workflows`/`projects` routes.
- **Two e2e modes ship with it:** Playwright `smoke`/`integration` (~80
  specs) against either a **mock IPC bridge** (`src/testing/e2eBridge.ts` —
  stubs every Tauri command; proves the whole UI runs with no live relay)
  or a **real seeded relay**. The mock bridge is a working template for a
  native-x0xd adapter later.

## 2. Fork strategy — staged, with the end state fixed

The seam analysis makes the trade explicit:

| Path | UI changes | Risk |
|---|---|---|
| (a) Point desktop at a localhost Nostr facade over x0x | ~0 files (relay URL is env-driven) | Protocol fidelity: facade must speak Buzz's relay dialect |
| (b) Native x0xd data layer, rip out Nostr | 40–60 files incl. the reconnect/liveness subsystem | Three deep couplings (§4) |

We do **(a) → (b) in stages**, because (a) gives a fully working product on
the x0x mesh in weeks, and (b) is where the PQC/serverless end-state claims
are earned. Nostr survives *temporarily* as an in-process localhost dialect
between our own UI and our own daemon — never on the network.

### Stage 0 — fork hygiene
Fork `block/buzz` → `saorsa-labs/tic-tac-toe` app tree (keep `desktop/`
lineage for upstream cherry-picks; preserve Apache-2.0 LICENSE + NOTICE with
attribution; our additions dual-licensed per org policy). Strip server-side
crates we will not run (`buzz-relay`, `buzz-push-gateway`, relay-mesh, admin)
from the build, keep `desktop/` + the crates it path-depends on
(`buzz-core`, `buzz-persona`, `buzz-sdk`, `buzz-agent`) until each is
replaced or vendored.

### Stage 1 — "Buzz UX, x0x mesh" (ships the five-minute demo)
Embed in the Tauri shell: `x0xd` (spawn-or-attach, `--name ttt`) + **bridge
v2** on loopback; set `BUZZ_RELAY_URL=ws://127.0.0.1:3300`. Bridge v2 grows
beyond the spike to Buzz's actual relay dialect:

1. The **relay HTTP API** used by `src-tauri/relay.rs` (reqwest):
   `POST /events`, membership/admission endpoints (`relay_admission.rs`).
2. **Custom-kind passthrough** (~100 kinds incl. 40002/40003 edits,
   39005/39006 threads, 43001–43006 jobs, 44100/44101 membership) — the
   bridge stores/fans-out by class rules, it does not need per-kind logic
   except:
3. **`thread_metadata` computed at ingest** (root/reply depth, counts) —
   Buzz's read model assumes the relay computes this; the bridge must
   reproduce it or every threaded view breaks (riskiest single item, has
   its own conformance milestone).
4. Multi-client fan-out on loopback (today's bridge caps are fine; the
   NIP-11 test, connection cap, and relay-tag fixes from the 2026-07-22
   review land here).

`buzz-conformance` + the Playwright `relay`-mode suite become the
acceptance gate: **unmodified Buzz UI, all suites green, zero relay
servers.**

### Stage 2 — identity flip (PQC becomes real)
Replace `secret_store.rs`-held nsec as the *primary* identity: x0xd owns
identity (ML-DSA-65, existing key files); the Tauri `sign_event` command is
re-pointed at a bridge-local Schnorr key **derived per-install and marked
as a compatibility artifact** — UI identity displays x0x AgentId +
four-word address, not npub. From here on, Nostr keys authenticate nothing
except the loopback dialect.

### Stage 3 — data-layer replacement, feature by feature
Using the mock-bridge pattern (`e2eBridge.ts`) as the adapter template,
introduce `x0xClientSession.ts` beside `relayClientSession.ts` and migrate
per feature (DMs → channels → threads → groups/MLS → presence), consuming
x0xd REST/WS with **ADR-0023 backfill-then-live** replacing REQ/EOSE, and
`/history/search` replacing relay search. The three risky couplings are
retired deliberately:
   - kind vocabulary → a `scope`/`content_type` mapping module (one place),
   - AUTH/reconnect state machine → rewritten against x0xd's token auth +
     WS semantics (this is the subsystem rewrite; scheduled, not incidental),
   - `thread_metadata` → computed by x0xd history store at write time
     (extension to ADR-0023 store: thread columns), removing the bridge's
     hardest job.
Bridge v2 is deleted from the app when the last feature migrates (it
remains a standalone Saorsa project for ecosystem interop).

### Stage 4 — symphony/swarm/templates (the extension David wants)
Buzz's `managed_agents/` runtime already speaks ACP and spawns
claude/codex/goose subprocesses; personas/teams are events. Mapping:
personas/teams/observer-frames → x0x named groups + agent cards + KV;
`config_bridge` → symphony presets; workflows route → symphony
`WORKFLOW.md`; **company templates** (agent/skill-pair bundles —
software-dev+sales first) instantiate through symphony onto the same
surfaces the UI already renders. This stage is where tic-tac-toe stops
being "Buzz on x0x" and becomes the company-in-a-box product.

## 3. Functional test plan (David's directive: complete functional testing)

- **Inherited:** Playwright smoke/integration (~80 specs) in mock mode run
  unchanged from Stage 0; relay-mode suites run against bridge v2 from
  Stage 1 (they are the bridge conformance gate).
- **Mesh-level:** the 6-node testnet plane (UDP 6483 / API 13600 — healthy
  on 0.34.2 as of 2026-07-22) hosts multi-region daemon meshes; tic-tac-toe
  clients attach to local daemons that peer with the testnet for
  cross-region convergence/latency runs (history + backfill under real WAN
  churn).
- **Native-shell functional testing on the Studios:** the two M3 Ultra
  Studios run the full desktop app; driven over ssh/mosh with window
  forwarding and the Apple computer-control skills (Screen/AX-driven) to
  execute the §5 acceptance suite of `tic-tac-toe-v1.md` end-to-end —
  real windows, real keychain, real two-machine mesh, no mocks. Recordings
  (gif/screencap) become the demo artifacts.
- **Cadence:** mock Playwright on every PR; bridge-conformance +
  local-mesh nightly; Studio + testnet full functional pass per milestone.

## 4. Risks

| Risk | Mitigation |
|---|---|
| `thread_metadata` fidelity (riskiest coupling) | Own conformance milestone in Stage 1; moved server-side into the x0x history store in Stage 3 so it is solved once, natively |
| Upstream velocity (Buzz is a week old and moving) | Keep `desktop/` tree cherry-pickable through Stage 2; accept divergence at Stage 3 by design — by then the UX shell is ours |
| Two Nostr identities linger past Stage 2 | Stage gate: after Stage 2, npub never appears in UI; CI grep enforces |
| Stack: team is Rust-first, app is React/TS | Accepted cost of "work from their UX" (David's call); Rust stays in the Tauri shell where our expertise concentrates (daemon, bridge, ACP runtime) |
| managed_agents runtime is deeply Nostr-evented | Stage 4 maps events → x0x surfaces before extending; do not build templates on the Nostr shapes |

## 5. Immediate next actions

1. Bridge v2 scope doc + `thread_metadata` conformance fixture extraction
   from `buzz-relay` ingest code.
2. Fork + Stage 0 hygiene (LICENSE/NOTICE, strip server crates, CI:
   Playwright mock suite green in our repo).
3. ADR-0023 store lands in x0xd (M0 of `tic-tac-toe-v1.md`) — prerequisite
   for Stage 3, parallel to Stages 0–1.
