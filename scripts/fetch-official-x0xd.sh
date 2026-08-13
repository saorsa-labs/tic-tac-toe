#!/usr/bin/env bash
# Fetch the official signed x0x macos-arm64 release asset and print the
# extracted x0xd path. Never uses a local cargo build (ttt #12).

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=sidecar-validation.sh
source "$SCRIPT_DIR/sidecar-validation.sh"

CACHE_DIR="${X0XD_RELEASE_CACHE:-${TMPDIR:-/tmp}/x0x-official-${PINNED_X0XD_VERSION}}"
TARBALL="$CACHE_DIR/x0x-macos-arm64.tar.gz"
EXTRACT_DIR="$CACHE_DIR/extract"
DEST="$EXTRACT_DIR/x0x-macos-arm64/x0xd"
URL="https://github.com/saorsa-labs/x0x/releases/download/v${PINNED_X0XD_VERSION}/x0x-macos-arm64.tar.gz"

mkdir -p "$CACHE_DIR"
if [[ ! -f "$TARBALL" ]]; then
  curl -fsSL -o "$TARBALL.partial" "$URL"
  mv "$TARBALL.partial" "$TARBALL"
fi

actual="$(shasum -a 256 "$TARBALL" | awk '{print $1}')"
if [[ "$actual" != "$PINNED_X0XD_MACOS_ARM64_TARBALL_SHA256" ]]; then
  echo "FATAL: downloaded tarball sha256 $actual != pinned $PINNED_X0XD_MACOS_ARM64_TARBALL_SHA256" >&2
  exit 5
fi

rm -rf "$EXTRACT_DIR"
mkdir -p "$EXTRACT_DIR"
tar -xzf "$TARBALL" -C "$EXTRACT_DIR"
if [[ ! -x "$DEST" ]]; then
  echo "FATAL: official archive did not contain x0xd at $DEST" >&2
  exit 5
fi
assert_official_x0xd_pin "$DEST"
printf '%s\n' "$DEST"
