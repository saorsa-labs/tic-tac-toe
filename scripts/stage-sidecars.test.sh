#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
TEST_ROOT="$(mktemp -d)"
trap 'rm -rf "$TEST_ROOT"' EXIT

REPO_ROOT="$TEST_ROOT/repo"
X0X_ROOT="$TEST_ROOT/x0x"
FAKE_BIN="$TEST_ROOT/bin"
TRIPLE="aarch64-apple-darwin"
mkdir -p "$REPO_ROOT/desktop/src-tauri/binaries" "$X0X_ROOT/target/release" "$FAKE_BIN"

printf '#!/bin/sh\nprintf "host: aarch64-apple-darwin\\n"\n' > "$FAKE_BIN/rustc"
printf '#!/bin/sh\nexit 0\n' > "$FAKE_BIN/cargo"
printf '#!/bin/sh\necho x0xd-test\n' > "$X0X_ROOT/target/release/x0xd"
chmod +x "$FAKE_BIN/rustc" "$FAKE_BIN/cargo" "$X0X_ROOT/target/release/x0xd"

run_stage() {
  PATH="$FAKE_BIN:$PATH" \
    STAGE_SIDECARS_REPO_ROOT="$REPO_ROOT" \
    X0X_DIR="$X0X_ROOT" \
    PROFILE=release \
    "$SCRIPT_DIR/stage-sidecars.sh"
}

run_stage

expected_placeholder="$TEST_ROOT/expected-placeholder"
printf '#!/bin/sh\nexit 0\n' > "$expected_placeholder"
for name in buzz-acp buzz-agent buzz-dev-mcp buzz; do
  path="$REPO_ROOT/desktop/src-tauri/binaries/$name-$TRIPLE"
  cmp "$expected_placeholder" "$path"
  [[ -x "$path" ]]
done
cmp "$X0X_ROOT/target/release/x0xd" \
  "$REPO_ROOT/desktop/src-tauri/binaries/x0xd-$TRIPLE"

# Idempotent staging accepts the exact generated placeholders.
run_stage

# A real or otherwise different binary is user-owned; staging must fail closed
# instead of silently replacing it.
protected="$REPO_ROOT/desktop/src-tauri/binaries/buzz-agent-$TRIPLE"
printf 'real binary\n' > "$protected"
if run_stage >"$TEST_ROOT/stdout" 2>"$TEST_ROOT/stderr"; then
  echo "expected non-placeholder overwrite refusal" >&2
  exit 1
fi
grep -F "refusing to overwrite non-placeholder external binary" "$TEST_ROOT/stderr"
cmp "$protected" <(printf 'real binary\n')

# Even a near-match with extra trailing bytes is not accepted as the canonical
# release placeholder.
printf '#!/bin/sh\nexit 0\n\n' > "$protected"
if run_stage >"$TEST_ROOT/stdout" 2>"$TEST_ROOT/stderr"; then
  echo "expected non-canonical placeholder refusal" >&2
  exit 1
fi
grep -F "refusing to overwrite non-placeholder external binary" "$TEST_ROOT/stderr"

echo "stage-sidecars tests passed"
