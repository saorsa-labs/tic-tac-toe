# FORK.md — Buzz import anchor (Stage 0.1)

**Source:** https://github.com/block/buzz.git
**Upstream commit:** `710ed9fff57878a1d69f809b80a6ee0416c53fc4` (2026-07-23, "chore(release): release Buzz Desktop version 0.4.24 (#2627)")
**Imported:** 2026-07-22 (Stage 0 of `docs/design/buzz-fork-plan.md`)
**Re-anchored:** 2026-07-24 to `710ed9fff57878a1d69f809b80a6ee0416c53fc4` (0.4.24 release) — Stage 0.1, pre-Stage-1 catch-up
**Import method:** file copy from a shallow clone (no git-subtree lineage; the
commit hash above is the cherry-pick anchor — fetch upstream and diff/apply
against it).

## Imported (byte-preserving except where noted)

| Path | Notes |
|---|---|
| `desktop/` | The Tauri 2 + React app, unmodified (1,900+ files; `node_modules`/`target`/`dist` excluded) |
| `crates/buzz-core`, `crates/buzz-agent`, `crates/buzz-sdk`, `crates/buzz-persona`, `crates/buzz-media` | The five crates `desktop/src-tauri` path-depends on, unmodified. `buzz-media` (deps only on `buzz-core`) became a desktop path-dep at 0.4.24 — newly imported at Stage 0.1 |
| `patches/` (isomorphic-git, virtua) | pnpm patchedDependencies referenced by the workspace file |
| `Cargo.lock`, `pnpm-lock.yaml`, `biome.json`, `rust-toolchain.toml` | Toolchain/resolution fidelity |
| `LICENSE` → `LICENSE-APACHE` | Upstream Apache-2.0, Copyright 2026 Block, Inc. — intact |

## Imported with modification (ours; marked per Apache-2.0 §4(b))

| Path | Modification |
|---|---|
| `Cargo.toml` | Workspace `members` pruned to the five imported crates; `[workspace.package]`/`[workspace.dependencies]` preserved verbatim so `workspace = true` inheritance resolves identically |
| `pnpm-workspace.yaml` | `packages` pruned to `desktop`; upstream `overrides`/`patchedDependencies` blocks preserved verbatim |

## Excluded (not imported)

Server/infra: `crates/buzz-relay`, `buzz-push-gateway`, `buzz-pair-relay`,
`buzz-relay-mesh`, `buzz-admin`, `buzz-db`, `buzz-pubsub`, `buzz-auth`,
`buzz-search`, `buzz-audit`, `buzz-conformance`, `buzz-test-client`,
`buzz-ws-client`, `buzz-workflow`, `buzz-cli`,
`buzz-pairing-cli`, `buzz-acp`, `buzz-dev-mcp`, `sprig`,
`git-credential-nostr`, `git-sign-nostr`; `admin-web/`, `web/`, `mobile/`,
`deploy/`, `bench/`, `benchmarks/`, `perf/`, `migrations/`, `schema/`,
`examples/`, `bin/`, `script/`, `scripts/`, `docker-compose*`, `Dockerfile*`,
`prometheus.yml`, upstream docs/VISION*/governance files. Rationale: the
bridge/native x0x backend replaces the relay stack (`buzz-fork-plan.md` §2);
excluded crates remain reachable at the upstream anchor for cherry-picks.

## License boundary

- Imported Buzz code: **Apache-2.0** (Block, Inc.) — `LICENSE-APACHE` + `NOTICE`.
- tic-tac-toe additions (everything not listed as imported above, incl. docs,
  CI, justfile, and all Stage 1+ new code): the repo's dual license
  (AGPL-3.0 / commercial).
- From Stage 1 onward, modified imported files carry a
  `// Modified from block/buzz @ 710ed9ff — see FORK.md` header (§4(b) marking).
