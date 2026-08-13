#!/usr/bin/env bash
# Mixed-version DM smoke (ttt #12): bundled sidecar → released 0.37.2 daemon
# must deliver. A 409 recipient_ack_semantics_unavailable is a hard fail.
#
# Isolated loopback pair: --no-hard-coded-bootstrap, no prod mesh.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
# shellcheck source=sidecar-validation.sh
source "$SCRIPT_DIR/sidecar-validation.sh"

BUNDLED_X0XD="${BUNDLED_X0XD:-$REPO_ROOT/desktop/src-tauri/binaries/x0xd-aarch64-apple-darwin}"
RELEASED_X0XD="${RELEASED_X0XD:-}"

if [[ ! -x "$BUNDLED_X0XD" ]]; then
  echo "FATAL: bundled x0xd missing at $BUNDLED_X0XD (run stage-sidecars or fetch-official-x0xd)" >&2
  exit 2
fi
reject_campaign_x0xd "$BUNDLED_X0XD"

if [[ -z "$RELEASED_X0XD" ]]; then
  RELEASED_X0XD="$("$SCRIPT_DIR/fetch-official-x0xd.sh")"
fi
assert_official_x0xd_pin "$RELEASED_X0XD"

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

A_PID="$(start_node "$BUNDLED_X0XD" mix-a 18711 "$A_DIR")"
B_PID="$(start_node "$RELEASED_X0XD" mix-b 18712 "$B_DIR")"

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
    curl -sS -m 20 -X "$method" -H "Authorization: Bearer $token" \
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
SEND_BODY="$(python3 -c 'import json,sys; print(json.dumps({"agent_id":sys.argv[1],"payload":sys.argv[2]}))' "$B_ID" "$PAYLOAD")"
RESP="$(api "$A_TOKEN" 18711 /direct/send POST "$SEND_BODY")"
echo "$RESP"
if echo "$RESP" | grep -q 'recipient_ack_semantics_unavailable'; then
  echo "FATAL: bundled sidecar 409'd a released daemon (ttt #12)" >&2
  exit 1
fi
echo "$RESP" | python3 -c 'import json,sys; d=json.load(sys.stdin); assert d.get("ok") is True, d; print("mixed-version DM smoke passed:", d.get("path"), d.get("request_id"))'
