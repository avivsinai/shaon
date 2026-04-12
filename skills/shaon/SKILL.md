---
name: shaon
description: >
  Automate Hilan (חילן) / Hilanet — Israeli HR & attendance system. Use this skill whenever the user
  mentions shaon, attendance reporting, presence, clock in/out, payslip, salary slip,
  work hours, שעון נוכחות, חילן, or any task related to their Israeli employer's HR portal.
  Also trigger when the user asks about their work schedule, missing attendance days, or wants
  to fill/fix attendance records — even if they don't say "shaon" explicitly.
---

# Shaon

Claude Code plugin skill for the `shaon` repo and binary.

## What To Use

- Use the **CLI** when you want explicit commands, JSON output, or shell scripting
- Use the **MCP server** when the client wants typed stdio tools
- Use this **skill** when the user is working inside Claude Code and wants natural-language help around attendance, payslips, or salary

## How To Load The Plugin

For local development against this repo:

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

The skill also auto-triggers on relevant keywords such as `shaon`, `attendance`, `clock in`, `payslip`, `salary`, `work hours`, and `שעון נוכחות`.

## CLI Entry Points

If working from a repo checkout, prefer:

```bash
scripts/run.sh <command> [args]
```

If installed globally:

```bash
shaon <command> [args]
```

On macOS, `scripts/run.sh` is the safest local path because it rebuilds and reuses the signed cached binary.

## First-Time Setup

Create `~/.shaon/config.toml`:

```toml
subdomain = "mycompany"
username = "123456789"
# password lives in keychain, not here
payslip_folder = "/path/to/payslips"   # optional
payslip_format = "%Y-%m.pdf"           # optional
```

Then authenticate:

```bash
shaon auth
```

If the config still contains a plaintext password:

```bash
shaon auth --migrate
```

## Core CLI Commands

### Read commands

| Command | Example |
|---------|---------|
| `status` | `shaon status --month 2026-04` |
| `errors` | `shaon errors --month 2026-04` |
| `overview` | `shaon overview --month 2026-04 --json` |
| `types` | `shaon types` |
| `absences` | `shaon absences` |
| `sheet` | `shaon sheet` |
| `corrections` | `shaon corrections` |
| `report <name>` | `shaon report ErrorsReportNEW` |
| `payslip` | `shaon payslip --month 2026-03` |
| `salary` | `shaon salary --months 6` |

### Write commands

All writes are preview-only by default. Use `--execute` to submit.

| Command | Example |
|---------|---------|
| `clock-in` | `shaon clock-in --execute` |
| `clock-out` | `shaon clock-out --execute` |
| `fill` | `shaon fill --from 2026-04-01 --to 2026-04-05 --type "work from home" --hours 09:00-18:00 --execute` |
| `fix` | `shaon fix 2026-04-08 --type "regular" --hours 09:00-18:00 --execute` |
| `auto-fill` | `shaon auto-fill --month 2026-04 --type "regular" --hours 09:00-18:00 --execute` |

## Output Modes

Use `--json` whenever an agent needs machine-readable output:

```bash
shaon overview --month 2026-04 --json
shaon status --month 2026-04 --json
```

## MCP Server

The repo also ships a stdio MCP server:

```bash
shaon serve
```

Current MCP tools cover:

- status
- errors
- types
- clock-in / clock-out
- fill / auto-fill
- salary
- sheet / corrections / absences
- overview

CLI-only features today:

- `payslip`
- `report`
- `fix`
- `auth`
- `sync-types`
- `completions`

## Agent Workflows

### Review the current month

```bash
shaon overview --json
```

### Auto-fill missing days

```bash
shaon auto-fill --month 2026-04 --type "regular" --hours 09:00-18:00
shaon auto-fill --month 2026-04 --type "regular" --hours 09:00-18:00 --execute
```

### Download a payslip

```bash
shaon payslip --month 2026-03 --output ~/Downloads/2026-03.pdf
```

## Troubleshooting

- **CAPTCHA**: the user must solve it in the browser first
- **Type resolution fails**: run `shaon sync-types` or pass the numeric type code directly
- **Keychain prompts on macOS**: prefer `scripts/run.sh`; for stable local signing run `scripts/setup-codesign.sh` once
- **Headless automation**: use `SHAON_PASSWORD` and `SHAON_SESSION_KEY`
- **Session expired**: rerun `shaon auth` if automatic reauthentication is not enough

## Further Reading

- [README.md](../../README.md)
- [ARCHITECTURE.md](../../ARCHITECTURE.md)
- [PROTOCOL.md](../../PROTOCOL.md)
