#!/usr/bin/env bash
#
# bridge-gate.sh — M1a relay-mode acceptance gate for the x0x-nostr-bridge.
#
# Spins up a FULLY ISOLATED x0xd (no bootstrap peers, loopback-only QUIC,
# self-update disabled) and the x0x-nostr-bridge with its demo seed, waits for
# the same readiness contract the Playwright suite asserts (GET /info returns
# NIP-11, POST /query returns the kind-39000 `general` channel over host
# `localhost`), then runs the desktop `integration` Playwright project against
# it. Both child processes are torn down via a trap on any exit.
#
# SAFETY: the x0xd here must never join the real x0x network. We pass
# --no-hard-coded-bootstrap --disable-peer-cache, an empty `bootstrap_peers`,
# a loopback QUIC bind (kills mDNS LAN discovery), and `[update] enabled=false`.
# It runs under its own --name/data-dir and API port; nothing outside this
# script's own PIDs is signalled.
#
# Env overrides (all optional):
#   X0X_DIR          path to the x0x repo        (default: <repo>/../x0x)
#   BRIDGE_DIR       path to x0x-nostr-bridge     (default: <repo>/../x0x-nostr-bridge)
#   BRIDGE_PORT      bridge HTTP+WS port          (default: auto free port)
#   X0XD_API_PORT    isolated x0xd REST port      (default: auto free port)
#   X0XD_QUIC_PORT   isolated x0xd QUIC/UDP port  (default: auto free port)
#   BRIDGE_GATE_SPECS  space-separated spec files to run instead of the whole
#                    integration project — e.g. the 4 relay-mode specs:
#                    "integration.spec.ts stream.spec.ts \
#                     dm-double-notification.spec.ts parity-ancestor-island.spec.ts"
#
# Exit code is the Playwright exit code (0 = gate green).

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

X0X_DIR="${X0X_DIR:-$(cd "$REPO_ROOT/../x0x" 2>/dev/null && pwd || true)}"
BRIDGE_DIR="${BRIDGE_DIR:-$(cd "$REPO_ROOT/../x0x-nostr-bridge" 2>/dev/null && pwd || true)}"

if [[ -z "${X0X_DIR:-}" || ! -d "$X0X_DIR" ]]; then
  echo "FATAL: x0x repo not found (set X0X_DIR)" >&2; exit 2
fi
if [[ -z "${BRIDGE_DIR:-}" || ! -d "$BRIDGE_DIR" ]]; then
  echo "FATAL: x0x-nostr-bridge repo not found (set BRIDGE_DIR)" >&2; exit 2
fi

# --- helpers ----------------------------------------------------------------

# Print a free TCP port (asks the OS for an ephemeral one, then releases it).
free_port() {
  python3 - <<'PY'
import socket
s = socket.socket(); s.bind(("127.0.0.1", 0))
print(s.getsockname()[1]); s.close()
PY
}

log() { printf '[bridge-gate] %s\n' "$*" >&2; }

WORK_DIR="$(mktemp -d "${TMPDIR:-/tmp}/bridge-gate.XXXXXX")"
LOG_DIR="$WORK_DIR/logs"; mkdir -p "$LOG_DIR"
X0XD_PID=""; BRIDGE_PID=""

cleanup() {
  local ec=$?
  log "tearing down (exit $ec)"
  [[ -n "$BRIDGE_PID" ]] && kill "$BRIDGE_PID" 2>/dev/null || true
  [[ -n "$X0XD_PID"   ]] && kill "$X0XD_PID"   2>/dev/null || true
  # give them a moment, then hard-kill any survivor
  sleep 1
  [[ -n "$BRIDGE_PID" ]] && kill -9 "$BRIDGE_PID" 2>/dev/null || true
  [[ -n "$X0XD_PID"   ]] && kill -9 "$X0XD_PID"   2>/dev/null || true
  log "logs kept in $LOG_DIR"
}
trap cleanup EXIT INT TERM

BRIDGE_PORT="${BRIDGE_PORT:-$(free_port)}"
X0XD_API_PORT="${X0XD_API_PORT:-$(free_port)}"
X0XD_QUIC_PORT="${X0XD_QUIC_PORT:-$(free_port)}"

# --- build binaries (always — cargo fingerprints decide freshness) ----------
#
# We invoke `cargo build` every run instead of gating on the presence of a
# previously-built binary. cargo's own dep-info fingerprinting no-ops in well
# under a second when the tree is already up to date and, unlike a presence or
# mtime check, also catches deleted source files, build.rs / env changes,
# Cargo.lock movement and toolchain changes.
#
# The previous presence-only guard (`if [[ ! -x "$BIN" ]]`) let a binary built
# before an upstream fix landed masquerade as a passing gate: the bridge's
# 39006 window-bounds overlay silently keyed off the wrong cursor, the stale
# binary served it, and the client threw every page away (empty timeline).
# Binding the gate to source freshness — not binary presence — closes that.

X0XD_BIN="$X0X_DIR/target/debug/x0xd"
log "ensuring x0xd (debug) is current ..."
( cd "$X0X_DIR" && cargo build --bin x0xd )
BRIDGE_BIN="$BRIDGE_DIR/target/debug/x0x-nostr-bridge"
log "ensuring x0x-nostr-bridge (debug) is current ..."
( cd "$BRIDGE_DIR" && cargo build --bin x0x-nostr-bridge )

