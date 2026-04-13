---
name: shaon
description: >
  Personal attendance, payslip, salary, and related self-service tasks on the
  user's own Hilan account. Trigger on shaon, attendance, clock in/out, payslip,
  salary slip, work hours, תלוש, תלוש שכר, משכורת, דוח נוכחות, corrections log,
  forgot to clock in, forgot to report, or any request about the user's own
  Israeli employer self-service portal.
---

# Shaon

Claude Code skill for the `shaon` repo and binary.

## Tool Selection

- Prefer the **MCP server** when a tool already covers the domain operation.
- Fall back to the **CLI** for local-machine or interactive operations:
  `auth`, `payroll payslip view`, `reports show`, `cache refresh attendance-types`,
  `serve`, and `completions`.
- Use this **skill** to pick the right surface, the right command/tool, and the
  right safety flow.

## Human-attested writes

All write commands (`attendance report today/day/range`, `attendance auto-fill`, `attendance resolve`) submit under the user's identity against their real employer's system. When driving these, ALWAYS:

1. Run with `--dry-run` (the default) first.
2. Show the resulting preview to the user.
3. Require the user to explicitly confirm in the conversation before rerunning with `--execute`.

Never autonomously rerun a write with `--execute`. The legal actor is the user, not the agent.

## CLI Entry Point

If working from a repo checkout, prefer:

```bash
scripts/run.sh <command> [args]
```

If installed globally:

```bash
shaon <command> [args]
```

## First-Time Setup

Create `~/.shaon/config.toml`:

```toml
subdomain = "mycompany"
username = "123456789"
payslip_folder = "/path/to/payslips"   # optional
payslip_format = "%Y-%m.pdf"           # optional
```

Then authenticate:

```bash
shaon auth
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
| "how much did I earn recently?" | `shaon payroll salary --months N` |

`attendance overview` is usually the best first move for an agent because it bundles identity, summary, errors, missing days, and suggested actions.

JSON contract notes:

- `attendance status --json` returns `{ month, employee_id, days[] }` with `day_name`, `entry_time`, `exit_time`, `attendance_type`, `total_hours`, `has_error`, `error_message`, and `source`.
- `attendance overview --json` returns `missing_days` as `{ date, day_name }` objects.
- `suggested_actions` is keyed by `kind`; fields are top-level, not nested inside a generic `params` object.
- `attendance overview --json --detailed` adds a `days[]` array using the same schema as `attendance status --json`.

## Write Commands

### Explicit reporting

| Intent | Command |
|--------|---------|
| clock in now | `shaon attendance report today --in` |
| clock out now | `shaon attendance report today --out` |
| report one explicit day | `shaon attendance report day YYYY-MM-DD --type "regular" --hours 09:00-18:00` |
| report a range | `shaon attendance report range --from YYYY-MM-DD --to YYYY-MM-DD --type "regular" --hours 09:00-18:00` |
| auto-fill missing days in a month | `shaon attendance auto-fill --month YYYY-MM --type "regular" --hours 09:00-18:00` |
| resolve an existing error day | `shaon attendance resolve YYYY-MM-DD --type "regular" --hours 09:00-18:00` |

### Safety Model

- Every write is **preview-only by default**.
- The agent should show the preview summary to the user before rerunning with `--execute` or `execute: true`.
- The agent must wait for explicit user confirmation before any live submit.
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
# After explicit user confirmation:
shaon attendance resolve 2026-04-09 --type "regular" --hours 09:00-18:00 --execute
```

### Fill a missing range safely

```bash
shaon attendance report range --from 2026-04-01 --to 2026-04-05 --type "regular" --hours 09:00-18:00
# After explicit user confirmation:
shaon attendance report range --from 2026-04-01 --to 2026-04-05 --type "regular" --hours 09:00-18:00 --execute
```

### Hebrew payslip workflow

```bash
shaon payroll payslip download --month 2026-03
shaon payroll payslip view --month 2026-03
```

### Sensitive recovery command

`shaon payroll payslip password` reveals the current Hilan login password in plaintext. Use it only if you explicitly need to open an older password-protected PDF. Avoid shared terminals, screenshots, and agent transcripts.

## Troubleshooting

- **CAPTCHA**: the user must solve it in the browser first.
- **Type resolution fails**: run `shaon cache refresh attendance-types` or pass the numeric type code directly.
- **Session expired**: rerun `shaon auth` if automatic reauthentication is not enough.

## Further Reading

- [README.md](../../README.md)
- [ARCHITECTURE.md](../../ARCHITECTURE.md)
- [PROTOCOL.md](../../PROTOCOL.md)
