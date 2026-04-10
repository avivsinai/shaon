# hilan

[![CI](https://github.com/avivsinai/hilan/actions/workflows/ci.yml/badge.svg)](https://github.com/avivsinai/hilan/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Rust: 1.80+](https://img.shields.io/badge/rust-1.80%2B-orange.svg)](https://www.rust-lang.org/)

Rust CLI for Hilan (חילן) attendance reporting, payslips, and HR automation.

> **Note**: This project automates the Hilanet web interface via reverse-engineered protocol.
> It is not affiliated with Hilan Ltd.

## Table of Contents

- [Features](#features)
- [Installation](#installation)
- [Quick Start](#quick-start)
- [Commands](#commands)
- [Configuration](#configuration)
- [Architecture](#architecture)
- [Safety Model](#safety-model)
- [AI Agent Integration](#ai-agent-integration)
- [Verifying Downloads](#verifying-downloads)
- [Contributing](#contributing)
- [License](#license)

## Features

- **15 commands** covering attendance, reports, payslips, and salary
- **Safe by default** — all write commands require `--execute` to submit
- **JSON output mode** for scripting and AI agents
- **OS keychain credential storage** with legacy plaintext migration
- **MCP server mode** for AI agent integration
- **Claude Code skill + plugin** for natural-language attendance automation
- **Full ASP.NET form replay** for attendance pages and error-wizard flows
- **Direct ASMX JSON calls** where Hilan exposes machine-friendly endpoints

For the protocol map and reverse-engineering notes, see [PROTOCOL.md](PROTOCOL.md).

## Installation

### From Source

```bash
git clone https://github.com/avivsinai/hilan.git
cd hilan
cargo build -p hilan --release
# Binary is at target/release/hilan
```

### Install With Cargo

```bash
cargo install --path .
```

### Pre-built Binaries

Download the latest release from [GitHub Releases](https://github.com/avivsinai/hilan/releases).

```bash
# macOS (Apple Silicon)
curl -LO https://github.com/avivsinai/hilan/releases/latest/download/hilan-aarch64-apple-darwin.tar.gz
tar xzf hilan-aarch64-apple-darwin.tar.gz
sudo mv hilan /usr/local/bin/

# macOS (Intel)
curl -LO https://github.com/avivsinai/hilan/releases/latest/download/hilan-x86_64-apple-darwin.tar.gz
tar xzf hilan-x86_64-apple-darwin.tar.gz
sudo mv hilan /usr/local/bin/

# Linux (x86_64)
curl -LO https://github.com/avivsinai/hilan/releases/latest/download/hilan-x86_64-unknown-linux-gnu.tar.gz
tar xzf hilan-x86_64-unknown-linux-gnu.tar.gz
sudo mv hilan /usr/local/bin/
```

### Local Development Wrapper

The repo includes a wrapper that builds and caches the release binary under
`~/.cache/hilan/<version>/hilan`:

```bash
./scripts/run.sh --help
```

For a stable local command name during development:

```bash
mkdir -p ~/bin
ln -sf "$PWD/scripts/run.sh" ~/bin/hilan
```

## Quick Start

```bash
# Verify credentials
hilan auth

# Cache attendance types for symbolic --type values
hilan sync-types

# Show the current month's attendance status
hilan status

# Preview a clock-in payload without submitting it
hilan clock-in

# Actually submit a clock-in
hilan clock-in --execute

# Download the previous month's payslip
hilan payslip

# Show recent salary totals
hilan salary --months 3
```

## Commands

### Setup and Discovery

| Command | Description |
|---------|-------------|
| `auth` | Authenticate with Hilan (test credentials) |
| `sync-types` | Sync attendance-type ontology from Hilan |
| `types` | Show cached attendance types |

### Attendance Reads

| Command | Description |
|---------|-------------|
| `status [--month YYYY-MM]` | Show monthly attendance status |
| `errors [--month YYYY-MM]` | Show attendance errors |
| `report <NAME>` | Fetch a named generic report page |
| `sheet` | Show hours analysis (`HoursAnalysis.aspx`) |
| `corrections` | Show correction log (`HoursReportLog.aspx`) |
| `absences` | Show initial absence symbols data |

### Attendance Writes

| Command | Description |
|---------|-------------|
| `clock-in [--type TYPE] [--execute]` | Clock in for today |
| `clock-out [--execute]` | Clock out for today |
| `fill --from DATE --to DATE [--type TYPE] [--hours HH:MM-HH:MM] [--execute]` | Fill attendance for a date range |
| `fix DATE [--type TYPE] [--hours HH:MM-HH:MM] [--report-id UUID] [--error-type N] [--execute]` | Fix a specific attendance error |

All write commands are **preview-only by default**. Pass `--execute` to submit.

### Payroll

| Command | Description |
|---------|-------------|
| `payslip [--month YYYY-MM] [--output PATH]` | Download payslip PDF |
| `salary [--months N]` | Show salary summaries |

## Configuration

`hilan` reads a TOML config file from:

| Path | Purpose |
|------|---------|
| `~/.hilan/config.toml` | Canonical config location |
| `~/.hilan/<subdomain>/` | Per-org state (`cookies.json`, `types.json`) |

Example:

```toml
subdomain = "YOUR_COMPANY"
username = "YOUR_ID_NUMBER"

# optional
payslip_folder = "/Users/you/Downloads/payslips"
payslip_format = "%Y-%m.pdf"
```

Notes:

- `subdomain` is the part before `.hilan.co.il`
- Run `hilan auth` to store the password in the OS keychain
- `hilan auth --migrate` moves a plaintext password from config into the keychain
- CAPTCHA is not bypassed; if Hilan asks for one, solve it in the browser first

## Architecture

```
hilan/
├── crates/
│   ├── hr-core/         # Provider-agnostic DTOs, traits, and use cases
│   ├── provider-hilan/  # Hilan transport, parsing, config, adapter, fixtures
│   ├── hilan-cli/       # CLI frontend
│   └── hilan-mcp/       # MCP frontend
├── src/
│   ├── lib.rs           # Compatibility facade re-exporting the workspace crates
│   └── main.rs          # Thin binary entrypoint
├── examples/
│   └── overview.rs      # Library consumer example
├── scripts/
│   └── run.sh           # Build-and-cache wrapper
├── skills/
│   └── hilan/SKILL.md   # Claude Code skill definition
├── .claude-plugin/
│   └── plugin.json      # Claude Code plugin manifest
└── tests/
    └── *.rs             # Facade and use-case integration tests
```

`hr-core` is the stable boundary for downstream code. `provider-hilan`
implements that contract over Hilan's ASP.NET and ASMX surfaces. The root
`hilan` crate intentionally stays as a convenience facade so existing
`hilan::core`, `hilan::provider`, `hilan::use_cases`, and `hilan::mcp`
imports keep working while the internal workspace stays modular.

For a minimal consumer, see [`examples/overview.rs`](examples/overview.rs).

## Safety Model

All write commands are **safe by default**.

- `clock-in`, `clock-out`, `fill`, and `fix` print the reconstructed request
  payload and **do not submit** anything until `--execute` is passed.
- Passing both `--execute` and `--dry-run` is an error.
- The CLI never mutates Hilan state without explicit opt-in.

## AI Agent Integration

### Claude Code Skill

The repo ships a Claude Code skill at `skills/hilan/SKILL.md` and a plugin
manifest at `.claude-plugin/plugin.json`. Install it with:

```bash
# Via skills.sh
npx skills add avivsinai/hilan

# Or install manually
/plugin marketplace add avivsinai/skills-marketplace
/plugin install hilan@avivsinai-marketplace
```

The skill triggers on keywords like "hilan", "attendance", "clock in/out",
"payslip", "salary", and "שעון נוכחות".

### Scripting

All commands can be used in shell scripts. Pass `--json` to emit structured
output for machine consumption.

### MCP Server

Run `hilan serve` to expose Hilan operations as MCP tools for AI agent
orchestration over stdio transport.

## Verifying Downloads

After downloading a release binary, verify its checksum:

```bash
# Download the checksums file
curl -LO https://github.com/avivsinai/hilan/releases/latest/download/SHA256SUMS.txt

# Verify (macOS)
shasum -a 256 -c SHA256SUMS.txt --ignore-missing

# Verify (Linux)
sha256sum -c SHA256SUMS.txt --ignore-missing
```

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for development setup and guidelines.

## Prior Art

- [zigius/hilan-bot](https://github.com/zigius/hilan-bot)
- [talsalmona/hilan](https://github.com/talsalmona/hilan)

## License

[MIT](LICENSE)
