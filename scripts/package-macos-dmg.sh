#!/bin/bash
# Package an already-built and signed tic-tac-toe.app without rebuilding it.
#
# The source app is treated as the release object: every regular file is hashed
# before staging and again from the finished image.  This prevents a DMG tool
# from silently substituting a stale or incomplete app bundle.
set -euo pipefail

APP_BUNDLE=""
OUTPUT_DMG=""
SIGNING_IDENTITY=""
BACKGROUND=""
VALIDATE_ONLY=false

usage() {
    cat <<'EOF'
Usage: package-macos-dmg.sh --app <tic-tac-toe.app> [options]

Options:
  --output <path.dmg>       Required unless --validate-only is used
  --signing-identity <id>   Required unless --validate-only is used
  --background <path.png>   Optional Finder window background
  --validate-only           Validate and print the executable receipt only
  -h, --help                Show this help

Set DMG_SKIP_FINDER_LAYOUT=1 only for non-interactive CI. The resulting image
still contains the background, but does not have Finder icon placement metadata.
EOF
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --app)
            APP_BUNDLE="${2:-}"
            shift 2
            ;;
        --output)
            OUTPUT_DMG="${2:-}"
            shift 2
            ;;
        --signing-identity)
            SIGNING_IDENTITY="${2:-}"
            shift 2
            ;;
        --background)
            BACKGROUND="${2:-}"
            shift 2
            ;;
        --validate-only)
            VALIDATE_ONLY=true
            shift
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            echo "Unknown option: $1" >&2
            usage >&2
            exit 2
            ;;
    esac
done

if [[ "$(uname -s)" != "Darwin" ]]; then
    echo "macOS disk images must be packaged on macOS" >&2
    exit 1
fi

if [[ -z "$APP_BUNDLE" || ! -d "$APP_BUNDLE" ]]; then
    echo "App bundle not found: ${APP_BUNDLE:-<unset>}" >&2
    exit 1
fi

if [[ "$(basename "$APP_BUNDLE")" != "tic-tac-toe.app" ]]; then
    echo "Expected a tic-tac-toe.app bundle, got: $APP_BUNDLE" >&2
    exit 1
fi

if ! $VALIDATE_ONLY; then
    if [[ -z "$OUTPUT_DMG" || "$OUTPUT_DMG" != *.dmg ]]; then
        echo "--output must name a .dmg file" >&2
        exit 1
    fi
    if [[ -z "$SIGNING_IDENTITY" ]]; then
        echo "--signing-identity is required" >&2
        exit 1
    fi
fi

if [[ -n "$BACKGROUND" && ! -f "$BACKGROUND" ]]; then
    echo "DMG background not found: $BACKGROUND" >&2
    exit 1
fi

PLIST_BUDDY=/usr/libexec/PlistBuddy
INFO_PLIST="$APP_BUNDLE/Contents/Info.plist"
RESOURCES_DIR="$APP_BUNDLE/Contents/Resources"
MACOS_DIR="$APP_BUNDLE/Contents/MacOS"
REQUIRED_EXECUTABLES=(buzz-desktop x0xd buzz-acp buzz-agent buzz-x0x-mcp)

if [[ ! -f "$INFO_PLIST" ]]; then
    echo "App bundle is incomplete: Contents/Info.plist is missing" >&2
    exit 1
fi

if [[ ! -d "$RESOURCES_DIR" ]] || [[ -z "$(find "$RESOURCES_DIR" -type f -print -quit)" ]]; then
    echo "App bundle is incomplete: Contents/Resources is missing or empty" >&2
    exit 1
fi

bundle_executable="$($PLIST_BUDDY -c 'Print :CFBundleExecutable' "$INFO_PLIST")"
if [[ "$bundle_executable" != "buzz-desktop" ]]; then
    echo "Unexpected CFBundleExecutable: $bundle_executable" >&2
    exit 1
fi

for executable in "${REQUIRED_EXECUTABLES[@]}"; do
    if [[ ! -x "$MACOS_DIR/$executable" ]]; then
        echo "App bundle is incomplete: executable Contents/MacOS/$executable is missing" >&2
        exit 1
    fi
done

codesign --verify --deep --strict --verbose=2 "$APP_BUNDLE"

