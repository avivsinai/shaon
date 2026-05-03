#!/usr/bin/env bash
set -euo pipefail

binary_path="${1:?usage: ./scripts/codesign-macos.sh /path/to/binary identifier [target-os]}"
identifier="${2:?usage: ./scripts/codesign-macos.sh /path/to/binary identifier [target-os]}"
target_os="${3:-darwin}"

if [[ "$target_os" != "darwin" ]]; then
  exit 0
fi

if [[ ! -f "$binary_path" ]]; then
  echo "error: binary not found: $binary_path" >&2
  exit 1
fi

if [[ "$(uname -s)" != "Darwin" ]]; then
  exit 0
fi

if ! command -v codesign >/dev/null 2>&1; then
  echo "error: codesign not found" >&2
  exit 1
fi

# Sign ad-hoc with an explicit Designated Requirement pinned to the identifier.
#
# Without -r, ad-hoc signatures derive a DR from the cdhash, which changes on
# every rebuild. That invalidates macOS Keychain "Always Allow" approvals after
# local rebuilds and release upgrades. Pinning the DR to the identifier keeps the
# Keychain ACL stable for both scripts/run.sh builds and published artifacts.
#
# Security trade-off: any other ad-hoc binary that claims this identifier can
# satisfy the same DR. This is acceptable for shaon's current local-user threat
# model; a Developer ID signature would provide a stricter anchor in the future.
codesign \
  --force \
  --sign - \
  --identifier "$identifier" \
  -r="designated => identifier \"$identifier\"" \
  "$binary_path"
