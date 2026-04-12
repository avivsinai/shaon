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