# --- isolated x0xd ----------------------------------------------------------

X0XD_DATA="$WORK_DIR/x0xd-data"
X0XD_CFG="$WORK_DIR/x0xd.toml"
cat > "$X0XD_CFG" <<EOF
instance_name = "bridgegate"
data_dir = "$X0XD_DATA"
api_address = "127.0.0.1:$X0XD_API_PORT"
bind_address = "127.0.0.1:$X0XD_QUIC_PORT"
log_level = "info"
bootstrap_peers = []

[update]
enabled = false
EOF

log "starting isolated x0xd (api 127.0.0.1:$X0XD_API_PORT, quic 127.0.0.1:$X0XD_QUIC_PORT)"
"$X0XD_BIN" --config "$X0XD_CFG" \
  --no-hard-coded-bootstrap --disable-peer-cache --skip-update-check \
  > "$LOG_DIR/x0xd.log" 2>&1 &
X0XD_PID=$!

# wait for the daemon to advertise its API + token and answer /health
X0X_TOKEN=""
for _ in $(seq 1 60); do
  if [[ -f "$X0XD_DATA/api.port" && -f "$X0XD_DATA/api-token" ]]; then
    X0X_TOKEN="$(cat "$X0XD_DATA/api-token")"
    if python3 - "$X0XD_API_PORT" "$X0X_TOKEN" <<'PY' 2>/dev/null; then
import sys, json, urllib.request
port, token = sys.argv[1], sys.argv[2]
req = urllib.request.Request(f"http://127.0.0.1:{port}/health",
                             headers={"Authorization": f"Bearer {token}"})
h = json.loads(urllib.request.urlopen(req, timeout=3).read())
sys.exit(0 if h.get("ok") else 1)
PY
      break
    fi
  fi
  kill -0 "$X0XD_PID" 2>/dev/null || { log "x0xd died early; see $LOG_DIR/x0xd.log"; exit 3; }
  sleep 1
done
[[ -n "$X0X_TOKEN" ]] || { log "x0xd never became ready"; exit 3; }
log "x0xd healthy"

# --- bridge -----------------------------------------------------------------

log "starting bridge on 127.0.0.1:$BRIDGE_PORT (seed-demo on)"
env \
  BRIDGE_BIND="127.0.0.1:$BRIDGE_PORT" \
  BRIDGE_DB="$WORK_DIR/bridge.db" \
  BRIDGE_SEED_DEMO=true \
  BRIDGE_PUBLIC_URL="http://localhost:$BRIDGE_PORT" \
  X0X_API="127.0.0.1:$X0XD_API_PORT" \
  X0X_TOKEN="$X0X_TOKEN" \
  RUST_LOG="${RUST_LOG:-info}" \
  "$BRIDGE_BIN" > "$LOG_DIR/bridge.log" 2>&1 &
BRIDGE_PID=$!

# Readiness: exactly what assertRelaySeeded() asserts — GET /info is NIP-11 and
# POST /query for kind-39000 returns the `general` channel. MUST use the
# `localhost` hostname (the seed maps host `localhost`, not `127.0.0.1`).
TYLER="e5ebc6cdb579be112e336cc319b5989b4bb6af11786ea90dbe52b5f08d741b34"
ready=""
for _ in $(seq 1 40); do
  if python3 - "$BRIDGE_PORT" "$TYLER" <<'PY' 2>/dev/null; then
import sys, json, urllib.request
port, tyler = sys.argv[1], sys.argv[2]
base = f"http://localhost:{port}"
info = json.loads(urllib.request.urlopen(base + "/info", timeout=3).read())
assert 11 in info.get("supported_nips", []), "not NIP-11"
data = json.dumps([{"kinds": [39000], "limit": 200}]).encode()
req = urllib.request.Request(base + "/query", data=data, method="POST",
    headers={"X-Pubkey": tyler, "Content-Type": "application/json"})
evs = json.loads(urllib.request.urlopen(req, timeout=3).read())
ok = any(any(t[:2] == ["name", "general"] for t in e.get("tags", [])) for e in evs)
sys.exit(0 if ok else 1)
PY
    ready=1; break
  fi
  kill -0 "$BRIDGE_PID" 2>/dev/null || { log "bridge died early; see $LOG_DIR/bridge.log"; exit 4; }
  sleep 1
done
[[ -n "$ready" ]] || { log "bridge never served the seed; see $LOG_DIR/bridge.log"; exit 4; }
log "bridge seeded and ready at http://localhost:$BRIDGE_PORT"

# --- the gate ---------------------------------------------------------------

log "running Playwright integration project against the bridge"
set +e
(
  cd "$REPO_ROOT/desktop" && corepack pnpm build:e2e >/dev/null 2>&1
  cd "$REPO_ROOT/desktop" && \
    BUZZ_E2E_RELAY_URL="http://localhost:$BRIDGE_PORT" \
    corepack pnpm exec playwright test --project=integration ${BRIDGE_GATE_SPECS:-} --reporter=list
)
GATE_EC=$?
set -e
log "Playwright exit $GATE_EC"
exit $GATE_EC
