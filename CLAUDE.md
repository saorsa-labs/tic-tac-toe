# CLAUDE.md — tic-tac-toe

The native x0x workspace frontend. Desktop app (Tauri 2 + Dioxus planned);
thin client over the local `x0xd` REST/WS API.

## Rules

Follow the Saorsa Labs 12-rule workflow (see the workspace-level CLAUDE.md).
Zero warnings, zero `.unwrap()`/`.expect()` in production code, justfile-first.

## Project-specific rules

- **The app holds no protocol state.** All chat/group/history state lives in
  `x0xd`; the app rehydrates from the daemon on every launch. If a feature
  needs app-side persistence beyond UI preferences, that's a design smell —
  push it into x0x (probably the ADR-0023 history store).
- **Daemon-only integration.** Talk to `x0xd` exclusively over REST/WS with
  token auth — no direct `x0x` crate calls from UI code. We dogfood the
  public API.
- **Design source of truth:** `docs/design/tic-tac-toe-v1.md` (v1 scope,
  acceptance suite, milestones). The proof-point acceptance suite in §5 is
  the definition of done for v1.

## Key dependencies

- `x0x` / `x0xd` — sibling repo (`../x0x`), REST/WS API reference:
  `../x0x/docs/api-reference.md`
- x0x ADR-0023 (durable local history) — load-bearing; v1 is sequenced
  behind it (M0 in the design doc)
- `x0x-symphony` — agent orchestration (v2, company templates)
