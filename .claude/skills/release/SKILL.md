---
name: release
description: Bump version, update all version references, build release binary, and create git tag. Use when cutting a new release of shaon.
disable-model-invocation: true
---

# Release Workflow

Run this skill with: `/release $ARGUMENTS`

`$ARGUMENTS` should be the new version number (e.g., `0.4.0`). If not provided, ask the user.

## Steps

1. **Validate version**: Confirm `$ARGUMENTS` is a valid semver (MAJOR.MINOR.PATCH). Read current version from `Cargo.toml`.

2. **Bump Cargo.toml**: Update `version = "..."` in `[package]` section.

3. **Bump plugin.json**: Update `"version": "..."` in `.claude-plugin/plugin.json`.

4. **Update README.md**: Replace any occurrences of the old version string with the new version (e.g., in install commands, badges, or header).

5. **Build release binary**: Run `cargo build --release` and confirm it compiles cleanly.

6. **Run clippy**: Run `cargo clippy -- -D warnings` to ensure no lint issues.

7. **Show diff**: Run `git diff` and present the changes for review.

8. **Commit**: Create a single commit: `Release v{VERSION}` with the version-bumped files.

9. **Tag**: Create an annotated git tag: `git tag -a v{VERSION} -m "Release v{VERSION}"`.

10. **Report**: Print the new version, tagged commit hash, and cached binary path (`~/.cache/shaon/{VERSION}/shaon`). Remind the user to `git push --follow-tags` when ready.
