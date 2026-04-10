# Contributing to hilan

## Development Setup

1. Install [Rust](https://www.rust-lang.org/tools/install) (1.80+).
2. Clone the repo:
   ```bash
   git clone https://github.com/avivsinai/hilan.git
   cd hilan
   ```
3. Build:
   ```bash
   cargo build
   ```
4. Run the checks:
   ```bash
   cargo fmt --all -- --check
   cargo clippy --all-targets -- -D warnings
   cargo test --all-targets
   ```

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
   cargo clippy --all-targets -- -D warnings
   cargo test --all-targets
   ```
4. Push and open a PR against `main`.
5. PRs require passing CI before merge.

## Code Style

- Follow `rustfmt` defaults (no custom config).
- All public items should have doc comments.
- Avoid `unwrap()` in library code; use `anyhow::Result` and `bail!`/`?`.

## Safety

- All write commands must default to dry-run.
- Never submit to Hilan without explicit `--execute`.
- Never store credentials in code or test fixtures.

## Reporting Issues

Open a GitHub issue with:
- What you expected
- What happened instead
- Steps to reproduce
- CLI version (`hilan --version`)
