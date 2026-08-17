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

# Check the imported Rust crates and native managed-agent bridges.
crates-check:
    cargo check -p buzz-core -p buzz-persona -p buzz-sdk -p buzz-agent -p buzz-media -p buzz-acp -p buzz-x0x-mcp

# Reject compatibility transports and retired bridge configuration in the packaged app.
# The invariant test verifies the gate's Rust/Nostr detection logic (always green);
# the gate itself stays red until the M3 relay/Nostr cutover completes.
no-relay-gate:
    node --test scripts/no-relay-gate.test.mjs
    node --test scripts/portable-package-contract.test.mjs
    node scripts/no-relay-gate.mjs

# Stage native managed-agent binaries and x0xd for the active target triple.
stage-sidecars:
    scripts/stage-sidecars.sh

# Reject incomplete or signature-invalid app bundles before DMG publication.
package-macos-dmg-test:
    scripts/package-macos-dmg.test.sh

# ttt #12: refuse campaign-branch x0xd strings in bundled sidecars.
sidecar-campaign-denylist-test:
    scripts/sidecar-campaign-denylist.test.sh

# Bundled 0.38.0 product send → 0.37.4 peer must 409; opt-out must deliver.
mixed-version-dm-smoke:
    scripts/mixed-version-dm-smoke.sh

# Full validation
check: desktop-check crates-check

# Render the design doc tree
docs:
    @find docs -name '*.md' | sort
