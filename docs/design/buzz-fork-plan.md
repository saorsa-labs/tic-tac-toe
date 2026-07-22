# tic-tac-toe — Buzz deep-copy plan

**Status:** Design for review
**Date:** 2026-07-22
**Decision (David):** deep-copy Buzz's desktop UX and work from it; extend for symphony/swarm/templates. End state remains pure x0x (no Nostr on any wire beyond the process boundary, and eventually none at all).
**Ground truth:** seam analysis of `block/buzz` @ main (2026-07-22, Apache-2.0, desktop v0.4.22). File references below are to `buzz-upstream/desktop`.

## 1. What Buzz desktop actually is (fork-relevant facts)

- **Stack: Tauri 2 + React 19/TypeScript** (Vite, Tailwind, TanStack Router +
  React Query), 1,092 TS/TSX files under `desktop/src`. **Not Dioxus** —
  adopting Buzz's UX means adopting this stack. That is the stack decision,
  stated plainly. (The desktop crate is excluded from Buzz's root cargo
  workspace, which eases subtree extraction.)
- **Split protocol stack:** raw WS socket + TLS in Rust
  (`src-tauri/src/native_websocket.rs`, a Tauri plugin), the Nostr protocol
  state machine in TS (`src/shared/api/relayClientSession.ts`, 1,082 lines,
  plus ~20 `relay*` modules), event signing in Rust, private keys in the
  **OS keychain** (`secret_store.rs`, keyring crate). Signing is NOT
  confined to the `sign_event`/`create_auth_event` commands — 12+ Rust
  sites sign (teams, personas, agents, media auth, NIP-98 headers, git
  workflow, pairing) — but **all draw from one key source**
  (`AppState.keys` / `state.signing_keys()` backed by `secret_store.rs`),
  which is the actual flip point for Stage 2.
- **A service layer exists and is disciplined:** UI features consume typed
  domain objects (`types.ts`, 1,012 lines) through the `relayClient`
  singleton choke point; only 4 production files import `nostr-tools`
  directly. But the **event-kind vocabulary (59 exported `KIND_*`
  constants) and tag-threading semantics permeate hooks and lib code**, and
  thread metadata is computed server-side at relay ingest.
- **Local persistence already matches our ADR-0023 choice:** rusqlite
  (bundled), WAL, in `src-tauri/src/archive/` — Buzz caches saved/observer
  events locally in SQLite.
- **Agent orchestration is first-class and reusable:**
  `src-tauri/src/managed_agents/` (61 files; 2,140-line runtime) spawns
  ACP agents (`claude-agent-acp` primary / `claude-code-acp`, `codex-acp`,
  goose, `buzz-agent`) as stdio subprocesses; personas/teams/observer
  frames are signed in the command layer and published as relay events;
  rich UI in `src/features/agents/ui/` + `workflows`/`projects` routes.
- **Two e2e modes ship with it:** Playwright `smoke`/`integration` (110
  specs) against either a **mock IPC bridge** (`src/testing/e2eBridge.ts` —
  stubs every Tauri command and *throws on unknown ones*, so coverage is
  exact; proves the whole UI runs with no live relay) or a **real seeded
  relay**. The mock bridge is a working template for a native-x0xd adapter
  later.

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
lineage for upstream cherry-picks). License hygiene per Apache-2.0 §4:
preserve the upstream LICENSE (Copyright 2026 Block, Inc.); **create our
own NOTICE** attributing Block, Inc. (upstream ships none); **mark modified
files** as changed; our additions dual-licensed per org policy. Strip
server-side crates we will not run (`buzz-relay`, `buzz-push-gateway`,
relay-mesh, admin) from the build, keep `desktop/` + the crates it
path-depends on (`buzz-core`, `buzz-persona`, `buzz-sdk`, `buzz-agent`)
until each is replaced or vendored.

### Stage 1 — "Buzz UX, x0x mesh" (ships the five-minute demo)
Embed in the Tauri shell: `x0xd` (spawn-or-attach, `--name ttt`) + **bridge
v2** on loopback; relay URL is env-driven (`BUZZ_RELAY_URL` →
`ws://127.0.0.1:3300`). Bridge v2's scope is the desktop's **actual**
relay surface, enumerated from source (2026-07-22 review):

