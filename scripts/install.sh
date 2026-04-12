#!/usr/bin/env bash
set -euo pipefail

# Usage:
#   curl -fsSL https://raw.githubusercontent.com/avivsinai/shaon/main/scripts/install.sh | bash
#   curl -fsSL https://raw.githubusercontent.com/avivsinai/shaon/main/scripts/install.sh | VERSION=v0.8.0 bash
#   curl -fsSL https://raw.githubusercontent.com/avivsinai/shaon/main/scripts/install.sh | INSTALL_DIR="$HOME/bin" bash

REPO="avivsinai/shaon"
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

  if [[ -d "$HOME/.local/bin" ]]; then
    printf '%s\n' "$HOME/.local/bin"
    return
  fi

  if [[ -d "$HOME/bin" ]]; then
    printf '%s\n' "$HOME/bin"
    return
  fi

  if [[ -d "$HOME/.cargo/bin" ]]; then
    printf '%s\n' "$HOME/.cargo/bin"
    return
  fi

  printf '%s\n' "$HOME/.local/bin"
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
      printf '%s\n' "shaon-${target}.tar.gz"
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
    die "Error: checksum entry not found for ${asset}"
  fi

  expected="$(printf '%s\n' "$line" | awk '{print $1}')"

  if command -v sha256sum >/dev/null 2>&1; then
    actual="$(sha256sum "$asset_path" | awk '{print $1}')"
  elif command -v shasum >/dev/null 2>&1; then
    actual="$(shasum -a 256 "$asset_path" | awk '{print $1}')"
  else
    die "Error: no SHA-256 verification tool found (need sha256sum or shasum)"
  fi

  if [[ "$expected" != "$actual" ]]; then
    die "Error: checksum verification failed for ${asset}"
  fi
}

require_command curl
require_command tar
require_command install

info "=== Shaon Installer ==="

INSTALL_DIR="$(determine_install_dir)"
ASSET="$(detect_asset)"
CHECKSUMS_ASSET="SHA256SUMS.txt"
if [[ "$VERSION" == "latest" ]]; then
  VERSION_LABEL="latest"
  ASSET_URL="https://github.com/${REPO}/releases/latest/download/${ASSET}"
  CHECKSUMS_URL="https://github.com/${REPO}/releases/latest/download/${CHECKSUMS_ASSET}"
else
  TAG="$VERSION"
  if [[ "$TAG" != v* ]]; then
    TAG="v${TAG}"
  fi
  VERSION_LABEL="$TAG"
  ASSET_URL="https://github.com/${REPO}/releases/download/${TAG}/${ASSET}"
  CHECKSUMS_URL="https://github.com/${REPO}/releases/download/${TAG}/${CHECKSUMS_ASSET}"
fi

printf 'Version: %s\n' "$VERSION_LABEL"
printf 'Platform: %s / %s\n' "$(uname -s)" "$(uname -m)"
printf 'Asset:   %s\n' "$ASSET"
printf 'Install: %s\n' "$INSTALL_DIR/shaon"

TMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TMP_DIR"' EXIT

info "Downloading release asset..."
if ! curl -fsSL "$ASSET_URL" -o "$TMP_DIR/$ASSET"; then
  die "Error: failed to download ${ASSET}. Check https://github.com/${REPO}/releases for available assets."
fi

curl -fsSL "$CHECKSUMS_URL" -o "$TMP_DIR/$CHECKSUMS_ASSET" \
  || die "Error: failed to download ${CHECKSUMS_ASSET} for checksum verification."
verify_checksum "$ASSET" "$TMP_DIR/$CHECKSUMS_ASSET" "$TMP_DIR/$ASSET"
info "Checksum verification passed."

info "Extracting archive..."
tar -xzf "$TMP_DIR/$ASSET" -C "$TMP_DIR"
[[ -f "$TMP_DIR/shaon" ]] || die "Error: archive did not contain the shaon binary."

mkdir -p "$INSTALL_DIR"
install -m 0755 "$TMP_DIR/shaon" "$INSTALL_DIR/shaon"

installed_version="$("$INSTALL_DIR/shaon" --version 2>/dev/null || true)"

printf '\n%sInstallation complete.%s\n' "$green" "$reset"
if [[ -n "$installed_version" ]]; then
  printf 'Installed: %s\n' "$installed_version"
fi

if path_contains "$INSTALL_DIR"; then
  printf 'Available as: shaon\n'
else
  warn "Warning: ${INSTALL_DIR} is not in your PATH."
  if [[ -n "${ZSH_VERSION:-}" || -f "$HOME/.zshrc" ]]; then
    printf 'Add this to ~/.zshrc:\n'
  else
    printf 'Add this to ~/.bashrc:\n'
  fi
  printf '  export PATH="%s:$PATH"\n' "$INSTALL_DIR"
  printf 'Direct path: %s/shaon\n' "$INSTALL_DIR"
fi

if [[ "$(uname -s)" == "Darwin" ]]; then
  printf '\nmacOS note:\n'
  printf '  Release binaries are ad-hoc signed in CI for Keychain access.\n'
  printf '  macOS may ask you to re-approve Keychain access after each upgrade.\n'
  printf '  For advanced unattended automation, prefer SHAON_PASSWORD and SHAON_MASTER_KEY.\n'
fi

printf '\nNext steps:\n'
printf '  # If upgrading from <= v0.7.0, run auth once to refresh the keychain entry.\n'
printf '  shaon auth\n'
printf '  shaon attendance overview\n'
printf '  shaon payroll payslip download --month YYYY-MM\n'
