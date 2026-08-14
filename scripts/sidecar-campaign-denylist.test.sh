#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=sidecar-validation.sh
source "$SCRIPT_DIR/sidecar-validation.sh"

TEST_ROOT="$(mktemp -d)"
trap 'rm -rf "$TEST_ROOT"' EXIT

clean="$TEST_ROOT/clean-x0xd"
printf '#!/bin/sh\necho x0xd 0.37.4\n' > "$clean"
chmod +x "$clean"
reject_campaign_x0xd "$clean"

dirty="$TEST_ROOT/campaign-x0xd"
printf '#!/bin/sh\necho recipient_ack_semantics_unavailable\n' > "$dirty"
chmod +x "$dirty"
if reject_campaign_x0xd "$dirty" >"$TEST_ROOT/out" 2>"$TEST_ROOT/err"; then
  echo "expected campaign-string rejection" >&2
  exit 1
fi
grep -F "unreleased campaign string 'recipient_ack_semantics_unavailable'" "$TEST_ROOT/err"

# Identity (version + denylist) is what the signed bundle can still prove.
# The sha256 pin is for the unsigned official asset only — codesign changes
# the bytes (and --remove-signature does not restore them).
assert_official_x0xd_identity "$clean"
if assert_official_x0xd_pin "$clean" >"$TEST_ROOT/pin-out" 2>"$TEST_ROOT/pin-err"; then
  echo "expected unsigned sha256 pin to reject a stub binary" >&2
  exit 1
fi
grep -F "sha256" "$TEST_ROOT/pin-err" >/dev/null

wrong="$TEST_ROOT/wrong-ver"
printf '#!/bin/sh\necho x0xd 0.37.0\n' > "$wrong"
chmod +x "$wrong"
if assert_official_x0xd_identity "$wrong" >"$TEST_ROOT/ver-out" 2>"$TEST_ROOT/ver-err"; then
  echo "expected identity check to reject the wrong version" >&2
  exit 1
fi
grep -F "expected $PINNED_X0XD_VERSION" "$TEST_ROOT/ver-err"

echo "sidecar campaign denylist tests passed"
