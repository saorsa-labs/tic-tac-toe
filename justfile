# tic-tac-toe — justfile (Stage 0: imported Buzz desktop tree, see FORK.md)

default:
    @just --list

# Install JS workspace deps (pnpm 11.4.0 via corepack)
install:
    corepack pnpm install --no-frozen-lockfile

# Typecheck + unit tests + lint check for the desktop app
desktop-check:
    cd desktop && corepack pnpm typecheck
    cd desktop && corepack pnpm test
    cd desktop && corepack pnpm lint

# Playwright smoke suite in mock mode (no relay, no daemon)
desktop-smoke:
    cd desktop && corepack pnpm build:e2e && corepack pnpm exec playwright test --project=smoke

# Check the five imported Rust crates against the pruned workspace
crates-check:
    cargo check -p buzz-core -p buzz-persona -p buzz-sdk -p buzz-agent -p buzz-media

# Reject compatibility transports and retired bridge configuration in the packaged app.
# The invariant test verifies the gate's Rust/Nostr detection logic (always green);
# the gate itself stays red until the M3 relay/Nostr cutover completes.
no-relay-gate:
    node --test scripts/no-relay-gate.test.mjs
    node scripts/no-relay-gate.mjs

# Stage x0xd for the active target triple (Tauri externalBin naming).
stage-sidecars:
    scripts/stage-sidecars.sh

# Full validation
check: desktop-check crates-check

# Render the design doc tree
docs:
    @find docs -name '*.md' | sort
