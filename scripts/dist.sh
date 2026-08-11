#!/bin/bash
# Build a signed macOS tic-tac-toe .app with the release x0xd sidecar.
#
# Usage: scripts/dist.sh [--skip-notarization]
#
# Produces:
#   dist/tic-tac-toe-v<X0XD_VERSION>-aarch64.tar.gz
#   dist/tic-tac-toe-v<X0XD_VERSION>-aarch64.dmg
#
# The archive contains:
#   tic-tac-toe.app/       - Tauri-built, signed application bundle
#   run-tic-tac-toe.sh     - launcher with x0xd auto-detection
#   VERSION                - exact build and signing manifest
#
# A distributable macOS build must be notarized. The explicit
# --skip-notarization escape hatch exists for local/two-machine acceptance.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
X0X_DIR="${X0X_DIR:-$ROOT/../x0x}"
DESKTOP_DIR="$ROOT/desktop"
DIST_DIR="$ROOT/dist"
APP_BUNDLE_NAME="tic-tac-toe.app"
TARGET_TRIPLE="$(rustc -vV | awk '/^host:/ { print $2 }')"
SKIP_NOTARIZATION=false

# shellcheck source=sidecar-validation.sh
source "$ROOT/scripts/sidecar-validation.sh"

usage() {
    echo "Usage: $0 [--skip-notarization]"
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --release)
            # Kept for compatibility with the original script. Distribution
            # builds are always release builds now.
            shift
            ;;
        --skip-notarization)
            SKIP_NOTARIZATION=true
            shift
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            usage >&2
            echo "Unknown option: $1" >&2
            exit 2
            ;;
    esac
done

if [[ "$(uname -s)" != "Darwin" ]]; then
    echo "macOS distribution builds must run on macOS" >&2
    exit 1
fi

if [[ "$TARGET_TRIPLE" != "aarch64-apple-darwin" ]]; then
    echo "Expected an Apple Silicon host, got: $TARGET_TRIPLE" >&2
    exit 1
fi

notarization_configured=false
if [[ -n "${APPLE_ID:-}" && -n "${APPLE_PASSWORD:-}" && -n "${APPLE_TEAM_ID:-}" ]]; then
    notarization_configured=true
elif [[ -n "${APPLE_API_KEY:-}" && -n "${APPLE_API_ISSUER:-}" && -n "${APPLE_API_KEY_PATH:-}" ]]; then
    notarization_configured=true
fi

notarytool_submit() {
    local artifact="$1"
    if [[ -n "${APPLE_ID:-}" ]]; then
        xcrun notarytool submit "$artifact" \
            --apple-id "$APPLE_ID" \
            --password "$APPLE_PASSWORD" \
            --team-id "$APPLE_TEAM_ID" \
            --wait
    else
        xcrun notarytool submit "$artifact" \
            --key "$APPLE_API_KEY_PATH" \
            --key-id "$APPLE_API_KEY" \
            --issuer "$APPLE_API_ISSUER" \
            --wait
    fi
}

if ! $notarization_configured && ! $SKIP_NOTARIZATION; then
    echo "Notarization credentials are not configured." >&2
    echo "Set APPLE_ID/APPLE_PASSWORD/APPLE_TEAM_ID or" >&2
    echo "APPLE_API_KEY/APPLE_API_ISSUER/APPLE_API_KEY_PATH." >&2
    echo "For local acceptance only, pass --skip-notarization." >&2
    exit 1
fi

if [[ -z "${APPLE_SIGNING_IDENTITY:-}" ]]; then
    APPLE_SIGNING_IDENTITY="$(
        security find-identity -v -p codesigning \
            | awk -F'"' '/Developer ID Application:/ { print $2; exit }'
    )"
    export APPLE_SIGNING_IDENTITY
fi

if [[ -z "${APPLE_SIGNING_IDENTITY:-}" ]]; then
    echo "No valid Developer ID Application signing identity was found." >&2
    exit 1
fi

echo "Validating managed-agent sidecars and building x0xd release sidecar"
PROFILE=release X0X_DIR="$X0X_DIR" "$ROOT/scripts/stage-sidecars.sh"

X0XD_OUT="$X0X_DIR/target/release/x0xd"
if [[ ! -x "$X0XD_OUT" ]]; then
    echo "x0xd build failed: $X0XD_OUT is missing" >&2
    exit 1
fi

X0XD_VERSION="$($X0XD_OUT --version)"
if [[ "$X0XD_VERSION" =~ ([0-9]+\.[0-9]+\.[0-9]+) ]]; then
    X0XD_SEMVER="${BASH_REMATCH[1]}"
