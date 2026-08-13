#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=sidecar-validation.sh
source "$SCRIPT_DIR/sidecar-validation.sh"

TEST_ROOT="$(mktemp -d)"
trap 'rm -rf "$TEST_ROOT"' EXIT

clean="$TEST_ROOT/clean-x0xd"
printf '#!/bin/sh\necho x0xd 0.37.2\n' > "$clean"
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

echo "sidecar campaign denylist tests passed"
