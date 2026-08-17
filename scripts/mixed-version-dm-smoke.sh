#!/usr/bin/env bash
# Mixed-version DM smoke: bundled official 0.38.0 → released 0.37.4 peer.
#
# Product send (durable-by-default) must 409
# `recipient_ack_semantics_unavailable` — ADR 0030 §2 forbids a silent
# downgrade. The same pair with `require_durable_app_ack: false` must
# still deliver (explicit opt-out).
#
# Isolated loopback pair: --no-hard-coded-bootstrap, no prod mesh.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
# shellcheck source=sidecar-validation.sh
source "$SCRIPT_DIR/sidecar-validation.sh"

BUNDLED_X0XD="${BUNDLED_X0XD:-$REPO_ROOT/desktop/src-tauri/binaries/x0xd-aarch64-apple-darwin}"
LEGACY_X0XD="${LEGACY_X0XD:-}"

if [[ ! -x "$BUNDLED_X0XD" ]]; then
  echo "FATAL: bundled x0xd missing at $BUNDLED_X0XD (run stage-sidecars or fetch-official-x0xd)" >&2
  exit 2
fi
assert_official_x0xd_identity "$BUNDLED_X0XD"

fetch_legacy_x0xd() {
  local cache="${X0XD_RELEASE_CACHE:-${TMPDIR:-/tmp}/x0x-official-${LEGACY_X0XD_VERSION}}"
  local tarball="$cache/x0x-macos-arm64.tar.gz"
  local extract="$cache/extract"
  local dest="$extract/x0x-macos-arm64/x0xd"
  local url="https://github.com/saorsa-labs/x0x/releases/download/v${LEGACY_X0XD_VERSION}/x0x-macos-arm64.tar.gz"
  mkdir -p "$cache"
  if [[ ! -f "$tarball" ]]; then
    curl -fsSL -o "$tarball.partial" "$url"
    mv "$tarball.partial" "$tarball"
  fi
  local actual
  actual="$(shasum -a 256 "$tarball" | awk '{print $1}')"
  if [[ "$actual" != "$LEGACY_X0XD_MACOS_ARM64_TARBALL_SHA256" ]]; then
    echo "FATAL: legacy tarball sha256 $actual != $LEGACY_X0XD_MACOS_ARM64_TARBALL_SHA256" >&2
    exit 5
  fi
  rm -rf "$extract"
  mkdir -p "$extract"
  tar -xzf "$tarball" -C "$extract"
  if [[ ! -x "$dest" ]]; then
    echo "FATAL: legacy archive did not contain x0xd at $dest" >&2
    exit 5
  fi
  local ver bin_sha
  ver="$("$dest" --version 2>/dev/null || true)"
  if [[ "$ver" != *"$LEGACY_X0XD_VERSION"* ]]; then
    echo "FATAL: $dest reports '$ver', expected $LEGACY_X0XD_VERSION" >&2
    exit 5
  fi
  bin_sha="$(shasum -a 256 "$dest" | awk '{print $1}')"
  if [[ "$bin_sha" != "$LEGACY_X0XD_AARCH64_APPLE_DARWIN_SHA256" ]]; then
    echo "FATAL: $dest sha256 $bin_sha != $LEGACY_X0XD_AARCH64_APPLE_DARWIN_SHA256" >&2
    exit 5
  fi
  printf '%s\n' "$dest"
}

if [[ -z "$LEGACY_X0XD" ]]; then
  LEGACY_X0XD="$(fetch_legacy_x0xd)"
fi

WORK="$(mktemp -d "${TMPDIR:-/tmp}/ttt-mixdm.XXXXXX")"
cleanup() {
  if [[ -n "${A_PID:-}" ]]; then kill "$A_PID" 2>/dev/null || true; fi
  if [[ -n "${B_PID:-}" ]]; then kill "$B_PID" 2>/dev/null || true; fi
  wait || true
  rm -rf "$WORK"
}
trap cleanup EXIT

A_DIR="$WORK/a"
B_DIR="$WORK/b"
mkdir -p "$A_DIR" "$B_DIR" "$WORK/Library/Application Support" "$WORK/share"

start_node() {
  local bin="$1" name="$2" port="$3" dir="$4"
  HOME="$WORK" XDG_DATA_HOME="$WORK/share" \
    "$bin" --name "$name" --api-port "$port" --no-hard-coded-bootstrap --skip-update-check \
    >"$dir/x0xd.out" 2>&1 &
  echo $!
}

# A = bundled 0.38.0 (product durable-by-default). B = official 0.37.4 (v1).
A_PID="$(start_node "$BUNDLED_X0XD" mix-a 18711 "$A_DIR")"
B_PID="$(start_node "$LEGACY_X0XD" mix-b 18712 "$B_DIR")"

