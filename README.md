# shaon

<p align="center">
  <img src="assets/logo.jpg" alt="Shaon — a clanker punching a time clock" width="200">
</p>

[![CI](https://github.com/avivsinai/shaon/actions/workflows/ci.yml/badge.svg)](https://github.com/avivsinai/shaon/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/avivsinai/shaon?display_name=tag)](https://github.com/avivsinai/shaon/releases)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Rust: 1.80+](https://img.shields.io/badge/rust-1.80%2B-orange.svg)](https://www.rust-lang.org/)

`shaon` is a Rust CLI, MCP server, and Claude Code plugin for personal attendance, payslip, salary, and related self-service tasks on the user's own Hilan account.

## Responsibility and Scope

You are responsible for all attendance submissions, payslip downloads, and credential handling performed with this tool. Use is conditional on your compliance with your employer's policies and your Hilanet customer's terms of service. `shaon` is intended for automating your own single-user account; multi-user, aggregation, and third-party-data use are out of scope.

`shaon` is an independent open-source project. It is not endorsed by, affiliated with, or sponsored by Hilan Ltd.
Hilan and Hilanet are names and marks of their respective owner, used solely to identify compatibility and the target system. No trademark license is granted, and no claim of sponsorship, endorsement, or affiliation is made.

`shaon` is not payroll, tax, HR, legal, or employment-compliance advice, and it is not warranted to produce complete, accurate, employer-accepted, or legally sufficient records.

The software is provided "AS IS" under the MIT License, without warranty of any kind, express or implied, including but not limited to the warranties of merchantability, fitness for a particular purpose, and noninfringement. In no event shall the authors or copyright holders be liable for any claim, damages, or other liability, whether in an action of contract, tort, or otherwise, arising from, out of, or in connection with the software or the use or other dealings in the software.

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

If you need to replace stale stored credentials deterministically, run:

```bash
shaon auth --force-prompt
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

JSON contract notes for agents:

- `shaon attendance status --json` returns `{ month, employee_id, days[] }` with `day_name`, `entry_time`, `exit_time`, `attendance_type`, `total_hours`, `has_error`, `error_message`, and `source`.
- `shaon attendance overview --json` returns `missing_days` as objects `{ date, day_name }`, not bare strings.
- `suggested_actions` is a tagged union keyed by `kind`; action fields live at the top level rather than inside a generic `params` bag.
- `shaon attendance overview --json --detailed` adds a top-level `days[]` array using the same schema as `attendance status --json`.

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
```

### Sensitive recovery command

`shaon payroll payslip password --force-sensitive-output` prints the current Hilan account password in plaintext to standard output. It does not recover historical passwords used for PDFs encrypted before a password change. Output may be captured by shells, terminals, logs, remote sessions, screenshots, and agent transcripts. Run it only on a private interactive terminal you control.

## Safety

- Treat attendance writes as human-attested actions under your identity.
- Write commands preview by default.
- Use `--execute` in the CLI, or `execute: true` over MCP, only after reviewing the preview and explicitly deciding to submit it.
- Bulk flows such as `attendance auto-fill` stay capped unless you raise the limit explicitly.
- CAPTCHA challenges must be solved manually in the browser.

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