| # | Surface | Desktop driver | Milestone |
|---|---|---|---|
| 1 | Nostr WS dialect (REQ/EVENT/EOSE/CLOSE + NIP-29 semantics) | `relayClientSession.ts` | **M1a** |
| 2 | NIP-42 AUTH over WS | `relayClientSession.ts:846` | **M1a** |
| 3 | `POST /events` (4 call sites + snapshot imports) | `relay.rs` | **M1a** |
| 4 | **`POST /query` with Buzz filter extensions** (`top_level`, `include_summaries`, `include_aux`, keyset cursor) — the main channel-timeline read path | `commands/channel_window.rs` | **M1a** |
| 5 | NIP-50 search routed through `/query` | `search_messages` command | **M1a** |
| 6 | `thread_metadata` computed at ingest (see below) | thread views, badges | **M1a** |
| 7 | `GET /info` membership gate | `commands/relay_members.rs` | **M1a** |
| 8 | Live thread-summary emits (recomputed post-commit, fan-out kind) | badges | **M1a** |
| 9 | Blossom media (`PUT /upload`, `GET /media/*`, `buzz-media://` proxy) | `commands/media*.rs`, `media_proxy.rs` | **M1b** |
| 10 | Invite/join-policy API (`/api/invites*`, `/api/join-policy*`, NIP-98-signed) | `shared/api/invites.ts` | **M1b** |
| 11 | Huddle voice (`WS /huddle/{ch}/audio`) | `huddle/relay_api.rs` | **cut for v1** — voice returns P2P via saorsa-webrtc (see `voice-over-x0x.md`) |
| 12 | NIP-11 info doc + pairing discovery | `commands/pairing.rs` | cut (mobile pairing out of scope) |
| 13 | Git smart-HTTP hosting | `project_git*.rs` | cut for v1 (routes hidden) |

Notes: `relay_admission.rs` is the client-side HTTP-429 rate-limit gate,
not an admission API — the bridge just needs sane 429 behavior. Kind
passthrough covers Buzz's 59 exported `KIND_*` constants; per-kind logic
exists only for rows 4/6/8. `thread_metadata` semantics are contained but
strict (marked NIP-10 tags only, parent-must-exist-at-ingest,
server-verified ancestry, depth cap 100, transactional counters,
`(community_id, event_created_at, event_id)` partition keys) — fixtures
mined from `buzz-test-client/tests/e2e_nostr_interop.rs`. Today's bridge
review fixes (NIP-11 test, connection cap, relay-tag) land in M1a.

**M1a acceptance:** mock Playwright suite + relay-mode
messaging/thread/search suites green against bridge v2, zero relay servers
— this IS the five-minute demo. **M1b acceptance:** media + invites suites
green (huddle: explicit ship/cut decision). BuilderLab hosted-community
onboarding (`builderlab.rs`, hardcoded Block service) is **cut in Stage 0**
— local-relay onboarding (`AddCommunityDialog`) is the supported path.

### Stage 2 — identity flip (PQC becomes real)
Swap the **key source**, not a command: all 12+ Rust signing sites draw
from `AppState.keys` / `state.signing_keys()` backed by `secret_store.rs`
— one type behind one accessor. Replace it with a bridge-local
compatibility key (derived per-install, marked as an artifact of the
loopback dialect) while x0xd owns the real identity (ML-DSA-65). UI
displays x0x AgentId + four-word address, never npub (CI grep enforces
from here on). From this stage, Nostr keys authenticate nothing except the
loopback dialect.

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
| `thread_metadata` + extended `/query` dialect fidelity (the same risk, write side + read side — riskiest coupling) | One conformance milestone in M1a with fixtures mined from Buzz's interop tests; moved server-side into the x0x history store in Stage 3 so it is solved once, natively |
| BuilderLab hosted-community dependency (hardcoded Block service) | Cut at Stage 0; local-relay onboarding is the supported path |
| Upstream velocity (Buzz is a week old and moving) | Keep `desktop/` tree cherry-pickable through Stage 2 (desktop is outside Buzz's root workspace, easing subtree extraction); accept divergence at Stage 3 by design — by then the UX shell is ours |
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
