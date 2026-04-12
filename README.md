# shaon

<p align="center">
  <img src="assets/logo.jpg" alt="Shaon — a clanker punching a time clock" width="200">
</p>

[![CI](https://github.com/avivsinai/shaon/actions/workflows/ci.yml/badge.svg)](https://github.com/avivsinai/shaon/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/avivsinai/shaon?display_name=tag)](https://github.com/avivsinai/shaon/releases)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Rust: 1.80+](https://img.shields.io/badge/rust-1.80%2B-orange.svg)](https://www.rust-lang.org/)

`shaon` is a Rust CLI, MCP server, and Claude Code plugin for Hilan / Hilanet. It covers attendance status, missing or broken days, reporting and correction flows, payslips, salary summaries, and related HR tasks.

> **Note**
> shaon automates the Hilanet web interface. It is not affiliated with Hilan Ltd.

## Install

### Homebrew

```bash
brew install avivsinai/tap/shaon
```

### Install Script

```bash
curl -fsSL https://raw.githubusercontent.com/avivsinai/shaon/main/scripts/install.sh | bash
```

### Cargo

```bash
cargo install --git https://github.com/avivsinai/shaon shaon
```

### Prebuilt Archives

Download a release archive from [GitHub Releases](https://github.com/avivsinai/shaon/releases), extract `shaon`, and place it on your `PATH`.

If you are working from a repo checkout, prefer:

```bash
scripts/run.sh <command> [args]
```

## One-Time Setup

Create `~/.shaon/config.toml`:

```toml
subdomain = "mycompany"
username = "123456789"

# optional
payslip_folder = "/Users/you/Downloads/payslips"
payslip_format = "%Y-%m.pdf"
```

Then authenticate once:

```bash
shaon auth
```

If Hilan asks for a CAPTCHA, solve it in the browser and rerun the command.

## Common Commands

### Read your current state

```bash
shaon attendance overview --month 2026-04
shaon attendance status --month 2026-04
shaon attendance errors --month 2026-04
shaon reports sheet
shaon reports corrections
shaon payroll salary --months 6
```

### Report or fix attendance

```bash
# Preview first
shaon attendance report day 2026-04-09 --type "regular" --hours 09:00-18:00
shaon attendance resolve 2026-04-09 --type "regular" --hours 09:00-18:00
shaon attendance auto-fill --month 2026-04 --type "work from home" --hours 09:00-18:00

# Then execute when the preview looks right
shaon attendance report day 2026-04-09 --type "regular" --hours 09:00-18:00 --execute
shaon attendance resolve 2026-04-09 --type "regular" --hours 09:00-18:00 --execute
shaon attendance auto-fill --month 2026-04 --type "work from home" --hours 09:00-18:00 --execute
```

### Payslips

```bash
shaon payroll payslip download --month 2026-03
shaon payroll payslip view --month 2026-03
shaon payroll payslip password
```

`payroll payslip password` prints the current Hilan password. Older downloaded PDFs may require the password that was current when they were downloaded.

## Safety

- Write commands preview by default.
- Use `--execute` in the CLI, or `execute: true` over MCP, only after reviewing the preview.
- Bulk flows such as `attendance auto-fill` stay capped unless you raise the limit explicitly.

For the exact live surface, use:

```bash
shaon --help
shaon <command> --help
```

## MCP Server

Start the server with:

```bash
shaon serve
```

Example MCP config with an installed binary:

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

From a repo checkout, use `scripts/run.sh` instead of `shaon`.

## Claude Code Plugin

This repo ships a Claude Code plugin manifest at `.claude-plugin/plugin.json` and a skill at `skills/shaon/SKILL.md`.

For local plugin development:

```bash
claude --plugin-dir /absolute/path/to/shaon
```

Inside Claude Code, the explicit skill name is:

```text
/shaon:shaon
```

Example:

```text
/shaon:shaon show my missing attendance days for 2026-04
```

## More Docs

- [ARCHITECTURE.md](ARCHITECTURE.md): crate boundaries and runtime surfaces
- [PROTOCOL.md](PROTOCOL.md): Hilanet protocol notes
- [CONTRIBUTING.md](CONTRIBUTING.md): contributor workflow
- [CLAUDE.md](CLAUDE.md): maintainer instructions for coding agents

## License

[MIT](LICENSE)
