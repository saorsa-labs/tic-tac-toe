#!/bin/bash
# tic-tac-toe portable launcher
#
# Detects an existing x0xd install and uses it. Falls back to the bundled
# binary. Never overwrites a system install.
#
# Flags:
#   --bundled   Force the bundled x0xd (ignores system install)
#   --fresh     Clear stale daemon state before launching
#
# Override: TTT_X0XD_BINARY=/path/to/x0xd
set -euo pipefail

DIR="$(cd "$(dirname "$0")" && pwd)"
APP_BUNDLE_NAME="tic-tac-toe.app"

# ── Parse flags ───────────────────────────────────────────────────────────

FORCE_BUNDLED=false
FRESH=false
while [[ $# -gt 0 ]]; do
    case "$1" in
        --bundled) FORCE_BUNDLED=true; shift ;;
        --fresh)   FRESH=true; shift ;;
        *)         break ;;
    esac
done

# ── Locate the app binary ─────────────────────────────────────────────────

# .app bundle: tic-tac-toe.app/Contents/MacOS/buzz-desktop
# Portable dir: ./tic-tac-toe or ./buzz-desktop
if [[ -x "$DIR/$APP_BUNDLE_NAME/Contents/MacOS/buzz-desktop" ]]; then
    BINARY="$DIR/$APP_BUNDLE_NAME/Contents/MacOS/buzz-desktop"
elif [[ -x "$DIR/buzz-desktop" ]]; then
    BINARY="$DIR/buzz-desktop"
elif [[ -x "$DIR/tic-tac-toe" ]]; then
    BINARY="$DIR/tic-tac-toe"
else
    echo "❌ tic-tac-toe binary not found in $DIR"
    exit 1
fi
# ── x0xd resolution ───────────────────────────────────────────────────────

BUNDLED_X0XD=""
for candidate in \
    "$DIR/$APP_BUNDLE_NAME/Contents/MacOS/x0xd" \
    "$DIR/x0xd" \
    "$DIR/../Resources/x0xd" \
    "$DIR/../Resources/binaries/x0xd"; do
    if [[ -x "$candidate" ]]; then
        BUNDLED_X0XD="$candidate"
        break
    fi
done
resolve_x0xd() {
    # 1. Explicit env override
    if [[ -n "${TTT_X0XD_BINARY:-}" && -x "$TTT_X0XD_BINARY" ]]; then
        echo "$TTT_X0XD_BINARY"
        return 0
    fi

    # 2. --bundled flag
    if $FORCE_BUNDLED && [[ -n "$BUNDLED_X0XD" ]]; then
        echo "$BUNDLED_X0XD"
        return 0
    fi

    # 3. PATH (system-wide install)
    local found
    found="$(command -v x0xd 2>/dev/null || true)"
    if [[ -n "$found" && -x "$found" ]]; then
        echo "$found"
        return 0
    fi

    # 4. Common install locations
    for loc in \
        /usr/local/bin/x0xd \
        /opt/homebrew/bin/x0xd \
        "$HOME/.cargo/bin/x0xd" \
        "$HOME/.local/bin/x0xd"; do
        if [[ -x "$loc" ]]; then
            echo "$loc"
            return 0
        fi
    done

    # 5. Bundled fallback
    if [[ -n "$BUNDLED_X0XD" ]]; then
        echo "$BUNDLED_X0XD"
        return 0
    fi

    return 1
}

X0XD_PATH="$(resolve_x0xd || true)"

if [[ -z "$X0XD_PATH" ]]; then
    echo "❌ No x0xd found."
    echo ""
    echo "   Install x0x (any one):"
    echo "     curl -sSf https://x0x.dev/install.sh | sh"
    echo "     cargo install x0x"
    echo ""
    echo "   Or set: export TTT_X0XD_BINARY=/path/to/x0xd"
    exit 1
fi

# Determine source label
if [[ "$X0XD_PATH" == "$BUNDLED_X0XD" ]]; then
    SOURCE="bundled"
elif [[ "$X0XD_PATH" == "${TTT_X0XD_BINARY:-}" ]]; then
    SOURCE="TTT_X0XD_BINARY"
else
    SOURCE="system"
fi

X0XD_VERSION="$("$X0XD_PATH" --version 2>&1 || echo 'unknown')"

# Version check — warn if below 0.36.0 (ADR-0029 threading minimum)
X0XD_VERSION_NUM="$(echo "$X0XD_VERSION" | grep -oE '[0-9]+\.[0-9]+\.[0-9]+' | head -1 || echo '0.0.0')"
if [[ -n "$X0XD_VERSION_NUM" ]] && [[ "$SOURCE" != "bundled" ]] && ! $FORCE_BUNDLED; then
    MAJOR="${X0XD_VERSION_NUM%%.*}"
    REST="${X0XD_VERSION_NUM#*.}"
    MINOR="${REST%%.*}"
    if [[ "$MAJOR" -lt 1 && "$MINOR" -lt 36 ]]; then
        echo "⚠️  x0xd $X0XD_VERSION_NUM < 0.36.0 — ADR-0029 thread replies need 0.36.0+."
        echo "   Run with --bundled to use the packaged x0xd instead."
        echo ""
    fi
fi

echo "▸ x0xd: $X0XD_VERSION ($SOURCE)"
echo "▸ path: $X0XD_PATH"

# ── Clean stale per-instance state ────────────────────────────────────────

if $FRESH; then
    DATA_DIR="$HOME/Library/Application Support/x0x-ttt"
    if [[ -d "$DATA_DIR" ]]; then
        echo "▸ Clearing stale daemon state: $DATA_DIR"
        rm -rf "$DATA_DIR"
    fi
fi

# ── Launch ────────────────────────────────────────────────────────────────

export TTT_X0XD_BINARY="$X0XD_PATH"

echo "▸ launching tic-tac-toe…"
exec "$BINARY" "$@"
