# Hilan CLI

Unofficial Rust CLI for Hilan / Hilanet attendance, reports, payslips, and salary. Targets the legacy ASP.NET WebForms protocol and newer ASMX JSON endpoints.

## Architecture

Single crate with lib/bin split:

- `src/lib.rs` — re-exports for the library surface (`client`, `attendance`, `reports`, `ontology`, `api`, `config`)
- `src/main.rs` — CLI entry point (clap subcommands)
- `src/client.rs` — HTTP session layer (reqwest + cookie jar, ASP.NET form replay, ASMX JSON calls)
- `src/attendance.rs` — calendar parsing, write replay (`submit_day`, `fix_error_day`)
- `src/reports.rs` — generic report table fetching and HTML-to-table parsing
- `src/ontology.rs` — attendance-type ontology sync and cache
- `src/api.rs` — ASMX API wrappers (bootstrap, absences)
- `src/config.rs` — config loading, keychain integration

MCP server mode (`hilan serve`) uses `rmcp` for stdio transport, exposing all commands as MCP tools.

## Dependencies

Core:

- `clap` (derive) — CLI argument parsing
- `reqwest` (rustls-tls, cookies, json) — HTTP client
- `tokio` — async runtime
- `serde`, `serde_json`, `toml` — serialization
- `scraper` — HTML parsing
- `chrono` — date handling
- `anyhow` — error handling
- `regex` — pattern matching
- `directories` — platform config paths

Auth and secrets:

- `keyring` — system keychain access
- `secrecy` — zeroize-on-drop secret strings
- `zeroize` — memory scrubbing

Other:

- `urlencoding` — URL percent encoding
- `tracing` — structured logging
- `rmcp` — MCP server (stdio transport)
- `clap_complete` — shell completion generation

## Build and Test

```bash
cargo check
cargo clippy -- -D warnings
cargo fmt --check
cargo test
```

`cargo test` runs unit tests for parsing logic, config loading, and protocol helpers. Tests use fixtures under `tests/fixtures/`.

## Key Conventions

- Write commands default to dry-run. `--execute` required for live submission.
- `--json` flag available on all commands for machine-readable output.
- Exit codes: 0 = success, 1 = error.
- Config lives at the platform config directory (`~/Library/Application Support/com.hilan.hilan/` on macOS).
- Credentials stored in system keychain after `hilan auth`. Legacy plaintext config supported with `auth --migrate` path.
- `fill` skips Friday/Saturday by default (Israeli work week). `--include-weekends` overrides.

## MCP Server Mode

`hilan serve` starts an MCP server on stdio. All read commands are exposed as tools. Write tools require explicit `execute: true` parameter from the caller.
