# CLAUDE.md

This file is for coding agents and maintainers working in this repository.

## Read This First

- [README.md](README.md): user-facing install and usage
- [ARCHITECTURE.md](ARCHITECTURE.md): crate boundaries and maintenance map
- [PROTOCOL.md](PROTOCOL.md): Hilanet wire behavior and replay details
- [skills/shaon/SKILL.md](skills/shaon/SKILL.md): Claude Code skill text

Do not duplicate hard-coded command or tool counts in docs unless the count itself matters. The authoritative user-facing surfaces are:

- CLI subcommands: `crates/shaon-cli/src/lib.rs`
- MCP tools: `crates/shaon-mcp/src/lib.rs`
- Claude Code plugin metadata: `.claude-plugin/plugin.json`
- Claude Code skill text: `skills/shaon/SKILL.md`

## What This Repo Is

Rust workspace for automating Hilan / Hilanet through:

1. a human-facing CLI
2. a stdio MCP server
3. a Claude Code plugin / skill

The protocol has two layers:

1. ASP.NET WebForms (`.aspx`) for calendar pages, error-fix flows, and classic reports
2. ASMX JSON endpoints (`/Services/Public/WS/*.asmx/*`) for bootstrap, absences, tasks, and salary data

## Build and Checks

```bash
cargo build -p shaon --release
cargo test --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
scripts/run.sh <subcommand> [args]
```

Rust requirement: 1.80+.

## Release

### Release Contract

- Release from `main` only through `./scripts/release.sh X.Y.Z` and the resulting release PR; do not create manual tags or GitHub releases.
- Keep one version across `CHANGELOG.md`, workspace metadata, skill frontmatter, and plugin manifests.
- The release PR merge is the trigger. CI validates the merged `chore(release): vX.Y.Z` commit, creates the matching tag, publishes the GitHub release from the committed changelog entry, and then opens `gh`-driven PRs to update Homebrew and Scoop.
- GitHub Actions workflow permissions for this repo must stay on `Read and write`, and `RELEASE_GITHUB_TOKEN` should be configured; the release workflow publishes the GitHub release with `gh`.
- `skills-marketplace` is a registry pointer to this repo's default branch. Once `shaon` is listed there, marketplace installs track `main`; no separate marketplace release job is required for day-to-day skill changes.

Use the fast release path:

```bash
./scripts/release.sh 0.8.3
```

After the release PR merges:

- `.github/workflows/release.yml` creates or verifies the tag for the merged release commit
- the same workflow publishes release artifacts and uses `CHANGELOG.md` as the GitHub release notes source
- the workflow can publish the skill to `skild` when `SKILD_AUTH_JSON` is configured
- the workflow can open PRs against `avivsinai/homebrew-tap` and `avivsinai/scoop-bucket` when `PACKAGING_REPO_GITHUB_TOKEN` is configured

## Workspace Structure

```text
crates/
├── hr-core/        provider-agnostic traits, DTOs, and use-cases
├── provider-hilan/ Hilan-specific transport, parsing, config, and session logic
├── shaon-cli/      clap CLI frontend
└── shaon-mcp/      rmcp stdio server frontend
```

Root `src/` is a compatibility facade re-exporting the workspace crates.

## Safety Model

- All writes default to preview
- CLI requires `--execute` for live submission
- MCP requires `execute: true` for live submission
- `attendance report range` / `attendance auto-fill` skip Fri/Sat unless explicitly overridden
- State-changing requests must not be retried automatically

## Documentation Maintenance

If you change user-visible behavior, update the docs in the same patch:

- CLI behavior or examples: `README.md`, `skills/shaon/SKILL.md`
- MCP tools or schemas: `README.md`
- crate boundaries or ownership: `ARCHITECTURE.md`
- wire behavior or endpoint assumptions: `PROTOCOL.md`
- contributor workflow or required checks: `CONTRIBUTING.md`

When possible, document stable concepts instead of fragile counts.

## Design Principles

- No backwards-compatibility shims unless explicitly requested
- No `#[serde(default)]` to paper over required data migrations
- Prefer changing the clean API directly over carrying migration layers

## Credentials and macOS Notes

- Passwords live in the OS keychain by default (`shaon-cli` service)
- Session cookies are encrypted at rest with AES-256-GCM
- `SHAON_PASSWORD` and `SHAON_MASTER_KEY` are the headless / CI escape hatches
- On macOS, `scripts/run.sh` uses the stable local signing identity from `scripts/setup-codesign.sh` when available
