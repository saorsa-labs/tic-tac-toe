#!/usr/bin/env bash
# Stage the x0xd sidecar for Tauri's active target triple.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
X0X_DIR="${X0X_DIR:-$(cd "$REPO_ROOT/../x0x" 2>/dev/null && pwd || true)}"
PROFILE="${PROFILE:-release}"

if [[ -z "${X0X_DIR:-}" || ! -d "$X0X_DIR" ]]; then
  echo "FATAL: x0x repo not found (set X0X_DIR)" >&2
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

printf '[stage-sidecars] building x0xd (%s) from %s\n' "$PROFILE" "$X0X_DIR" >&2
(cd "$X0X_DIR" && cargo build --profile "$PROFILE" --bin x0xd)

SOURCE="$X0X_DIR/target/$SUB/x0xd$EXT"
DEST="$REPO_ROOT/desktop/src-tauri/binaries/x0xd-$TRIPLE$EXT"
if [[ ! -x "$SOURCE" ]]; then
  echo "FATAL: built x0xd not found at $SOURCE" >&2
  exit 3
fi

mkdir -p "$(dirname "$DEST")"
cp "$SOURCE" "$DEST"
chmod +x "$DEST"
printf '[stage-sidecars] staged %s\n' "$DEST" >&2
