#!/bin/bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PLUGIN_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
MANIFEST_PATH="$PLUGIN_DIR/Cargo.toml"
PLUGIN_JSON="$PLUGIN_DIR/.claude-plugin/plugin.json"

cache_root() {
    if [ -n "${XDG_CACHE_HOME:-}" ]; then
        printf '%s/hilan\n' "$XDG_CACHE_HOME"
    else
        printf '%s/.cache/hilan\n' "$HOME"
    fi
}

require_rust() {
    if command -v cargo >/dev/null 2>&1 && command -v rustc >/dev/null 2>&1; then
        return 0
    fi

    echo "[hilan] Rust toolchain not found (need both cargo and rustc)." >&2
    case "$(uname -s)" in
        Darwin)
            echo "[hilan] Install Rust with one of:" >&2
            echo "[hilan]   brew install rust" >&2
            echo "[hilan]   or curl https://sh.rustup.rs -sSf | sh" >&2
            ;;
        Linux)
            echo "[hilan] Install Rust with:" >&2
            echo "[hilan]   curl https://sh.rustup.rs -sSf | sh" >&2
            ;;
        *)
            echo "[hilan] Install Rust from https://www.rust-lang.org/tools/install" >&2
            ;;
    esac
    exit 1
}

plugin_version() {
    grep -o '"version"[[:space:]]*:[[:space:]]*"[^"]*"' "$PLUGIN_JSON" \
        | head -1 \
        | sed 's/.*"\([^"]*\)"$/\1/'
}

needs_rebuild() {
    local bin_path="$1"

    if [ ! -x "$bin_path" ]; then
        return 0
    fi

    if [ "$MANIFEST_PATH" -nt "$bin_path" ] || [ "$PLUGIN_DIR/Cargo.lock" -nt "$bin_path" ]; then
        return 0
    fi

    if find "$PLUGIN_DIR/src" -type f -newer "$bin_path" -print -quit | grep -q .; then
        return 0
    fi

    return 1
}

require_rust

VERSION="$(plugin_version)"
if [ -z "$VERSION" ]; then
    echo "[hilan] ERROR: Could not read version from $PLUGIN_JSON" >&2
    exit 1
fi

CACHE_DIR="$(cache_root)"
VERSION_DIR="$CACHE_DIR/$VERSION"
TARGET_DIR="$CACHE_DIR/target"
BIN_PATH="$VERSION_DIR/hilan"

if needs_rebuild "$BIN_PATH"; then
    echo "[hilan] Building hilan v${VERSION}..." >&2
    mkdir -p "$VERSION_DIR"
    CARGO_TARGET_DIR="$TARGET_DIR" cargo build --release --manifest-path "$MANIFEST_PATH"

    SOURCE_BIN="$TARGET_DIR/release/hilan"
    if [ ! -x "$SOURCE_BIN" ]; then
        echo "[hilan] ERROR: build finished but binary not found at $SOURCE_BIN" >&2
        exit 1
    fi

    cp "$SOURCE_BIN" "$BIN_PATH.tmp"
    chmod +x "$BIN_PATH.tmp"
    mv "$BIN_PATH.tmp" "$BIN_PATH"
    echo "[hilan] Cached binary at $BIN_PATH" >&2
fi

exec "$BIN_PATH" "$@"
