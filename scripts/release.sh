#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage: ./scripts/release.sh <version> [options]

Examples:
  ./scripts/release.sh 1.0.0
  ./scripts/release.sh v1.0.0 --date 2026-04-10

Options:
  --date YYYY-MM-DD  Override release date (default: today in UTC)
  --skip-verify      Skip cargo check/test/clippy/fmt release gates
EOF
}

require_command() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "error: required command not found: $1" >&2
    exit 1
  fi
}

normalize_version() {
  local raw="$1"
  raw="${raw#v}"
  if [[ ! "$raw" =~ ^[0-9]+\.[0-9]+\.[0-9]+([.-][0-9A-Za-z]+)*$ ]]; then
    echo "error: version must look like 1.0.0 or v1.0.0" >&2
    exit 1
  fi
  printf '%s\n' "$raw"
}

update_json_version() {
  local path="$1"
  local version="$2"
  perl -0pi -e 's/"version":\s*"[^"]+"/"version": "'"$version"'"/' "$path"
}

require_command git
require_command cargo
require_command perl
require_command python3

root_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root_dir"

release_date="$(date -u +%Y-%m-%d)"
skip_verify=0

while [[ $# -gt 0 ]]; do
  case "$1" in
    -h|--help)
      usage
      exit 0
      ;;
    --date)
      [[ $# -ge 2 ]] || { echo "error: --date requires a value" >&2; exit 1; }
      release_date="$2"
      shift 2
      ;;
    --skip-verify)
      skip_verify=1
      shift
      ;;
    --*)
      echo "error: unknown option: $1" >&2
      usage >&2
      exit 1
      ;;
    *)
      break
      ;;
  esac
done

if [[ $# -ne 1 ]]; then
  usage >&2
  exit 1
fi

version="$(normalize_version "$1")"
tag="v${version}"

if [[ -n "$(git status --porcelain)" ]]; then
  echo "error: working tree has uncommitted changes" >&2
  exit 1
fi

current_branch="$(git branch --show-current)"
if [[ "$current_branch" != "main" ]]; then
  echo "error: release prep must run from main (current: ${current_branch})" >&2
  exit 1
fi

git fetch origin main --tags

local_main="$(git rev-parse HEAD)"
remote_main="$(git rev-parse origin/main)"
if [[ "$local_main" != "$remote_main" ]]; then
  echo "error: main is not up to date with origin/main; run git pull --ff-only first" >&2
  exit 1
fi

if git rev-parse -q --verify "refs/tags/${tag}" >/dev/null 2>&1; then
  echo "error: tag already exists locally: ${tag}" >&2
  exit 1
fi
if git ls-remote --exit-code --tags origin "refs/tags/${tag}" >/dev/null 2>&1; then
  echo "error: tag already exists on origin: ${tag}" >&2
  exit 1
fi

python3 - "$version" "$release_date" <<'PY'
import pathlib
import re
import sys

version, release_date = sys.argv[1], sys.argv[2]
path = pathlib.Path("CHANGELOG.md")
text = path.read_text()
marker = "## [Unreleased]"
if marker not in text:
    raise SystemExit("error: CHANGELOG.md is missing an Unreleased section")

pattern = re.compile(
    r"## \[Unreleased\]\n(?P<body>.*?)(?=\n## \[|\Z)",
    re.S,
)
match = pattern.search(text)
if not match:
    raise SystemExit("error: failed to parse CHANGELOG.md Unreleased section")

body = match.group("body").strip("\n")
if not body.strip():
    raise SystemExit("error: CHANGELOG.md Unreleased section is empty")

release_header = f"## [{version}] - {release_date}\n"
replacement = f"## [Unreleased]\n\n{release_header}\n{body}\n\n"
text = text[:match.start()] + replacement + text[match.end():]

unreleased_link = f"[Unreleased]: https://github.com/avivsinai/hilan/compare/{version and 'v'+version}...HEAD"
release_link = f"[{version}]: https://github.com/avivsinai/hilan/releases/tag/v{version}"

lines = text.rstrip("\n").splitlines()
lines = [line for line in lines if not line.startswith("[Unreleased]: ")]
if release_link not in lines:
    lines.append(release_link)
lines.append(unreleased_link)
path.write_text("\n".join(lines) + "\n")
PY

HILAN_VERSION="$version" perl -0pi -e '
  s/(\[package\]\n(?:[^\[]*\n)*?version = ")[^"]+(")/${1}$ENV{HILAN_VERSION}$2/s
    or die "failed to update [package].version\n";
' Cargo.toml

update_json_version .claude-plugin/plugin.json "$version"
update_json_version .codex-plugin/plugin.json "$version"

if [[ "$skip_verify" -eq 0 ]]; then
  cargo check --all-targets
  cargo test --all-targets
  cargo clippy --all-targets -- -D warnings
  cargo fmt --all -- --check
fi

./scripts/check-release-version.sh "$version"

git add CHANGELOG.md Cargo.toml Cargo.lock .claude-plugin/plugin.json .codex-plugin/plugin.json
git commit -m "chore(release): ${tag}"
git tag -a "$tag" -m "$tag"
git push origin main
git push origin "$tag"
