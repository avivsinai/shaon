# shaon

<p align="center">
  <img src="assets/logo.jpg" alt="Shaon — a clanker punching a time clock" width="200">
</p>

[![CI](https://github.com/avivsinai/shaon/actions/workflows/ci.yml/badge.svg)](https://github.com/avivsinai/shaon/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/avivsinai/shaon?display_name=tag)](https://github.com/avivsinai/shaon/releases)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Rust: 1.80+](https://img.shields.io/badge/rust-1.80%2B-orange.svg)](https://www.rust-lang.org/)

Rust CLI + MCP server + Claude Code plugin for automating Hilan (חילן) / Hilanet attendance, payslips, salary summaries, and related HR workflows.

> **Note**
> shaon automates the Hilanet web interface via reverse-engineered protocol details. It is not affiliated with Hilan Ltd.

## Documentation

- [README.md](README.md): end-user install, setup, CLI, MCP, and Claude Code usage
- [ARCHITECTURE.md](ARCHITECTURE.md): crate boundaries, runtime surfaces, and maintenance map
- [PROTOCOL.md](PROTOCOL.md): low-level Hilanet HTTP / WebForms / ASMX protocol notes
- [skills/shaon/SKILL.md](skills/shaon/SKILL.md): the Claude Code skill text shipped by this repo
- [CLAUDE.md](CLAUDE.md): maintainer instructions for coding agents and contributors
- [CONTRIBUTING.md](CONTRIBUTING.md): contributor workflow and safety requirements

## What Shaon Covers

- Attendance reads: status, errors, overview, sheet, corrections, absences
- Attendance writes: clock in/out, fill ranges, fix error days, auto-fill missing days
- Payroll reads: payslip download and salary summaries
- Automation surfaces: JSON CLI output, stdio MCP server, Claude Code plugin/skill
- Safe defaults: every write path is preview-only until you pass `--execute` or `execute: true`

## Installation

### Quick Install Script

```bash
curl -fsSL https://raw.githubusercontent.com/avivsinai/shaon/main/scripts/install.sh | bash
```

The installer downloads the latest release asset, verifies `SHA256SUMS.txt`, and installs `shaon` into a user-local bin directory.

### Homebrew

```bash
brew install avivsinai/tap/shaon
```

### Prebuilt Archives

Download the latest release from [GitHub Releases](https://github.com/avivsinai/shaon/releases).

```bash
# macOS (Apple Silicon)
curl -LO https://github.com/avivsinai/shaon/releases/latest/download/shaon-aarch64-apple-darwin.tar.gz
tar xzf shaon-aarch64-apple-darwin.tar.gz
sudo mv shaon /usr/local/bin/

# macOS (Intel)
curl -LO https://github.com/avivsinai/shaon/releases/latest/download/shaon-x86_64-apple-darwin.tar.gz
tar xzf shaon-x86_64-apple-darwin.tar.gz
sudo mv shaon /usr/local/bin/

# Linux (x86_64)
curl -LO https://github.com/avivsinai/shaon/releases/latest/download/shaon-x86_64-unknown-linux-gnu.tar.gz
tar xzf shaon-x86_64-unknown-linux-gnu.tar.gz
sudo mv shaon /usr/local/bin/
```

### Build From Source

```bash
git clone https://github.com/avivsinai/shaon.git
cd shaon
cargo build -p shaon --release
```

The binary will be at `target/release/shaon`.

### Development Wrapper

For repo checkouts, especially on macOS, prefer the wrapper:

```bash
./scripts/run.sh --help
```

It builds, caches, and launches the release binary from `~/.cache/shaon/<version>/shaon`.

### macOS Signing and Keychain Behavior

GitHub release and Homebrew binaries are ad-hoc signed in CI because this project does not ship with an Apple Developer ID certificate. That is enough for Keychain access, but the code hash changes on each upgrade, so macOS may ask you to re-approve access after installing a new release.

For stable local identities across rebuilds:

```bash
./scripts/setup-codesign.sh
./scripts/run.sh status
```

For headless or CI automation, prefer `SHAON_PASSWORD` and `SHAON_SESSION_KEY` instead of interactive keychain access.

## Quick Start

### 1. Create `~/.shaon/config.toml`

```toml
subdomain = "mycompany"
username = "123456789"

# optional
payslip_folder = "/Users/you/Downloads/payslips"
payslip_format = "%Y-%m.pdf"
```

### 2. Authenticate

```bash
shaon auth
```

This stores the password in the OS keychain and verifies the login.

### 3. Common Workflows

```bash
# Full monthly context: identity, errors, missing days, suggestions
shaon overview --month 2026-04

# Same, but machine-readable
shaon overview --month 2026-04 --json

# Show the attendance calendar for a month
shaon status --month 2026-04

# Preview an auto-fill run, then execute it
shaon auto-fill --month 2026-04 --type "work from home" --hours 09:00-18:00
shaon auto-fill --month 2026-04 --type "work from home" --hours 09:00-18:00 --execute

# Clock in / out
shaon clock-in --execute
shaon clock-out --execute

# Download the previous month's payslip (default month)
shaon payslip

# Download a specific payslip to a chosen path
shaon payslip --month 2026-03 --output ~/Downloads/2026-03.pdf

# Show the last 2 salary months (CLI default)
shaon salary
shaon salary --months 6
```

## Configuration and Credentials

### Files and Directories

| Path | Purpose |
|------|---------|
| `~/.shaon/config.toml` | canonical config |
| `~/.shaon/<subdomain>/cookies.json` | encrypted session cookies |
| `~/.shaon/<subdomain>/types.json` | cached attendance type ontology |

### Credential Sources

shaon reads secrets in this order:

1. `SHAON_PASSWORD` / `SHAON_SESSION_KEY`
2. OS keychain entries
3. legacy plaintext password in `config.toml` only for migration

### Notes

- Session cookies are encrypted at rest with AES-256-GCM
- `shaon auth --migrate` moves a legacy plaintext password into the keychain
- If Hilan asks for a CAPTCHA, solve it in the browser and retry
- `sync-types` is optional; attendance types auto-sync on first use

## CLI Guide

For the exact live surface, use:

```bash
shaon --help
shaon <command> --help
```

### Setup and Utility Commands

| Command | Purpose |
|---------|---------|
| `auth` | test credentials and store password in keychain |
| `sync-types` | refresh the attendance type cache |
| `types` | show currently known attendance types |
| `completions <shell>` | generate shell completions |
| `serve` | start the stdio MCP server |

### Attendance Read Commands

| Command | Purpose |
|---------|---------|
| `status [--month YYYY-MM]` | monthly attendance calendar |
| `errors [--month YYYY-MM]` | error days for a month |
| `overview [--month YYYY-MM] [--detailed]` | identity, summary, errors, missing days, suggestions |
| `sheet` | analyzed attendance sheet (`HoursAnalysis.aspx`) |
| `corrections` | correction log (`HoursReportLog.aspx`) |
| `absences` | absence symbols and display names |
| `report <name>` | fetch a named Hilan report page and parse the first meaningful HTML table |

### Attendance Write Commands

| Command | Purpose |
|---------|---------|
| `clock-in [--type TYPE] [--execute]` | report entry time for today |
| `clock-out [--execute]` | report exit time for today |
| `fill --from DATE --to DATE [--type TYPE] [--hours HH:MM-HH:MM] [--include-weekends] [--execute]` | fill a date range |
| `fix DATE [--type TYPE] [--hours HH:MM-HH:MM] [--report-id UUID] [--error-type N] [--execute]` | fix a specific error day |
| `auto-fill [--month YYYY-MM] --type TYPE [--hours HH:MM-HH:MM] [--include-weekends] [--max-days N] [--execute]` | fill all missing days in a month |

All write commands are preview-only by default.

### Payroll Commands

| Command | Purpose |
|---------|---------|
| `payslip [--month YYYY-MM] [--output PATH]` | download a payslip PDF; defaults to the previous month |
| `salary [--months N]` | salary summary; defaults to `2` months |

### JSON Output

All CLI commands support `--json`.

```bash
shaon status --month 2026-04 --json | jq '.days[] | select(.error == true)'
```

## MCP Server

`shaon serve` exposes a stdio MCP server implemented in `crates/shaon-mcp`.

### Current MCP Tools

| Tool | Purpose |
|------|---------|
| `shaon_status` | monthly attendance calendar |
| `shaon_errors` | error days only |
| `shaon_types` | attendance types |
| `shaon_clock_in` | preview or submit clock-in |
| `shaon_clock_out` | preview or submit clock-out |
| `shaon_fill` | preview or submit a date-range fill |
| `shaon_auto_fill` | preview or submit auto-fill |
| `shaon_salary` | salary summary |
| `shaon_sheet` | analyzed attendance sheet |
| `shaon_corrections` | correction log |
| `shaon_absences` | absence symbols |
| `shaon_overview` | monthly overview |

Current CLI-only capabilities:

- `payslip`
- `report`
- `fix`
- `auth`
- `sync-types`
- `completions`

### MCP Client Configuration Example

Using the repo wrapper:

```json
{
  "mcpServers": {
    "shaon": {
      "command": "/absolute/path/to/shaon/scripts/run.sh",
      "args": ["serve"]
    }
  }
}
```

Using an installed binary:

```json
{
  "mcpServers": {
    "shaon": {
      "command": "shaon",
      "args": ["serve"]
    }
  }
}
```

### MCP Behavior Notes

- Tool results are JSON payloads
- Tool errors are wrapped in an `error` envelope with `code`, `message`, `retryable`, and optional `details`
- Write tools stay in preview mode unless the request includes `execute: true`
- Each tool loads local config and keychain state on demand

## Claude Code Plugin and Skill

This repo ships a Claude Code plugin manifest at `.claude-plugin/plugin.json` and a skill at `skills/shaon/SKILL.md`.

### Local Plugin Development

```bash
claude --plugin-dir /absolute/path/to/shaon
```

Once Claude Code starts, the explicit plugin skill name is:

```text
/shaon:shaon
```

Example:

```text
/shaon:shaon show my missing attendance days for 2026-04
```

The skill also auto-triggers on keywords like `shaon`, `attendance`, `clock in`, `payslip`, `salary`, `work hours`, and `שעון נוכחות`.

### Why Use the Skill vs MCP Directly

- Use the **CLI** when you want explicit shell commands or scripting
- Use the **MCP server** when your agent platform wants typed tools over stdio
- Use the **Claude Code plugin/skill** when you want natural-language workflows inside Claude Code

For plugin development guidance, see the official Claude Code plugin docs:

- https://code.claude.com/docs/en/plugins
- https://code.claude.com/docs/en/discover-plugins

## Architecture

For the maintainable view of the codebase, read [ARCHITECTURE.md](ARCHITECTURE.md).

Short version:

- `crates/hr-core`: provider-agnostic traits, DTOs, and use-cases
- `crates/provider-hilan`: Hilan-specific transport, parsing, session, config, and protocol replay
- `crates/shaon-cli`: human-facing CLI
- `crates/shaon-mcp`: stdio MCP server
- root `src/`: compatibility facade and binary entrypoint

For endpoint-level details, see [PROTOCOL.md](PROTOCOL.md).

## Verifying Downloads

After downloading a release binary, verify its checksum:

```bash
curl -LO https://github.com/avivsinai/shaon/releases/latest/download/SHA256SUMS.txt

# macOS
shasum -a 256 -c SHA256SUMS.txt --ignore-missing

# Linux
sha256sum -c SHA256SUMS.txt --ignore-missing
```

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md).

## License

[MIT](LICENSE)
