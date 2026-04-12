---
name: shaon
description: >
  Automate Hilan (חילן) / Hilanet for attendance, work hours, missing clock-ins,
  corrections, payslips, salary, and HR self-service. Trigger on shaon, attendance,
  clock in/out, payslip, salary slip, work hours, תלוש, תלוש שכר, משכורת,
  דוח נוכחות, corrections log, forgot to clock in, forgot to report, or any request
  about an Israeli employer's Hilan portal.
---

# Shaon

Claude Code skill for the `shaon` repo and binary.

## Tool Selection

- Prefer the **MCP server** when a tool already covers the domain operation.
- Fall back to the **CLI** for local-machine or interactive operations:
  `auth`, `payroll payslip view`, `payroll payslip password`, `reports show`,
  `cache refresh attendance-types`, `serve`, and `completions`.
- Use this **skill** to pick the right surface, the right command/tool, and the
  right safety flow.

## CLI Entry Point

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
# password lives in the keychain, not here
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

## Pick The Right Read Command

| User intent | Use |
|-------------|-----|
| "what's my current month state?" | `shaon attendance overview --json` |
| "show me the raw calendar / what did Hilan record per day?" | `shaon attendance status --month YYYY-MM` |
| "what errors do I need to fix?" | `shaon attendance errors --month YYYY-MM` |
| "what attendance types can I report?" | `shaon attendance types` |
| "what absence symbols mean?" | `shaon attendance absences` |
| "show the analyzed hours sheet" | `shaon reports sheet` |
| "show the manual correction log / audit trail" | `shaon reports corrections` |
| "show a named Hilan report page" | `shaon reports show <name>` |
| "download my payslip" | `shaon payroll payslip download --month YYYY-MM` |
| "open my payslip locally" | `shaon payroll payslip view --month YYYY-MM` |
| "what is the payslip PDF password?" | `shaon payroll payslip password` |
| "how much did I earn recently?" | `shaon payroll salary --months N` |

`attendance overview` is usually the best first move for an agent because it bundles identity, summary, errors, missing days, and suggested actions.

## Write Commands

### Explicit reporting

| Intent | Command |
|--------|---------|
| clock in now | `shaon attendance report today --in --execute` |
| clock out now | `shaon attendance report today --out --execute` |
| report one explicit day | `shaon attendance report day YYYY-MM-DD --type "regular" --hours 09:00-18:00` |
| report a range | `shaon attendance report range --from YYYY-MM-DD --to YYYY-MM-DD --type "regular" --hours 09:00-18:00` |
| auto-fill missing days in a month | `shaon attendance auto-fill --month YYYY-MM --type "regular" --hours 09:00-18:00` |
| resolve an existing error day | `shaon attendance resolve YYYY-MM-DD --type "regular" --hours 09:00-18:00` |

### Safety Model

- Every write is **preview-only by default**.
- The agent should show the preview summary to the user before rerunning with `--execute` or `execute: true`.
- `attendance report range` and `attendance auto-fill` skip Fri/Sat unless explicitly overridden.
- `attendance auto-fill` is capped at `--max-days 10` by default to prevent accidental bulk edits.

## MCP Coverage

Current MCP tools cover:

- `shaon_status`
- `shaon_errors`
- `shaon_types`
- `shaon_clock_in`
- `shaon_clock_out`
- `shaon_fill`
- `shaon_auto_fill`
- `shaon_resolve`
- `shaon_payslip_download`
- `shaon_salary`
- `shaon_sheet`
- `shaon_corrections`
- `shaon_absences`
- `shaon_overview`

CLI-only features:

- `shaon auth`
- `shaon payroll payslip view`
- `shaon payroll payslip password`
- `shaon reports show <name>`
- `shaon cache refresh attendance-types`
- `shaon serve`
- `shaon completions`

## Agent Workflows

### Review the month first

```bash
shaon attendance overview --json
```

### Fix an attendance error safely

```bash
shaon attendance errors --month 2026-04 --json
shaon attendance resolve 2026-04-09 --type "regular" --hours 09:00-18:00
shaon attendance resolve 2026-04-09 --type "regular" --hours 09:00-18:00 --execute
```

### Fill a missing range safely

```bash
shaon attendance report range --from 2026-04-01 --to 2026-04-05 --type "regular" --hours 09:00-18:00
shaon attendance report range --from 2026-04-01 --to 2026-04-05 --type "regular" --hours 09:00-18:00 --execute
```

### Hebrew payslip workflow

```bash
shaon payroll payslip download --month 2026-03
shaon payroll payslip view --month 2026-03
shaon payroll payslip password
```

## Troubleshooting

- **CAPTCHA**: the user must solve it in the browser first.
- **Type resolution fails**: run `shaon cache refresh attendance-types` or pass the numeric type code directly.
- **Keychain prompts on macOS**: prefer `scripts/run.sh`; for stable local signing run `scripts/setup-codesign.sh` once.
- **Advanced headless automation**: use `SHAON_PASSWORD` and `SHAON_MASTER_KEY`.
- **Session expired**: rerun `shaon auth` if automatic reauthentication is not enough.

## Further Reading

- [README.md](../../README.md)
- [ARCHITECTURE.md](../../ARCHITECTURE.md)
- [PROTOCOL.md](../../PROTOCOL.md)
