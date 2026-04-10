# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

Rust CLI + Claude Code plugin for automating Hilanet (Israeli HR/attendance system). Reverse-engineered protocol — two layers:
1. **ASP.NET WebForms** (`.aspx`) — parse hidden fields, replay full form POST
2. **ASMX JSON API** (`/Services/Public/WS/*.asmx/*`) — direct JSON RPC

See `@PROTOCOL.md` for endpoint details. See `@skills/hilan/SKILL.md` for CLI command reference.

## Build & run

```bash
scripts/run.sh <subcommand> [args]   # smart-rebuild wrapper (caches + codesigns on macOS)
cargo build --release                # direct build
cargo clippy --all-targets -- -D warnings  # lint (must pass clean)
cargo test --all-targets             # run all tests
cargo fmt --check                    # format check
```

Requires Rust 1.80+ (edition 2021).

## Architecture

Lib/bin split — `src/lib.rs` re-exports all modules, `src/main.rs` is CLI dispatch only.

- `lib.rs` — public module re-exports
- `main.rs` — clap subcommands (18 commands), `WriteMode` struct, async dispatch via tokio
- `client.rs` — `HilanClient` wrapping reqwest with cookie jar; WebForms form replay + ASMX JSON calls; login with keychain credential lookup; retry with exponential backoff; session expiry detection
- `attendance.rs` — calendar read/write, month navigation, submit preview, auto-fill, `CalendarDay` struct
- `ontology.rs` — attendance type cache (JSON with 24h TTL), lazy auto-sync on first symbolic type use
- `api.rs` — bootstrap call to extract user/employee/org IDs
- `reports.rs` — HTML table parsing for error/missing/absence reports
- `config.rs` — config loading with keychain integration via `keyring` crate; password stored as `secrecy::SecretString`
- `mcp.rs` — MCP server (stdio transport) exposing 12 tools via `rmcp` 1.3

## Key dependencies

`clap` 4 (CLI), `reqwest` 0.12 (HTTP + cookies + rustls), `tokio` (async), `serde`/`serde_json` (serialization), `scraper` (HTML parsing), `keyring` (OS keychain), `secrecy`/`zeroize` (credential safety), `urlencoding`, `clap_complete` (shell completions), `tracing`/`tracing-subscriber` (structured logging), `rmcp` 1.3 (MCP server), `schemars` (JSON schema for MCP tools)

## Safety model

All write commands (`clock-in`, `clock-out`, `fill`, `fix`) default to **dry-run**. The `--execute` flag is required for live submission. `fill` skips weekends (Fri/Sat) unless `--include-weekends` is set. `clock-in` preserves existing exit time and comment data.

## Output modes

All commands support `--json` for machine-parseable JSON output. Human-readable tables are the default. Status/diagnostic messages go to stderr via `tracing`, data to stdout. Use `--verbose` for debug output, `--quiet` to suppress all diagnostics.

## MCP server

`hilan serve` starts an MCP server on stdio transport (JSON-RPC). Exposes 12 tools (8 read, 4 write with dry-run default). Each tool call creates a fresh authenticated client. Register in Claude Desktop or any MCP client:
```json
{ "mcpServers": { "hilan": { "command": "hilan", "args": ["serve"] } } }
```

## Adding a new command

1. Add subcommand variant to `Commands` enum in `main.rs`
2. Add clap fields (use `WriteMode` struct for any write operation)
3. Implement handler — use `client.get_aspx_form()` / `client.post_aspx_form()` for WebForms, `client.asmx_call()` for JSON API
4. Add `--json` branch using the `print_json` helper
5. Add `#[derive(Serialize)]` to any new output structs
6. Wire into the `match` in `main()`

## Plugin packaging

`.claude-plugin/plugin.json` defines this as a Claude Code plugin. `scripts/run.sh` reads version from there. Keep `Cargo.toml` version and `plugin.json` version in sync. Skills live in `skills/hilan/SKILL.md` with symlinks from `.claude/skills/` and `.agents/skills/`.

## Credentials

Credentials are stored in the OS keychain (`hilan-cli` service). Run `hilan auth` to set up. Legacy plaintext `config.toml` passwords are supported with migration via `hilan auth --migrate`. Binary must be codesigned on macOS for silent keychain access (`scripts/run.sh` handles this).