else
    echo "Could not parse x0xd version from: $X0XD_VERSION" >&2
    exit 1
fi

echo "Building and signing the Tauri application"
(
    cd "$DESKTOP_DIR"
    corepack pnpm exec tauri build --bundles app --ci
)

APP_BUNDLE="$DESKTOP_DIR/src-tauri/target/release/bundle/macos/$APP_BUNDLE_NAME"
if [[ ! -d "$APP_BUNDLE" ]]; then
    echo "Tauri app bundle not found: $APP_BUNDLE" >&2
    exit 1
fi

validate_managed_agent_sidecars "$APP_BUNDLE/Contents/MacOS" "$TARGET_TRIPLE" ""

BUNDLED_X0XD="$APP_BUNDLE/Contents/MacOS/x0xd"
if [[ "$($BUNDLED_X0XD --version)" != "$X0XD_VERSION" ]]; then
    echo "Bundled x0xd version does not match the built sidecar" >&2
    exit 1
fi

codesign --verify --deep --strict --verbose=2 "$APP_BUNDLE"
spctl --assess --type execute --verbose=2 "$APP_BUNDLE"

notarized="no"
if $notarization_configured; then
    NOTARY_WORK_DIR="$(mktemp -d "${TMPDIR:-/tmp}/tic-tac-toe-notary.XXXXXX")"
    NOTARY_APP_ZIP="$NOTARY_WORK_DIR/$APP_BUNDLE_NAME.zip"
    ditto -c -k --keepParent "$APP_BUNDLE" "$NOTARY_APP_ZIP"
    notarytool_submit "$NOTARY_APP_ZIP"
    xcrun stapler staple "$APP_BUNDLE"
    xcrun stapler validate "$APP_BUNDLE"
    codesign --verify --deep --strict --verbose=2 "$APP_BUNDLE"
    spctl --assess --type execute --verbose=2 "$APP_BUNDLE"
    rm -rf "$NOTARY_WORK_DIR"
    notarized="yes"
fi

PKG_NAME="tic-tac-toe-v${X0XD_SEMVER}-aarch64"
PKG_DIR="$DIST_DIR/$PKG_NAME"
TARBALL="$DIST_DIR/$PKG_NAME.tar.gz"
TARBALL_TMP="$TARBALL.tmp"
DMG="$DIST_DIR/$PKG_NAME.dmg"
DMG_BUILD="$DIST_DIR/.$PKG_NAME.building.dmg"

rm -rf "$PKG_DIR"
mkdir -p "$PKG_DIR"
ditto "$APP_BUNDLE" "$PKG_DIR/$APP_BUNDLE_NAME"
install -m 755 "$ROOT/scripts/run-tic-tac-toe.sh" "$PKG_DIR/run-tic-tac-toe.sh"

cat > "$PKG_DIR/VERSION" <<EOF
tic-tac-toe: $(git -C "$ROOT" describe --tags --always --dirty)
x0xd: $X0XD_VERSION
target: $TARGET_TRIPLE
built: $(date -u +%Y-%m-%dT%H:%M:%SZ)
signing_identity: $APPLE_SIGNING_IDENTITY
notarized: $notarized
EOF

codesign --verify --deep --strict --verbose=2 "$PKG_DIR/$APP_BUNDLE_NAME"
rm -f "$TARBALL_TMP"
tar -czf "$TARBALL_TMP" -C "$PKG_DIR" "$APP_BUNDLE_NAME" run-tic-tac-toe.sh VERSION
mv "$TARBALL_TMP" "$TARBALL"

rm -f "$DMG_BUILD"
"$ROOT/scripts/package-macos-dmg.sh" \
    --app "$APP_BUNDLE" \
    --output "$DMG_BUILD" \
    --signing-identity "$APPLE_SIGNING_IDENTITY" \
    --background "$DESKTOP_DIR/src-tauri/icons/dmg-background.png"

if $notarization_configured; then
    notarytool_submit "$DMG_BUILD"
    xcrun stapler staple "$DMG_BUILD"
    xcrun stapler validate "$DMG_BUILD"
    codesign --verify --deep --strict --verbose=2 "$DMG_BUILD"
    spctl --assess --type open --verbose=2 "$DMG_BUILD"
fi
mv "$DMG_BUILD" "$DMG"

echo
echo "Built: $TARBALL"
echo "SHA-256: $(shasum -a 256 "$TARBALL" | awk '{ print $1 }')"
echo "Built: $DMG"
echo "SHA-256: $(shasum -a 256 "$DMG" | awk '{ print $1 }')"
if ! $notarization_configured; then
    echo "Warning: these artifacts are signed but not notarized; use them for acceptance only."
fi