wait_health() {
  local port="$1" token_file="$2"
  local i token
  for i in $(seq 1 40); do
    if [[ -f "$token_file" ]]; then
      token="$(tr -d ' \n' < "$token_file")"
      if curl -sf -m 2 -H "Authorization: Bearer $token" "http://127.0.0.1:${port}/health" \
        | grep -q '"ok":true'; then
        return 0
      fi
    fi
    sleep 0.25
  done
  echo "FATAL: daemon on :$port never became healthy" >&2
  return 1
}

# Named instances on macOS land under $HOME/Library/Application Support/x0x-<name>
# unless XDG_DATA_HOME is honoured. Probe both.
find_token() {
  local name="$1"
  local candidate
  for candidate in \
    "$WORK/Library/Application Support/x0x-$name/api-token" \
    "$WORK/share/x0x-$name/api-token" \
    "$WORK/.x0x-$name/api-token"
  do
    if [[ -f "$candidate" ]]; then
      printf '%s\n' "$candidate"
      return 0
    fi
  done
  return 1
}

A_TOKEN_FILE=""
B_TOKEN_FILE=""
for i in $(seq 1 40); do
  A_TOKEN_FILE="$(find_token mix-a || true)"
  B_TOKEN_FILE="$(find_token mix-b || true)"
  if [[ -n "$A_TOKEN_FILE" && -n "$B_TOKEN_FILE" ]]; then
    break
  fi
  sleep 0.25
done
wait_health 18711 "$A_TOKEN_FILE"
wait_health 18712 "$B_TOKEN_FILE"
A_TOKEN="$(tr -d ' \n' < "$A_TOKEN_FILE")"
B_TOKEN="$(tr -d ' \n' < "$B_TOKEN_FILE")"

api() {
  local token="$1" port="$2" path="$3" method="${4:-GET}" body="${5:-}"
  if [[ -n "$body" ]]; then
    curl -sS -m 20 -X "$method" -H "Authorization: Bearer $token" \
      -H "Content-Type: application/json" -d "$body" "http://127.0.0.1:${port}${path}"
  else
    curl -sS -m 20 -H "Authorization: Bearer $token" \
      "http://127.0.0.1:${port}${path}"
  fi
}

A_CARD="$(api "$A_TOKEN" 18711 '/agent/card?include_local_addresses=true')"
B_CARD="$(api "$B_TOKEN" 18712 '/agent/card?include_local_addresses=true')"
A_LINK="$(python3 -c 'import json,sys; print(json.load(sys.stdin)["link"])' <<<"$A_CARD")"
B_LINK="$(python3 -c 'import json,sys; print(json.load(sys.stdin)["link"])' <<<"$B_CARD")"
A_ID="$(api "$A_TOKEN" 18711 /agent | python3 -c 'import json,sys; print(json.load(sys.stdin)["agent_id"])')"
B_ID="$(api "$B_TOKEN" 18712 /agent | python3 -c 'import json,sys; print(json.load(sys.stdin)["agent_id"])')"

api "$B_TOKEN" 18712 /agent/card/import POST "$(python3 -c 'import json,sys; print(json.dumps({"card":sys.argv[1],"trust_level":"trusted"}))' "$A_LINK")" >/dev/null
api "$A_TOKEN" 18711 /agent/card/import POST "$(python3 -c 'import json,sys; print(json.dumps({"card":sys.argv[1],"trust_level":"trusted"}))' "$B_LINK")" >/dev/null

PAYLOAD="$(python3 -c 'import base64; print(base64.b64encode(b"{\"text\":\"cutest-mixdm\",\"createdAt\":1,\"clientId\":\"mix-1\"}").decode())')"

STRICT_BODY="$(python3 -c 'import json,sys; print(json.dumps({"agent_id":sys.argv[1],"payload":sys.argv[2],"logical_id":"mix-strict-1"}))' "$B_ID" "$PAYLOAD")"
STRICT_RESP="$(api "$A_TOKEN" 18711 /direct/send POST "$STRICT_BODY")"
echo "strict: $STRICT_RESP"
echo "$STRICT_RESP" | python3 -c '
import json,sys
d=json.load(sys.stdin)
assert d.get("ok") is False, d
assert d.get("error")=="recipient_ack_semantics_unavailable", d
print("mixed-version product send 409 as specified")
'

# logical_id is refused on the opt-out path (400 logical_id_requires_durable_ack).
OPT_OUT_BODY="$(python3 -c 'import json,sys; print(json.dumps({"agent_id":sys.argv[1],"payload":sys.argv[2],"require_durable_app_ack":False}))' "$B_ID" "$PAYLOAD")"
OPT_OUT_RESP="$(api "$A_TOKEN" 18711 /direct/send POST "$OPT_OUT_BODY")"
echo "opt-out: $OPT_OUT_RESP"
echo "$OPT_OUT_RESP" | python3 -c '
import json,sys
d=json.load(sys.stdin)
assert d.get("ok") is True, d
print("mixed-version opt-out DM smoke passed:", d.get("path"), d.get("request_id"))
'
