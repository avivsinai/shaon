# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

Rust CLI + Claude Code plugin for automating Hilanet (Israeli HR/attendance system). Reverse-engineered protocol — two layers:
1. **ASP.NET WebForms** (`.aspx`) — parse hidden fields, replay full form POST
2. **ASMX JSON API** (`/Services/Public/WS/*.asmx/*`) — direct JSON RPC

See `@PROTOCOL.md` for endpoint details. See `@skills/hilan/SKILL.md` for CLI command reference.

## Build & run

```bash
cargo build -p hilan --release       # build the CLI binary
cargo test --workspace --all-targets  # run all tests across workspace
cargo clippy --workspace --all-targets -- -D warnings  # lint
cargo fmt --all -- --check            # format check
scripts/run.sh <subcommand> [args]    # smart-rebuild wrapper (caches + codesigns on macOS)
```

Requires Rust 1.80+ (edition 2021).

## Workspace architecture

Multi-crate workspace with provider abstraction:

```
crates/
├── hr-core/          — Provider-agnostic traits, DTOs, use-cases (no HTTP/Hilan deps)
├── provider-hilan/   — Hilan implementation: HTTP client, session, parsing, config
├── hilan-cli/        — CLI frontend (clap commands, rendering)
└── hilan-mcp/        — MCP server frontend (rmcp tools)
```

Root `src/` is a thin compatibility facade re-exporting the workspace crates.

### hr-core (generic layer)
- `AttendanceProvider` trait — identity, calendar, types, submit, fix
- `SalaryProvider`, `PayslipProvider`, `ReportProvider`, `AbsenceProvider` — optional capabilities
- Domain DTOs: `CalendarDay`, `MonthCalendar`, `AttendanceType`, `AttendanceChange`, `WritePreview`, `FixTarget`, `SalarySummary`
- Use-cases: `build_overview`, `fill_range`, `auto_fill`, `resolve_attendance_type` — provider-agnostic orchestration

### provider-hilan (Hilan adapter)
- `HilanProvider` implements all core traits
- `HilanClient` — reqwest with cookie jar, session reuse, retry, encrypted cookie persistence
- ASP.NET form replay + ASMX JSON calls
- Config at `~/.hilan/config.toml`, keychain via `keyring` crate

### hilan-cli + hilan-mcp (frontends)
- CLI: 19 clap subcommands, `--json` output, `--verbose`/`--quiet`
- MCP: 12 tools via rmcp 1.3 stdio transport, dry-run default

## Safety model

All write commands default to **dry-run**. `--execute` required for live submission. `fill`/`auto-fill` skip weekends (Fri/Sat) unless `--include-weekends`. `auto-fill` has `--max-days` safety cap (default 10).

## Adding a new command

1. Add use-case in `crates/hr-core/src/use_cases.rs` if it's provider-agnostic
2. Add trait method in hr-core if needed
3. Implement in `crates/provider-hilan/src/provider.rs`
4. Add CLI command in `crates/hilan-cli/src/lib.rs`
5. Add MCP tool in `crates/hilan-mcp/src/lib.rs` if appropriate
6. Add `--json` support and tests

## Credentials

Stored in OS keychain (`hilan-cli` service). Session cookies encrypted at rest (AES-256-GCM, random DEK in keychain). Binary must be codesigned with stable identifier (`com.avivsinai.hilan`) for silent macOS keychain access.
