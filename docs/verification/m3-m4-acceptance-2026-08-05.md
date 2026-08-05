# M3/M4 acceptance verification — 2026-08-05

## Result

The native desktop release gates pass on the current tree. Verification found and fixed four gate defects: stale identity assertions after the native messaging cutover, five unformatted native adapter files, a forbidden legacy identity token in a native-membership comment, and an E2E mock that did not expose native group/Symphony commands.

The Playwright `smoke` project now gates the M3/M4 transport and product slice rather than the imported relay-dialect suite: native workspace boot proves no relay apply/WebSocket command is issued, and Company exercises template selection, instantiation, approval, and cancellation.

## Desktop evidence

| Command | Result |
|---|---|
| `cd desktop && corepack pnpm typecheck` | PASS |
| focused native adapter unit command (history/thread, messaging, community/member, identity/presence, workflows, Symphony, no-relay) | PASS — 28/28 |
| `cargo test --manifest-path desktop/src-tauri/Cargo.toml --all-features company_template -- --nocapture` | PASS — 37/37 |
| `cargo test --manifest-path desktop/src-tauri/Cargo.toml --all-features symphony -- --nocapture` | PASS — 28/28 |
| `cd desktop && corepack pnpm test` | PASS — 3,564 passed, 0 failed, 0 skipped |
| `cd desktop && corepack pnpm lint` | PASS — 1,635 files |
| `cd desktop && corepack pnpm check` | PASS — Biome, file-size, px-text, pubkey-truncation, Nostr-identity, and no-relay gates |
| `just desktop-smoke` | PASS — 2/2 native M3/M4 Playwright scenarios |

Before the native smoke definition was corrected, the imported Buzz relay-dialect selection produced 345 passes and 324 failures. Those specs remain in the repository for feature-by-feature migration, but are deliberately outside the M3/M4 release smoke gate; treating relay WebSocket behavior as native acceptance would be false evidence.

## Rust gates — tic-tac-toe

The mandated order was used across both Cargo workspaces: format, all-target lint, production panic-safety lint, then check.

1. `cargo fmt --all` — PASS
2. `cargo fmt --manifest-path desktop/src-tauri/Cargo.toml --all` — PASS
3. `cargo clippy --all-features --all-targets -- -D warnings` — PASS
4. `cargo clippy --manifest-path desktop/src-tauri/Cargo.toml --all-features --all-targets -- -D warnings` — PASS
5. `cargo clippy --all-features --lib --bins -- -D warnings -D clippy::panic -D clippy::unwrap_used -D clippy::expect_used` — PASS
6. the same production panic-safety command with the Tauri manifest — PASS
7. `cargo check --workspace --all-targets` — PASS
8. `cargo check --manifest-path desktop/src-tauri/Cargo.toml --workspace --all-targets` — PASS

## x0x evidence

### ADR-0001 seed-hint controls

- Cache/coordinator focused filter: 3/3 PASS, including `local_discovery_scope_tracks_bootstrap_partition` and `proactive_reconnect_default_global_bootstrap_same_host`.
- `bootstrap_cache_integration` plus `gossip_cache_adapter_integration`: 10/10 PASS, including restart persistence and signed coordinator-advert cache enrichment.

### Required Rust gates

After replacing one Clippy-flagged manual `Option` branch with `?`, the exact sequence passed:

1. `cargo fmt --all`
2. `cargo clippy --all-features --all-targets -- -D warnings`
3. `cargo clippy --all-features --lib --bins -- -D warnings -D clippy::panic -D clippy::unwrap_used -D clippy::expect_used`
4. `cargo check --workspace --all-targets`

### Native data and two-daemon workflows

- History store/wiring/API focused default tests: 13/13 PASS.
- Real two-daemon ignored history suite: 8/8 PASS in 171.334 s:
  - REST list, FTS search, stats, and purge;
  - WebSocket backfill → live with no gap/duplicate;
  - hard-kill restart survival for outbound/inbound DM history;
  - offline ML-DSA signature re-verification;
  - recorder backpressure behavior.
- `bash tests/e2e_dogfood_local.sh`: PASS — 20/20 in 6 s. This cold two-daemon run covered 64-hex AgentIds, agent-card exchange/import, contact trust transitions, DM round-trip, named-group create/invite/join/authority commit/messages/leave.
- Thread ancestry is covered at the desktop native boundary by the passing history contract test (`root_msg_id`/`parent_msg_id` mapping and paging). The x0x repository currently has no distinct two-daemon threaded-message harness; threads are stored message ancestry rather than a separate daemon workflow.

## Symphony/Company evidence

- Company backend tests: 37/37 PASS, covering `software-dev-and-sales`, private Engineering/Sales groups, public All-Hands, deterministic planning, resumable provisioning, workflow generation, and no relay/Nostr emission.
- Symphony client/supervisor tests: 28/28 PASS, covering loopback-only API use, SSE parsing, attach/spawn, shutdown, and token redaction.
- `x0x-symphony-bin --test approvals_api`: 16/16 PASS, including signed approvals, deny, stale hash/signer rejection, blocked-task requeue, concurrent approval, and approval events.
- Playwright Company smoke: PASS through template selection → instantiate/run → waiting approval → approve → cancel/return.

## External prerequisites not exercised

No external prerequisite was substituted with a placeholder. The local harnesses cover all workflows they expose. The following proof-point operations require facilities outside this checkout and were therefore not claimed:

- a physical two-machine packet capture and offline inspection of that capture;
- VPS/cross-region execution;
- a distinct live threaded-message harness (none exists in x0x today).

The local two-daemon signature test does verify the cryptographic history artifact offline, but it is not reported as a packet-capture result.
