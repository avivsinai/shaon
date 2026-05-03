#!/usr/bin/env bash
set -euo pipefail

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "[setup-codesign] not macOS, nothing to do." >&2
  exit 0
fi

cat >&2 <<'MSG'
[setup-codesign] No setup is required for current shaon builds.
[setup-codesign]
[setup-codesign] scripts/run.sh and the release workflow now use
[setup-codesign] scripts/codesign-macos.sh, which signs macOS binaries with
[setup-codesign] the stable identifier io.github.avivsinai.shaon and an
[setup-codesign] explicit identifier-based designated requirement.
[setup-codesign]
[setup-codesign] If you previously approved an older shaon binary in Keychain,
[setup-codesign] macOS may ask for "Always Allow" one more time because the old
[setup-codesign] approval was tied to the previous signing requirement.
MSG
