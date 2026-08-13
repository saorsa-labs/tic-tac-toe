#!/usr/bin/env bash
# Stage native managed-agent sidecars and x0xd for Tauri's active target triple.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="${STAGE_SIDECARS_REPO_ROOT:-$(cd "$SCRIPT_DIR/.." && pwd)}"
X0X_DIR="${X0X_DIR:-$(cd "$REPO_ROOT/../x0x" 2>/dev/null && pwd || true)}"
PROFILE="${PROFILE:-release}"

# shellcheck source=sidecar-validation.sh
source "$SCRIPT_DIR/sidecar-validation.sh"

X0XD_SOURCE="${X0XD_SOURCE:-official}"
if [[ "$X0XD_SOURCE" == "local-build" && ( -z "${X0X_DIR:-}" || ! -d "$X0X_DIR" ) ]]; then
  echo "FATAL: x0x repo not found (set X0X_DIR) for X0XD_SOURCE=local-build" >&2
  exit 2
fi

TRIPLE="$(rustc -vV | sed -n 's/^host: //p')"
if [[ -z "$TRIPLE" ]]; then
  echo "FATAL: could not determine target triple from rustc -vV" >&2
  exit 2
fi

case "$PROFILE" in
  release) SUB="release" ;;
  dev) SUB="debug" ;;
  *) SUB="$PROFILE" ;;
esac
case "$TRIPLE" in
  *-windows-*) EXT=".exe" ;;
  *) EXT="" ;;
esac

BINARY_DIR="$REPO_ROOT/desktop/src-tauri/binaries"
mkdir -p "$BINARY_DIR"

printf '[stage-sidecars] building native agent sidecars (%s) from %s\n' \
  "$PROFILE" "$REPO_ROOT" >&2
(cd "$REPO_ROOT" && cargo build --profile "$PROFILE" --locked \
  --bin buzz-acp --bin buzz-agent --bin buzz-x0x-mcp)

for name in buzz-acp buzz-agent buzz-x0x-mcp; do
  source_path="$REPO_ROOT/target/$SUB/$name$EXT"
  destination="$BINARY_DIR/$name-$TRIPLE$EXT"
  if [[ ! -x "$source_path" ]]; then
    echo "FATAL: built current-source sidecar not found at $source_path" >&2
    exit 3
  fi
  cp "$source_path" "$destination"
  chmod +x "$destination"
  printf '[stage-sidecars] staged %s\n' "$destination" >&2
done

validate_managed_agent_sidecars "$BINARY_DIR" "$TRIPLE"

DEST="$BINARY_DIR/x0xd-$TRIPLE$EXT"
case "$X0XD_SOURCE" in
  official)
    if [[ "$TRIPLE" != "aarch64-apple-darwin" ]]; then
      echo "FATAL: official x0xd fetch is pinned for aarch64-apple-darwin (ttt #12); set X0XD_SOURCE=local-build for $TRIPLE" >&2
      exit 2
    fi
    printf '[stage-sidecars] fetching official x0xd v%s\n' "$PINNED_X0XD_VERSION" >&2
    SOURCE="$("$SCRIPT_DIR/fetch-official-x0xd.sh")"
    ;;
  local-build)
    printf '[stage-sidecars] building x0xd (%s) from %s\n' "$PROFILE" "$X0X_DIR" >&2
    (cd "$X0X_DIR" && cargo build --profile "$PROFILE" --locked --bin x0xd)
    SOURCE="$X0X_DIR/target/$SUB/x0xd$EXT"
    ;;
  *)
    echo "FATAL: X0XD_SOURCE must be official or local-build (got $X0XD_SOURCE)" >&2
    exit 2
    ;;
esac
if [[ ! -x "$SOURCE" ]]; then
  echo "FATAL: x0xd not found at $SOURCE" >&2
  exit 3
fi

cp "$SOURCE" "$DEST"
chmod +x "$DEST"
reject_campaign_x0xd "$DEST"
if [[ "$X0XD_SOURCE" == "official" ]]; then
  assert_official_x0xd_pin "$DEST"
fi
printf '[stage-sidecars] staged %s\n' "$DEST" >&2
