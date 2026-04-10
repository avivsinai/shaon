#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage: ./scripts/release.sh <version> [options]

Examples:
  ./scripts/release.sh 0.5.0
  ./scripts/release.sh v0.5.0

This script:
1. Verifies you are on a clean, up-to-date main branch
2. Creates release/vX.Y.Z branch
3. Moves CHANGELOG.md's Unreleased section into a versioned release entry
4. Bumps [workspace.package].version plus skill/plugin metadata
5. Runs cargo check/test/clippy/fmt release gates
6. Commits the release bump as chore(release): vX.Y.Z
7. Pushes the branch
8. Creates a GitHub PR with gh and enables squash auto-merge

After the PR merges, the Release workflow creates vX.Y.Z automatically,
publishes artifacts, and uses CHANGELOG.md as the release notes source.

Options:
  --date YYYY-MM-DD  Override release date (default: today in UTC)
  --allow-empty      Allow releasing with an empty Unreleased section
  --skip-verify      Skip cargo check/test/clippy/fmt release gates
  --no-auto-merge    Create the PR but do not enable auto-merge
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
    echo "error: version must look like 0.5.0 or v0.5.0" >&2
    exit 1
  fi
  printf '%s\n' "$raw"
}

update_json_version() {
  local path="$1"
  local version="$2"
  [[ -f "$path" ]] || return 0
  perl -0pi -e 's/"version":\s*"[^"]+"/"version": "'"$version"'"/' "$path"
}

require_command git
require_command cargo
require_command gh
require_command perl
require_command python3

root_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root_dir"

release_date="$(date -u +%Y-%m-%d)"
allow_empty=0
skip_verify=0
auto_merge=1

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
    --allow-empty)
      allow_empty=1
      shift
      ;;
    --skip-verify)
      skip_verify=1
      shift
      ;;
    --no-auto-merge)
      auto_merge=0
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
branch="release/${tag}"

# --- Pre-flight checks ---

if [[ -n "$(git status --porcelain)" ]]; then
  echo "error: working tree has uncommitted changes" >&2
  exit 1
fi

current_branch="$(git branch --show-current)"
if [[ "$current_branch" != "main" ]]; then
  echo "error: release prep must start from main (current: ${current_branch})" >&2
  exit 1
fi

git fetch origin main --tags

local_main="$(git rev-parse HEAD)"
remote_main="$(git rev-parse origin/main)"
if [[ "$local_main" != "$remote_main" ]]; then
  echo "error: main is not up to date with origin/main; run git pull --ff-only first" >&2
  exit 1
fi

if git show-ref --verify --quiet "refs/heads/${branch}"; then
  echo "error: branch already exists locally: ${branch}" >&2
  exit 1
fi
if git ls-remote --exit-code --heads origin "${branch}" >/dev/null 2>&1; then
  echo "error: branch already exists on origin: ${branch}" >&2
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

# --- Create release branch ---

git switch -c "${branch}"

# --- Bump CHANGELOG ---

python3 - "$version" "$release_date" "$allow_empty" <<'PY'
import pathlib
import re
import sys

version, release_date, allow_empty = sys.argv[1], sys.argv[2], sys.argv[3] == "1"

path = pathlib.Path("CHANGELOG.md")
text = path.read_text()
marker = "## [Unreleased]"
if marker not in text:
    raise SystemExit("error: CHANGELOG.md is missing an Unreleased section")

start = text.index(marker)
after_marker = start + len(marker)
rest = text[after_marker:]
match = re.search(r"(?m)^## \[", rest)
if match:
    unreleased_body = rest[:match.start()]
    suffix = rest[match.start():]
else:
    unreleased_body = rest
    suffix = ""

if not unreleased_body.strip() and not allow_empty:
    raise SystemExit("error: CHANGELOG.md Unreleased section is empty; add release notes first or pass --allow-empty")

release_header = f"\n\n## [{version}] - {release_date}\n"
new_text = text[:start] + marker + release_header + unreleased_body.lstrip("\n")
if suffix:
    new_text += suffix if suffix.startswith("\n") else "\n" + suffix

path.write_text(new_text)
PY

# --- Bump versions ---

SHAON_VERSION="$version" perl -0pi -e '
  s/(\[workspace\.package\]\n(?:[^\[]*\n)*?version = ")[^"]+(")/${1}$ENV{SHAON_VERSION}$2/s
    or die "failed to update [workspace.package].version\n";
' Cargo.toml

update_json_version .claude-plugin/plugin.json "$version"
update_json_version .codex-plugin/plugin.json "$version"

# Bump skill frontmatter versions
while IFS= read -r skill_md; do
  if grep -q '^version:' "$skill_md"; then
    sed -i '' "s/^version: .*/version: ${version}/" "$skill_md" 2>/dev/null \
      || sed -i "s/^version: .*/version: ${version}/" "$skill_md"
  fi
done < <(find skills -mindepth 2 -maxdepth 2 -name SKILL.md | sort)

# --- Verify ---

if [[ "$skip_verify" -eq 0 ]]; then
  cargo check --workspace --all-targets
  cargo test --workspace --all-targets
  cargo clippy --workspace --all-targets -- -D warnings
  cargo fmt --all -- --check
fi
./scripts/check-release-version.sh "$version"

# --- Commit and push ---

git add CHANGELOG.md Cargo.toml Cargo.lock
for f in .claude-plugin/plugin.json .codex-plugin/plugin.json; do
  [[ -f "$f" ]] && git add "$f"
done
find skills -mindepth 2 -maxdepth 2 -name SKILL.md -print0 | while IFS= read -r -d '' f; do
  git add "$f"
done

if git diff --cached --quiet; then
  echo "error: release prep produced no staged changes" >&2
  exit 1
fi

git commit -m "chore(release): ${tag}"
git push -u origin "${branch}"

# --- Create PR with auto-merge ---

pr_body=$(cat <<EOF
## Release ${tag}

- Updates CHANGELOG.md for ${tag}
- Bumps package version to ${version}
- Aligns skill/plugin metadata to ${version}

After merge, .github/workflows/release.yml creates the tag and publishes artifacts.
GitHub release notes come from the committed CHANGELOG.md entry.
EOF
)

pr_url="$(
  gh pr create \
    --base main \
    --head "${branch}" \
    --title "chore(release): ${tag}" \
    --body "${pr_body}"
)"

if [[ "$auto_merge" -eq 1 ]]; then
  gh pr merge --auto --squash --delete-branch "$pr_url" || {
    echo "warning: failed to enable auto-merge; merge manually or rerun with --no-auto-merge" >&2
  }
fi

echo ""
echo "Prepared ${tag}"
echo "Release branch: ${branch}"
echo "Pull request: ${pr_url}"
if [[ "$auto_merge" -eq 1 ]]; then
  echo "Auto-merge: enabled (squash)"
else
  echo "Auto-merge: not enabled — merge manually"
fi
