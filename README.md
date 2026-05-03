# shaon

<p align="center">
  <img src="assets/logo.jpg" alt="Shaon — a clanker punching a time clock" width="200">
</p>

[![CI](https://github.com/avivsinai/shaon/actions/workflows/ci.yml/badge.svg)](https://github.com/avivsinai/shaon/actions/workflows/ci.yml)
[![Gitleaks](https://github.com/avivsinai/shaon/actions/workflows/gitleaks.yml/badge.svg)](https://github.com/avivsinai/shaon/actions/workflows/gitleaks.yml)
[![Release](https://img.shields.io/github/v/release/avivsinai/shaon?display_name=tag)](https://github.com/avivsinai/shaon/releases)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Rust: 1.80+](https://img.shields.io/badge/rust-1.80%2B-orange.svg)](https://www.rust-lang.org/)

`shaon` is a Rust CLI, MCP server, and agent skill for personal attendance, payslip, salary, and related self-service tasks on your own Hilan account.

## Features

- **One tool, three surfaces**: use `shaon` as a CLI, an MCP server (`shaon serve`), or an agent skill for Claude Code / Codex-compatible workflows.
- **Safety-first writes**: attendance writes preview by default and require explicit `--execute` for live submission.
- **Payslips and salary**: download or open password-protected payslips and fetch recent salary history.
- **Agent-friendly JSON**: status, overview, errors, reports, and payroll commands return stable structured output.
- **Release-ready distribution**: GitHub Releases publish checksummed binaries; Homebrew and Scoop are updated from the release workflow.

## Responsibility and Scope

You are responsible for all attendance submissions, payslip downloads, and credential handling performed with this tool. Use is conditional on your compliance with your employer's policies and your Hilanet customer's terms of service. `shaon` is intended for automating your own single-user account; multi-user, aggregation, and third-party-data use are out of scope.

`shaon` is an independent open-source project. It is not endorsed by, affiliated with, or sponsored by Hilan Ltd. Hilan and Hilanet are names and marks of their respective owner, used solely to identify compatibility and the target system. No trademark license is granted, and no claim of sponsorship, endorsement, or affiliation is made.

`shaon` is not payroll, tax, HR, legal, or employment-compliance advice. The software is provided "AS IS" under the MIT License, without warranty of any kind.

## Installation

### Homebrew (macOS / Linux)

```bash
brew install avivsinai/tap/shaon
```

### Scoop (Windows)

Windows bucket installs start with the first release produced by the PR-based release workflow in this repo. If the current latest release predates Windows packaging, wait for the next tagged release or build from source.

```powershell
scoop bucket add avivsinai https://github.com/avivsinai/scoop-bucket
scoop install shaon
```

### Install Script

```bash
curl -fsSL https://raw.githubusercontent.com/avivsinai/shaon/main/scripts/install.sh | bash
```

Pin a specific release if needed:

```bash
curl -fsSL https://raw.githubusercontent.com/avivsinai/shaon/main/scripts/install.sh | VERSION=v0.8.2 bash
```

### Cargo

```bash
cargo install --git https://github.com/avivsinai/shaon shaon
```

### Prebuilt Binaries

Download a release asset from [GitHub Releases](https://github.com/avivsinai/shaon/releases) and place the extracted `shaon` binary on your `PATH`.

```bash
# macOS (Apple Silicon)
curl -LO https://github.com/avivsinai/shaon/releases/latest/download/shaon-aarch64-apple-darwin.tar.gz
tar xzf shaon-aarch64-apple-darwin.tar.gz

# macOS (Intel)
curl -LO https://github.com/avivsinai/shaon/releases/latest/download/shaon-x86_64-apple-darwin.tar.gz
tar xzf shaon-x86_64-apple-darwin.tar.gz

# Linux (x86_64)
curl -LO https://github.com/avivsinai/shaon/releases/latest/download/shaon-x86_64-unknown-linux-gnu.tar.gz
tar xzf shaon-x86_64-unknown-linux-gnu.tar.gz
```

If you are working from a repo checkout, prefer:

```bash
scripts/run.sh <command> [args]
```

On macOS, `scripts/run.sh` signs the local build with the same stable
identifier-based designated requirement used by release artifacts. If you
previously approved an older local build in Keychain, macOS may ask for
"Always Allow" once after upgrading to this signing model.

### Agent Skill

Install the `shaon` skill for Claude Code or repo-native skill managers:

<details open>
<summary><b>Via skills.sh</b></summary>

```bash
npx skills add avivsinai/shaon -g -y
```

</details>

<details>
<summary><b>Via Skills Marketplace (Claude Code)</b></summary>

> **Known issue:** Claude Code marketplace installs currently clone over SSH, which can prompt for SSH keys unexpectedly. See [anthropics/claude-code#14485](https://github.com/anthropics/claude-code/issues/14485). If that affects your setup, prefer `skills.sh`.

```bash
/plugin marketplace add avivsinai/skills-marketplace
/plugin install shaon@avivsinai-marketplace
```

</details>

<details>
<summary><b>Manual install</b></summary>

```bash
git clone https://github.com/avivsinai/shaon.git
cp -r shaon/.claude/skills/shaon ~/.claude/skills/
```

</details>

Codex CLI does not currently support marketplace indirection. Use the repo-native skill layout or `skills.sh`.

## Quick Start

Create `~/.shaon/config.toml`:

```toml
subdomain = "mycompany"
username = "123456789"

# optional
payslip_folder = "/Users/you/Downloads/payslips"
payslip_format = "%Y-%m.pdf"
```

Authenticate once:

```bash
shaon auth
```

If stored credentials are stale and you want a deterministic refresh prompt:

```bash
shaon auth --force-prompt
```

If Hilan presents a CAPTCHA, solve it in the browser and rerun the command.

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
- `shaon attendance overview --json` returns `missing_days` as `{ date, day_name }` objects rather than bare strings.
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

`shaon payroll payslip password --force-sensitive-output` prints the current Hilan account password in plaintext to standard output. It does not recover historical passwords used for PDFs encrypted before a password change. Run it only on a private interactive terminal you control.

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

From a repo checkout, use `scripts/run.sh serve` instead of `shaon serve`.

## Safety

- Treat attendance writes as human-attested actions under your identity.
- Write commands preview by default.
- Use `--execute` in the CLI, or `execute: true` over MCP, only after reviewing the preview and explicitly deciding to submit it.
- Bulk flows such as `attendance auto-fill` stay capped unless you raise the limit explicitly.
- CAPTCHA challenges must be solved manually in the browser.

## Verifying Downloads

After downloading a release binary, verify its checksum:

```bash
curl -LO https://github.com/avivsinai/shaon/releases/latest/download/SHA256SUMS.txt

# macOS
shasum -a 256 -c SHA256SUMS.txt --ignore-missing

# Linux
sha256sum -c SHA256SUMS.txt --ignore-missing
```

## More Docs

- [ARCHITECTURE.md](ARCHITECTURE.md): crate boundaries and runtime surfaces
- [PROTOCOL.md](PROTOCOL.md): Hilanet protocol notes
- [CONTRIBUTING.md](CONTRIBUTING.md): contributor workflow
- [CLAUDE.md](CLAUDE.md): maintainer instructions for coding agents

## License

[MIT](LICENSE)
