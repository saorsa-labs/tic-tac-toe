#!/usr/bin/env bash
# Stage the x0xd sidecar for Tauri's active target triple.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="${STAGE_SIDECARS_REPO_ROOT:-$(cd "$SCRIPT_DIR/.." && pwd)}"
X0X_DIR="${X0X_DIR:-$(cd "$REPO_ROOT/../x0x" 2>/dev/null && pwd || true)}"
PROFILE="${PROFILE:-release}"
PLACEHOLDER_NAMES=(buzz-acp buzz-agent buzz-dev-mcp buzz)

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

if [[ -n "$EXT" ]]; then
  echo "FATAL: legacy external-bin placeholders are not implemented for $TRIPLE" >&2
  exit 4
fi

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

# The legacy agent binaries remain in Tauri's externalBin list because the
# imported desktop still resolves those sidecar names. The released app ships
# inert POSIX executables for them. Create the same deterministic placeholder,
# but never overwrite a non-placeholder file: a developer may have staged a
# real binary intentionally, and silently replacing it would be destructive.
PLACEHOLDER_CONTENT=$'#!/bin/sh\nexit 0\n'
PLACEHOLDER_SIZE=${#PLACEHOLDER_CONTENT}
for name in "${PLACEHOLDER_NAMES[@]}"; do
  placeholder="$REPO_ROOT/desktop/src-tauri/binaries/$name-$TRIPLE"
  if [[ -e "$placeholder" ]]; then
    if [[ ! -f "$placeholder" \
      || "$(wc -c < "$placeholder")" -ne "$PLACEHOLDER_SIZE" \
      || "$(<"$placeholder")" != "${PLACEHOLDER_CONTENT%$'\n'}" ]]; then
      echo "FATAL: refusing to overwrite non-placeholder external binary: $placeholder" >&2
      exit 5
    fi
  else
    printf '%s' "$PLACEHOLDER_CONTENT" > "$placeholder"
  fi
  chmod +x "$placeholder"
  printf '[stage-sidecars] staged placeholder %s\n' "$placeholder" >&2
done