write_bundle_manifest() {
    local bundle="$1"
    local destination="$2"
    local relative_path
    local digest

    : > "$destination"
    while IFS= read -r relative_path; do
        digest="$(shasum -a 256 "$bundle/$relative_path" | awk '{print $1}')"
        printf '%s  %s\n' "$digest" "$relative_path" >> "$destination"
    done < <(
        cd "$bundle"
        find . -type f -print | LC_ALL=C sort
    )
}

print_executable_receipt() {
    local bundle="$1"
    local executable
    local digest

    echo "Executable SHA-256 receipt:"
    for executable in "${REQUIRED_EXECUTABLES[@]}"; do
        digest="$(shasum -a 256 "$bundle/Contents/MacOS/$executable" | awk '{print $1}')"
        printf '  %s  %s\n' "$digest" "$executable"
    done
}

print_executable_receipt "$APP_BUNDLE"

if $VALIDATE_ONLY; then
    exit 0
fi

OUTPUT_DIR="$(cd "$(dirname "$OUTPUT_DMG")" && pwd)"
OUTPUT_NAME="$(basename "$OUTPUT_DMG")"
WORK_DIR="$(mktemp -d "${TMPDIR:-/tmp}/tic-tac-toe-dmg.XXXXXX")"
STAGING_DIR="$WORK_DIR/staging"
SOURCE_MANIFEST="$WORK_DIR/source.sha256"
STAGED_MANIFEST="$WORK_DIR/staged.sha256"
MOUNTED_MANIFEST="$WORK_DIR/mounted.sha256"
RW_IMAGE="$WORK_DIR/tic-tac-toe-rw.dmg"
FINAL_IMAGE="$WORK_DIR/$OUTPUT_NAME"
LAYOUT_SCRIPT="$WORK_DIR/layout.applescript"
BUILD_VOLUME_NAME="tic-tac-toe-build-$$"
MOUNT_POINT=""
MOUNT_DEVICE=""

cleanup() {
    if [[ -n "$MOUNT_DEVICE" ]]; then
        hdiutil detach "$MOUNT_DEVICE" >/dev/null 2>&1 || true
    fi
    rm -rf "$WORK_DIR"
}
trap cleanup EXIT

mkdir -p "$STAGING_DIR"
write_bundle_manifest "$APP_BUNDLE" "$SOURCE_MANIFEST"
ditto "$APP_BUNDLE" "$STAGING_DIR/tic-tac-toe.app"
ln -s /Applications "$STAGING_DIR/Applications"
if [[ -n "$BACKGROUND" ]]; then
    mkdir -p "$STAGING_DIR/.background"
    ditto "$BACKGROUND" "$STAGING_DIR/.background/$(basename "$BACKGROUND")"
fi

write_bundle_manifest "$STAGING_DIR/tic-tac-toe.app" "$STAGED_MANIFEST"
if ! cmp -s "$SOURCE_MANIFEST" "$STAGED_MANIFEST"; then
    echo "Staged app differs from the signed source app" >&2
    diff -u "$SOURCE_MANIFEST" "$STAGED_MANIFEST" >&2 || true
    exit 1
fi
codesign --verify --deep --strict --verbose=2 "$STAGING_DIR/tic-tac-toe.app"

source_kib="$(du -sk "$STAGING_DIR" | awk '{print $1}')"
image_mebibytes="$((source_kib / 1024 + 40))"
hdiutil create -quiet \
    -size "${image_mebibytes}m" \
    -fs HFS+ \
    -format UDRW \
    -volname "$BUILD_VOLUME_NAME" \
    -srcfolder "$STAGING_DIR" \
    "$RW_IMAGE"

attach_output="$(hdiutil attach -readwrite -noverify -noautoopen -nobrowse "$RW_IMAGE")"
MOUNT_DEVICE="$(
    printf '%s\n' "$attach_output" \
        | awk 'index($0, "/Volumes/") { print $1; exit }'
)"
MOUNT_POINT="$(
    printf '%s\n' "$attach_output" \
        | awk 'index($0, "/Volumes/") { print substr($0, index($0, "/Volumes/")) }' \
        | tail -n 1
)"
if [[ -z "$MOUNT_DEVICE" || -z "$MOUNT_POINT" || ! -d "$MOUNT_POINT" ]]; then
    echo "Could not determine the mounted DMG path" >&2
    exit 1
fi

