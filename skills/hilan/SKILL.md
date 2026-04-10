---
name: hilan
description: >
  Automate Hilan (חילן) / Hilanet — Israeli HR & attendance system. Use this skill whenever the user
  mentions hilan, hilanet, attendance reporting, presence, clock in/out, payslip, salary slip,
  work hours, שעון נוכחות, חילן, or any task related to their Israeli employer's HR portal.
  Also trigger when the user asks about their work schedule, missing attendance days, or wants
  to fill/fix attendance records — even if they don't say "hilan" explicitly.
---

# Hilan CLI

Rust CLI for automating Hilanet — attendance reporting, payslips, salary, and HR data.

## How to run

If installed as a plugin:
```bash
plugins/hilan/scripts/run.sh <command> [args]
```

If installed as a binary (cargo install, brew, or direct):
```bash
hilan <command> [args]
```

The wrapper script auto-builds from source, caches the binary, and codesigns on macOS.

## First-time setup

Create `~/.hilan/config.toml` with the non-secret settings:

```toml
subdomain = "mycompany"        # the part before .hilan.co.il
username = "123456789"         # Israeli ID number
# password lives in keychain, not here
payslip_folder = "/path/to/payslips"   # optional
payslip_format = "%Y-%m.pdf"           # optional
```

Then run:

```bash
hilan auth
```

This tests the configured account and stores the password in the OS keychain. No plaintext passwords are required on disk.

If the config still contains a plaintext password field, migrate it explicitly:
```bash
hilan auth --migrate
```

## Output modes

All commands support `--json` for machine-parseable output. Always use `--json` when you need to process the results programmatically.

```bash
hilan status --month 2026-04 --json    # structured JSON to stdout
hilan status --month 2026-04           # human-readable table
```

## Safety model

All write commands default to **dry-run preview**. You must pass `--execute` to actually submit. Never assume execution happened unless you passed `--execute` AND the CLI returned success.

- `fill` automatically skips weekends (Fri/Sat). Use `--include-weekends` to override.
- `clock-in` preserves existing exit time and comment data.

## Commands

### Read commands (safe, no side effects)

| Command | What it does | Example |
|---------|-------------|---------|
| `status` | Monthly attendance calendar | `hilan status --month 2026-04` |
| `errors` | Days with attendance errors | `hilan errors --month 2026-04` |
| `types` | List cached attendance types | `hilan types` |
| `report` | Fetch a named report | `hilan report ErrorsReportNEW` |
| `sheet` | Hours analysis sheet | `hilan sheet` |
| `corrections` | Correction/change log | `hilan corrections` |
| `absences` | Absence type symbols | `hilan absences` |
| `payslip` | Download payslip PDF | `hilan payslip --month 2026-03` |
| `salary` | Salary summary with trend | `hilan salary --months 3` |
| `overview` | Full context in one call (identity, summary, types, errors, suggestions) | `hilan overview --json` |

### Write commands (require `--execute` for live submission)

| Command | What it does | Example |
|---------|-------------|---------|
| `clock-in` | Report entry for today | `hilan clock-in --execute` |
| `clock-out` | Report exit for today | `hilan clock-out --execute` |
| `fill` | Fill attendance for date range | `hilan fill --from 2026-04-01 --to 2026-04-05 --type "Work from Home" --execute` |
| `fix` | Fix an error day | `hilan fix 2026-04-08 --type "regular" --hours 09:00-18:00 --execute` |
| `auto-fill` | Batch fill all missing days in a month | `hilan auto-fill --month 2026-04 --type "regular" --hours 09:00-18:00 --execute` |

### Setup & utility commands

| Command | What it does |
|---------|-------------|
| `auth` | Set up keychain credentials (interactive) |
| `auth --migrate` | Move plaintext password to keychain |
| `sync-types` | Refresh attendance type cache (auto-syncs with 24h TTL) |
| `completions` | Generate shell completions (`bash`, `zsh`, `fish`) |
| `serve` | Start MCP server (stdio transport) for AI agent integration |

### Available report names

Use these with `hilan report <name>`:
- `ErrorsReportNEW` — attendance errors
- `MissingReportNEW` — missing reports
- `AttendanceStatusReportNew2` — attendance status
- `AbsenceReportNEW` — absences
- `AllReportNEW` — all attendance data
- `ManualReportingReportNEW` — manual corrections

## Common agent workflows

### Fastest path: overview + auto-fill (recommended)

```bash
# 1. Get full context in one call — identity, summary, types, errors, suggestions
hilan overview --json

# 2. Auto-fill all missing days (preview first)
hilan auto-fill --month 2026-04 --type "regular" --hours 09:00-18:00

# 3. Execute if preview looks right
hilan auto-fill --month 2026-04 --type "regular" --hours 09:00-18:00 --execute
```

`auto-fill` has a safety cap of 10 days by default. Use `--max-days N` to override.

### Manual path: status + fill

```bash
# 1. See current month status
hilan status --month 2026-04 --json

# 2. Check for errors
hilan errors --month 2026-04 --json

# 3. Fill specific days
hilan fill --from 2026-04-07 --to 2026-04-11 --type "regular" --hours 09:00-18:00 --execute
```

### Quick clock in/out

```bash
hilan clock-in --execute
# ... at end of day ...
hilan clock-out --execute
```

### Download payslip

```bash
hilan payslip --month 2026-03 --output ~/Downloads/march.pdf
```

## Troubleshooting

- **CAPTCHA**: If login returns a CAPTCHA error, the user must solve it in their browser at `https://<subdomain>.hilan.co.il` and retry.
- **Type resolution fails**: Run `hilan sync-types` to refresh the cache, or pass a numeric type code directly.
- **Keychain access prompts on macOS**: The binary needs codesigning. `scripts/run.sh` handles this automatically. If running from `cargo run`, sign manually: `codesign -s - target/release/hilan`.
- **Session expired**: The CLI re-authenticates automatically. If it persists, run `hilan auth` to refresh credentials.

## Protocol reference

For endpoint details and the reverse-engineered Hilanet protocol, see `@PROTOCOL.md`.
