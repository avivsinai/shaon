#!/bin/bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
MANIFEST_PATH="$ROOT_DIR/Cargo.toml"

cache_root() {
    if [ -n "${XDG_CACHE_HOME:-}" ]; then
        printf '%s/shaon\n' "$XDG_CACHE_HOME"
    else
        printf '%s/.cache/shaon\n' "$HOME"
    fi
}

require_rust() {
    if command -v cargo >/dev/null 2>&1 && command -v rustc >/dev/null 2>&1; then
        return 0
    fi

    echo "[shaon] Rust toolchain not found (need both cargo and rustc)." >&2
    case "$(uname -s)" in
        Darwin)
            echo "[shaon] Install Rust with one of:" >&2
            echo "[shaon]   brew install rust" >&2
            echo "[shaon]   or curl https://sh.rustup.rs -sSf | sh" >&2
            ;;
        Linux)
            echo "[shaon] Install Rust with:" >&2
            echo "[shaon]   curl https://sh.rustup.rs -sSf | sh" >&2
            ;;
        *)
            echo "[shaon] Install Rust from https://www.rust-lang.org/tools/install" >&2
            ;;
    esac
    exit 1
}

package_version() {
    # Try [package] version = "x.y.z" first, then fall back to [workspace.package]
    awk '
        BEGIN { in_pkg = 0; in_ws_pkg = 0 }
        /^\[package\]/           { in_pkg = 1; in_ws_pkg = 0; next }
        /^\[workspace\.package\]/ { in_ws_pkg = 1; in_pkg = 0; next }
        /^\[/                    { in_pkg = 0; in_ws_pkg = 0; next }
        (in_pkg || in_ws_pkg) && $1 == "version" && $3 !~ /workspace/ {
            gsub(/"/, "", $3)
            print $3
            exit
        }
    ' "$MANIFEST_PATH"
}

needs_rebuild() {
    local bin_path="$1"

    if [ ! -x "$bin_path" ]; then
        return 0
    fi

    if [ "$MANIFEST_PATH" -nt "$bin_path" ] || [ "$ROOT_DIR/Cargo.lock" -nt "$bin_path" ]; then
        return 0
    fi

    if find "$ROOT_DIR/src" "$ROOT_DIR/crates" -type f -newer "$bin_path" -print -quit 2>/dev/null | grep -q .; then
        return 0
    fi

    return 1
}

require_rust

VERSION="$(package_version)"
if [ -z "$VERSION" ]; then
    echo "[shaon] ERROR: Could not read version from $MANIFEST_PATH" >&2
    exit 1
fi

CACHE_DIR="$(cache_root)"
VERSION_DIR="$CACHE_DIR/$VERSION"
TARGET_DIR="$CACHE_DIR/target"
BIN_PATH="$VERSION_DIR/shaon"

if needs_rebuild "$BIN_PATH"; then
    echo "[shaon] Building shaon v${VERSION}..." >&2
    mkdir -p "$VERSION_DIR"
    CARGO_TARGET_DIR="$TARGET_DIR" cargo build -p shaon --release --manifest-path "$MANIFEST_PATH"

    SOURCE_BIN="$TARGET_DIR/release/shaon"
    if [ ! -x "$SOURCE_BIN" ]; then
        echo "[shaon] ERROR: build finished but binary not found at $SOURCE_BIN" >&2
        exit 1
    fi

    # Codesign on macOS so the keyring crate can access the Keychain.
    # Prefer the self-signed identity created by scripts/setup-codesign.sh
    # (ACL sticks across rebuilds); fall back to ad-hoc only when the identity
    # is not installed. If it's installed but signing fails, surface a clear
    # warning instead of silently regressing to the old cdhash-churn behavior.
    #
    # We sign the fresh cargo output directly (before copy). That way anyone
    # invoking "$TARGET_DIR/release/shaon" (direct CDP experiments, sub-agents,
    # example binaries copied from the same tree) gets the stable identity too.
    if [[ "$(uname -s)" == "Darwin" ]] && command -v codesign >/dev/null 2>&1; then
        if security find-identity -v -p codesigning 2>/dev/null | grep -q "shaon-cli-signer"; then
            if ! codesign -s "shaon-cli-signer" -f --identifier "com.avivsinai.shaon" "$SOURCE_BIN" 2>/dev/null; then
                echo "[shaon] WARNING: signing with shaon-cli-signer failed. Falling back to ad-hoc;" >&2
                echo "[shaon]          macOS keychain will re-prompt for 'Always Allow' on every rebuild." >&2
                echo "[shaon]          Re-run scripts/setup-codesign.sh to reinstall the signing identity." >&2
                codesign -s - -f --identifier "com.avivsinai.shaon" "$SOURCE_BIN" 2>/dev/null || true
            fi
        else
            codesign -s - -f --identifier "com.avivsinai.shaon" "$SOURCE_BIN" 2>/dev/null || true
        fi
    fi

    cp "$SOURCE_BIN" "$BIN_PATH.tmp"
    chmod +x "$BIN_PATH.tmp"

    mv "$BIN_PATH.tmp" "$BIN_PATH"
    echo "[shaon] Cached binary at $BIN_PATH" >&2
fi

exec "$BIN_PATH" "$@"