if [[ "${DMG_SKIP_FINDER_LAYOUT:-0}" != "1" ]]; then
    background_clause=""
    if [[ -n "$BACKGROUND" ]]; then
        background_clause="set background picture of opts to file \".background:$(basename "$BACKGROUND")\""
    fi
    cat > "$LAYOUT_SCRIPT" <<EOF
on run argv
  set volumeName to item 1 of argv
  tell application "Finder"
    tell disk (volumeName as string)
      open
      tell container window
        set current view to icon view
        set toolbar visible to false
        set statusbar visible to false
        set bounds to {100, 100, 760, 632}
      end tell
      set opts to the icon view options of container window
      tell opts
        set arrangement to not arranged
        set icon size to 128
        set text size to 16
      end tell
      $background_clause
      set position of item "tic-tac-toe.app" to {191, 330}
      set position of item "Applications" to {469, 330}
      update without registering applications
      delay 2
      close
    end tell
  end tell
end run
EOF
    volume_name="$(basename "$MOUNT_POINT")"
    layout_attempt=1
    until osascript "$LAYOUT_SCRIPT" "$volume_name"; do
        if [[ "$layout_attempt" -ge 3 ]]; then
            echo "Finder could not apply the DMG layout after $layout_attempt attempts" >&2
            exit 1
        fi
        layout_attempt="$((layout_attempt + 1))"
        sleep "$layout_attempt"
    done
fi

write_bundle_manifest "$MOUNT_POINT/tic-tac-toe.app" "$MOUNTED_MANIFEST"
if ! cmp -s "$SOURCE_MANIFEST" "$MOUNTED_MANIFEST"; then
    echo "Mounted app differs from the signed source app" >&2
    diff -u "$SOURCE_MANIFEST" "$MOUNTED_MANIFEST" >&2 || true
    exit 1
fi
codesign --verify --deep --strict --verbose=2 "$MOUNT_POINT/tic-tac-toe.app"

diskutil rename "$MOUNT_DEVICE" tic-tac-toe >/dev/null
hdiutil detach "$MOUNT_DEVICE" >/dev/null
MOUNT_DEVICE=""
MOUNT_POINT=""
hdiutil convert -quiet "$RW_IMAGE" -format UDZO -imagekey zlib-level=9 -o "$FINAL_IMAGE"
codesign --force --timestamp --sign "$SIGNING_IDENTITY" "$FINAL_IMAGE"
codesign --verify --deep --strict --verbose=2 "$FINAL_IMAGE"
hdiutil verify "$FINAL_IMAGE" >/dev/null

# Re-open the compressed, signed image and compare the release object one final
# time.  The image is not published unless this exact-object check succeeds.
attach_output="$(hdiutil attach -readonly -noverify -noautoopen -nobrowse "$FINAL_IMAGE")"
MOUNT_DEVICE="$(
    printf '%s\n' "$attach_output" \
        | awk 'index($0, "/Volumes/") { print $1; exit }'
)"
MOUNT_POINT="$(
    printf '%s\n' "$attach_output" \
        | awk 'index($0, "/Volumes/") { print substr($0, index($0, "/Volumes/")) }' \
        | tail -n 1
)"
if [[ -z "$MOUNT_DEVICE" || -z "$MOUNT_POINT" || ! -d "$MOUNT_POINT" ]]; then
    echo "Could not mount the finished DMG for verification" >&2
    exit 1
fi
write_bundle_manifest "$MOUNT_POINT/tic-tac-toe.app" "$MOUNTED_MANIFEST"
if ! cmp -s "$SOURCE_MANIFEST" "$MOUNTED_MANIFEST"; then
    echo "Finished DMG app differs from the signed source app" >&2
    diff -u "$SOURCE_MANIFEST" "$MOUNTED_MANIFEST" >&2 || true
    exit 1
fi
codesign --verify --deep --strict --verbose=2 "$MOUNT_POINT/tic-tac-toe.app"
hdiutil detach "$MOUNT_DEVICE" >/dev/null
MOUNT_DEVICE=""
MOUNT_POINT=""

rm -f "$OUTPUT_DIR/$OUTPUT_NAME.tmp"
ditto "$FINAL_IMAGE" "$OUTPUT_DIR/$OUTPUT_NAME.tmp"
mv "$OUTPUT_DIR/$OUTPUT_NAME.tmp" "$OUTPUT_DMG"

echo "Built: $OUTPUT_DMG"
echo "SHA-256: $(shasum -a 256 "$OUTPUT_DMG" | awk '{print $1}')"
