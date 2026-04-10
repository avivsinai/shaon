#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage: ./scripts/check-release-version.sh <version>
EOF
}

if [[ $# -ne 1 ]]; then
  usage >&2
  exit 1
fi

version="${1#v}"
root_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root_dir"

read_package_version() {
  awk '
    BEGIN { in_workspace_package = 0; in_package = 0 }
    /^\[workspace\.package\]/ { in_workspace_package = 1; in_package = 0; next }
    /^\[package\]/ { in_package = 1; in_workspace_package = 0; next }
    /^\[/ { in_workspace_package = 0; in_package = 0 }
    (in_workspace_package || in_package) && $1 == "version" {
      gsub(/"/, "", $3)
      print $3
      exit
    }
  ' Cargo.toml
}

package_version="$(read_package_version)"
if [[ "$package_version" != "$version" ]]; then
  echo "error: Cargo.toml version is $package_version, expected $version" >&2
  exit 1
fi

for plugin_json in .claude-plugin/plugin.json .codex-plugin/plugin.json; do
  [[ -f "$plugin_json" ]] || continue
  plugin_version="$(
    sed -n 's/.*"version":[[:space:]]*"\([^"]*\)".*/\1/p' "$plugin_json" | head -n1
  )"
  if [[ "$plugin_version" != "$version" ]]; then
    echo "error: $plugin_json version is $plugin_version, expected $version" >&2
    exit 1
  fi
done

if ! grep -q "^## \[$version\]" CHANGELOG.md; then
  echo "error: CHANGELOG.md is missing a release entry for $version" >&2
  exit 1
fi
