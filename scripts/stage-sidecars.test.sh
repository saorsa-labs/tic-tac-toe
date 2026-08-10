#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
TEST_ROOT="$(mktemp -d)"
trap 'rm -rf "$TEST_ROOT"' EXIT

REPO_ROOT="$TEST_ROOT/repo"
X0X_ROOT="$TEST_ROOT/x0x"
FAKE_BIN="$TEST_ROOT/bin"
FAKE_FILE_BIN="$TEST_ROOT/fake-file-bin"
BASE_PATH="$PATH"
TRIPLE="$(rustc -vV | sed -n 's/^host: //p')"
mkdir -p "$REPO_ROOT/desktop/src-tauri/binaries" "$X0X_ROOT/target/release" \
  "$REPO_ROOT/target/release" "$FAKE_BIN" "$FAKE_FILE_BIN"

printf '#!/bin/sh\nexit 0\n' > "$FAKE_BIN/cargo"
printf '#!/bin/sh\necho x0xd-test\n' > "$X0X_ROOT/target/release/x0xd"
printf '#!/bin/sh\nprintf "%%s\\n" "${FAKE_FILE_DESCRIPTION:?}"\n' > "$FAKE_FILE_BIN/file"
chmod +x "$FAKE_BIN/cargo" "$FAKE_FILE_BIN/file" "$X0X_ROOT/target/release/x0xd"

case "$TRIPLE" in
  aarch64-apple-darwin) MATCHING_DESCRIPTION='Mach-O 64-bit executable arm64' ;;
  x86_64-apple-darwin) MATCHING_DESCRIPTION='Mach-O 64-bit executable x86_64' ;;
  aarch64-*-linux-*) MATCHING_DESCRIPTION='ELF 64-bit LSB pie executable, ARM aarch64' ;;
  x86_64-*-linux-*) MATCHING_DESCRIPTION='ELF 64-bit LSB pie executable, x86-64' ;;
  *)
    echo "unsupported test host triple: $TRIPLE" >&2
    exit 1
    ;;
esac

NATIVE_FIXTURE="$(type -P true)"
if [[ -z "$NATIVE_FIXTURE" || ! -x "$NATIVE_FIXTURE" ]]; then
  echo "could not find a native system executable fixture" >&2
  exit 1
fi

for name in buzz-acp buzz-agent buzz-x0x-mcp; do
  cp "$NATIVE_FIXTURE" "$REPO_ROOT/target/release/$name"
  chmod +x "$REPO_ROOT/target/release/$name"
done

run_stage() {
  local path_prefix="${1:-$FAKE_BIN}"
  PATH="$path_prefix:$BASE_PATH" \
    STAGE_SIDECARS_REPO_ROOT="$REPO_ROOT" \
    X0X_DIR="$X0X_ROOT" \
    PROFILE=release \
    "$SCRIPT_DIR/stage-sidecars.sh"
}

run_stage

# Acceptance uses only the host's native system executable as an isolated
# fixture; no binary from an installed or previous Buzz build is imported.
for name in buzz-acp buzz-agent buzz-x0x-mcp; do
  path="$REPO_ROOT/desktop/src-tauri/binaries/$name-$TRIPLE"
  cmp "$NATIVE_FIXTURE" "$path"
  [[ -x "$path" ]]
done
[[ ! -e "$REPO_ROOT/desktop/src-tauri/binaries/buzz-dev-mcp-$TRIPLE" ]]
[[ ! -e "$REPO_ROOT/desktop/src-tauri/binaries/buzz-$TRIPLE" ]]
cmp "$X0X_ROOT/target/release/x0xd" \
  "$REPO_ROOT/desktop/src-tauri/binaries/x0xd-$TRIPLE"

# Idempotent staging accepts the real target-native fixtures.
run_stage

# Sole-catching placeholder mutation: shadow `file` with a matching native
# description so executability, presence, and target checks all pass. The exact
# 17-byte content predicate must independently reject the inert sidecar.
mutated="$REPO_ROOT/target/release/buzz-agent"
printf '#!/bin/sh\nexit 0\n' > "$mutated"
chmod +x "$mutated"
if FAKE_FILE_DESCRIPTION="$MATCHING_DESCRIPTION" \
  run_stage "$FAKE_FILE_BIN:$FAKE_BIN" >"$TEST_ROOT/stdout" 2>"$TEST_ROOT/stderr"; then
  echo "expected exact placeholder rejection" >&2
  exit 1
fi
grep -F "refusing inert 17-byte external-bin placeholder" "$TEST_ROOT/stderr"
cmp "$mutated" <(printf '#!/bin/sh\nexit 0\n')

# Target validation is also fail-closed. Use the real fixture while shadowing
# `file` with an incompatible executable description.
cp "$NATIVE_FIXTURE" "$mutated"
chmod +x "$mutated"
if FAKE_FILE_DESCRIPTION='POSIX shell script, ASCII text executable' \
  run_stage "$FAKE_FILE_BIN:$FAKE_BIN" >"$TEST_ROOT/stdout" 2>"$TEST_ROOT/stderr"; then
  echo "expected non-native binary rejection" >&2
  exit 1
fi
grep -F "external binary is not target-native for $TRIPLE" "$TEST_ROOT/stderr"

echo "stage-sidecars tests passed"
