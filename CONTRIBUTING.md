# Contributing to shaon

## Development Setup

1. Install [Rust](https://www.rust-lang.org/tools/install) (1.80+).
2. Clone the repo:
   ```bash
   git clone https://github.com/avivsinai/shaon.git
   cd shaon
   ```
3. Build:
   ```bash
   cargo build -p shaon
   ```
4. Run the checks:
   ```bash
   cargo fmt --all -- --check
   cargo clippy --workspace --all-targets -- -D warnings
   cargo test --workspace --all-targets
   ```

The repo is a Cargo workspace. `crates/hr-core` holds the provider-agnostic
surface, `crates/provider-hilan` contains the Hilan implementation, and the
root `shaon` package is the compatibility facade plus binary entrypoint.
On macOS, `scripts/run.sh` signs rebuilt binaries through
`scripts/codesign-macos.sh` with the release identifier so Keychain approvals
do not churn between local builds and release artifacts.

For the high-level code map, see [ARCHITECTURE.md](ARCHITECTURE.md).

## Commit Conventions

This project uses [Conventional Commits](https://www.conventionalcommits.org/):

```
<type>(<scope>): <subject>

[optional body]

[optional footer(s)]
```

Types: `feat`, `fix`, `docs`, `style`, `refactor`, `perf`, `test`, `build`, `ci`, `chore`.

Examples:
- `feat(attendance): add bulk fill command`
- `fix(client): handle expired session redirect`
- `docs: update README installation section`

Keep the subject line under 72 characters. Use the body for context on *why*, not *what*.

## Pull Request Process

1. Create a feature branch from `main`:
   ```bash
   git checkout -b feat/my-feature main
   ```
2. Make your changes in small, focused commits.
3. Ensure all checks pass locally:
   ```bash
   cargo fmt --all -- --check
   cargo clippy --workspace --all-targets -- -D warnings
   cargo test --workspace --all-targets
   ```
4. Push and open a PR against `main`.
5. PRs require passing CI before merge.

## Code Style

- Follow `rustfmt` defaults (no custom config).
- All public items should have doc comments.
- Avoid `unwrap()` in library code; use `anyhow::Result` and `bail!`/`?`.

## Documentation Checklist

If your change affects user-visible behavior, update the docs in the same PR:

- `README.md` for install, setup, CLI, MCP, and Claude Code usage
- `ARCHITECTURE.md` for crate boundaries or runtime-surface changes
- `PROTOCOL.md` for wire-level endpoint or replay behavior
- `skills/shaon/SKILL.md` for Claude Code skill usage or trigger changes
- `CLAUDE.md` when maintainer / coding-agent instructions change

Prefer stable descriptions over fragile hard-coded counts.

## Safety

- All write commands must default to dry-run.
- Never submit to Hilan without explicit `--execute`.
- Never store credentials in code or test fixtures.

## Responsible Contributions

`shaon` is designed for single-user personal automation. Please do not contribute features that:

- Aggregate multiple users' data
- Scrape third parties' accounts
- Bypass CAPTCHA, MFA, or other security measures
- Circumvent Hilan's terms of service

If in doubt, open an issue first to discuss.

## Reporting Issues

Open a GitHub issue with:
- What you expected
- What happened instead
- Steps to reproduce
- CLI version (`shaon --version`)
