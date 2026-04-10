#!/usr/bin/env bash
set -euo pipefail

# Usage:
#   curl -fsSL https://raw.githubusercontent.com/avivsinai/hilan/main/scripts/install.sh | bash
#   curl -fsSL https://raw.githubusercontent.com/avivsinai/hilan/main/scripts/install.sh | VERSION=v1.0.0 bash
#   curl -fsSL https://raw.githubusercontent.com/avivsinai/hilan/main/scripts/install.sh | INSTALL_DIR="$HOME/bin" bash

REPO="avivsinai/hilan"
VERSION="${VERSION:-latest}"

blue=$'\033[0;34m'
green=$'\033[0;32m'
yellow=$'\033[1;33m'
red=$'\033[0;31m'
reset=$'\033[0m'

info() {
  printf '%s%s%s\n' "$blue" "$1" "$reset"
}

warn() {
  printf '%s%s%s\n' "$yellow" "$1" "$reset" >&2
}

die() {
  printf '%s%s%s\n' "$red" "$1" "$reset" >&2
  exit 1
}

require_command() {
  if ! command -v "$1" >/dev/null 2>&1; then
    die "Error: required command not found: $1"
  fi
}

path_contains() {
  case ":${PATH:-}:" in
    *":$1:"*) return 0 ;;
    *) return 1 ;;
  esac
}

determine_install_dir() {
  if [[ -n "${INSTALL_DIR:-}" ]]; then
    printf '%s\n' "$INSTALL_DIR"
    return
  fi

  if [[ -d /usr/local/bin && -w /usr/local/bin ]]; then
    printf '%s\n' "/usr/local/bin"
    return
  fi

  if [[ ! -e /usr/local/bin && -d /usr/local && -w /usr/local ]]; then
    printf '%s\n' "/usr/local/bin"
    return
  fi

  printf '%s\n' "$HOME/.local/bin"
}

normalize_version() {
  local value="$1"
  if [[ "$value" == "latest" ]]; then
    printf '%s\n' "$value"
  elif [[ "$value" == v* ]]; then
    printf '%s\n' "$value"
  else
    printf 'v%s\n' "$value"
  fi
}

resolve_latest_version() {
  curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" \
    | sed -n 's/.*"tag_name":[[:space:]]*"\([^"]*\)".*/\1/p' \
    | head -n1
}

detect_asset() {
  local os arch target

  case "$(uname -s)" in
    Darwin) os="apple-darwin" ;;
    Linux) os="unknown-linux-gnu" ;;
    MINGW*|MSYS*|CYGWIN*)
      die "Error: Windows is not supported by this installer. Use WSL or download a release asset manually."
      ;;
    *)
      die "Error: unsupported operating system: $(uname -s)"
      ;;
  esac

  case "$(uname -m)" in
    arm64|aarch64) arch="aarch64" ;;
    x86_64|amd64) arch="x86_64" ;;
    *)
      die "Error: unsupported architecture: $(uname -m)"
      ;;
  esac

  target="${arch}-${os}"

  case "$target" in
    aarch64-apple-darwin|x86_64-apple-darwin|x86_64-unknown-linux-gnu)
      printf '%s\n' "hilan-${target}.tar.gz"
      ;;
    *)
      die "Error: no prebuilt release asset is published for ${target}"
      ;;
  esac
}

verify_checksum() {
  local asset="$1"
  local checksums_file="$2"
  local asset_path="$3"
  local line expected actual

  line="$(grep "[[:space:]]${asset}\$" "$checksums_file" || true)"
  if [[ -z "$line" ]]; then
    warn "Warning: checksum entry not found for ${asset}; skipping verification."
    return
  fi

  expected="$(printf '%s\n' "$line" | awk '{print $1}')"

  if command -v sha256sum >/dev/null 2>&1; then
    actual="$(sha256sum "$asset_path" | awk '{print $1}')"
  elif command -v shasum >/dev/null 2>&1; then
    actual="$(shasum -a 256 "$asset_path" | awk '{print $1}')"
  else
    warn "Warning: no SHA-256 tool found; skipping verification."
    return
  fi

  if [[ "$expected" != "$actual" ]]; then
    die "Error: checksum verification failed for ${asset}"
  fi
}

require_command curl
require_command tar
require_command install

info "=== Hilan Installer ==="

INSTALL_DIR="$(determine_install_dir)"
TAG="$(normalize_version "$VERSION")"
if [[ "$TAG" == "latest" ]]; then
  info "Fetching latest release..."
  TAG="$(resolve_latest_version)"
  [[ -n "$TAG" ]] || die "Error: could not determine the latest release tag."
fi

ASSET="$(detect_asset)"
CHECKSUMS_ASSET="SHA256SUMS.txt"
ASSET_URL="https://github.com/${REPO}/releases/download/${TAG}/${ASSET}"
CHECKSUMS_URL="https://github.com/${REPO}/releases/download/${TAG}/${CHECKSUMS_ASSET}"

printf 'Version: %s\n' "$TAG"
printf 'Asset:   %s\n' "$ASSET"
printf 'Target:  %s\n' "$INSTALL_DIR/hilan"

TMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TMP_DIR"' EXIT

info "Downloading release asset..."
if ! curl -fsSL "$ASSET_URL" -o "$TMP_DIR/$ASSET"; then
  die "Error: failed to download ${ASSET}. Check https://github.com/${REPO}/releases for available assets."
fi

if curl -fsSL "$CHECKSUMS_URL" -o "$TMP_DIR/$CHECKSUMS_ASSET"; then
  verify_checksum "$ASSET" "$TMP_DIR/$CHECKSUMS_ASSET" "$TMP_DIR/$ASSET"
else
  warn "Warning: could not download ${CHECKSUMS_ASSET}; skipping verification."
fi

info "Extracting archive..."
tar -xzf "$TMP_DIR/$ASSET" -C "$TMP_DIR"
[[ -f "$TMP_DIR/hilan" ]] || die "Error: archive did not contain the hilan binary."

mkdir -p "$INSTALL_DIR"
install -m 0755 "$TMP_DIR/hilan" "$INSTALL_DIR/hilan"

installed_version="$("$INSTALL_DIR/hilan" --version 2>/dev/null || true)"

printf '\n%sInstallation complete.%s\n' "$green" "$reset"
if [[ -n "$installed_version" ]]; then
  printf 'Installed: %s\n' "$installed_version"
fi

if path_contains "$INSTALL_DIR"; then
  printf 'Available as: hilan\n'
else
  warn "Warning: ${INSTALL_DIR} is not in your PATH."
  printf 'Add it to your shell profile:\n'
  printf '  export PATH="%s:$PATH"\n' "$INSTALL_DIR"
fi
