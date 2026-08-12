#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PACKAGER="$SCRIPT_DIR/package-macos-dmg.sh"
TEST_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/tic-tac-toe-dmg-test.XXXXXX")"
trap 'rm -rf "$TEST_ROOT"' EXIT

bash -n "$PACKAGER"
bash -n "$SCRIPT_DIR/dist.sh"

if [[ "$(uname -s)" != "Darwin" ]]; then
    echo "package-macos-dmg tests skipped: macOS-only validation"
    exit 0
fi

BROKEN_APP="$TEST_ROOT/tic-tac-toe.app"
mkdir -p "$BROKEN_APP/Contents/MacOS"
cat > "$BROKEN_APP/Contents/Info.plist" <<'EOF'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleExecutable</key>
  <string>buzz-desktop</string>
</dict>
</plist>
EOF
for executable in buzz-desktop x0xd buzz-acp buzz-agent buzz-dev-mcp buzz; do
    cp /usr/bin/true "$BROKEN_APP/Contents/MacOS/$executable"
done

if "$PACKAGER" --app "$BROKEN_APP" --validate-only \
    >"$TEST_ROOT/stdout" 2>"$TEST_ROOT/stderr"; then
    echo "expected an app without Resources to be rejected" >&2
    exit 1
fi
grep -F "Contents/Resources is missing or empty" "$TEST_ROOT/stderr"

mkdir -p "$BROKEN_APP/Contents/Resources"
printf 'fixture\n' > "$BROKEN_APP/Contents/Resources/index.html"
if "$PACKAGER" --app "$BROKEN_APP" --validate-only \
    >"$TEST_ROOT/stdout" 2>"$TEST_ROOT/stderr"; then
    echo "expected an unsigned app to be rejected" >&2
    exit 1
fi

if [[ -n "${SIGNED_APP_FIXTURE:-}" ]]; then
    "$PACKAGER" --app "$SIGNED_APP_FIXTURE" --validate-only \
        >"$TEST_ROOT/signed-app-receipt"
    grep -F "Executable SHA-256 receipt:" "$TEST_ROOT/signed-app-receipt"
    grep -F "buzz-desktop" "$TEST_ROOT/signed-app-receipt"
    grep -F "x0xd" "$TEST_ROOT/signed-app-receipt"
fi

echo "package-macos-dmg tests passed"
