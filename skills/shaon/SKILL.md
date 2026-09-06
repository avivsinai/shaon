---
name: shaon
version: 0.9.3
description: Manage personal Hilan attendance, corrections, payslips, and salary data through shaon.
---

# Shaon

Use the MCP tool when it covers the requested operation. Use the `shaon` CLI
for unsupported operations, diagnosis, or when the user asks for the CLI.
Inspect `shaon --help` or the relevant subcommand help for exact flags.

## Choose the task

- Month status, missing days, and errors: start with `shaon attendance overview`.
- Daily or monthly attendance details: use `shaon attendance` read commands.
- Corrections: preview the relevant `attendance report` or `attendance resolve` command first.
- Payslips and salary: use `shaon payroll` commands.
- Authentication or account setup: use `shaon auth` commands.

Prefer structured output when another tool will consume the result. Do not
request credentials or a browser login for a read until the command reports
that authentication is required.

## Human-attested writes

Attendance submissions are claims about real work performed. The user must
attest the facts; neither inferred calendar events nor an agent's judgment is
enough.

- Reporting commands preview by default. Show the employee, date or range,
  times, report type, and resulting action.
- Run with `--execute` (or MCP `execute: true`) only after the user explicitly
  authorizes that concrete submission.
- Never autonomously rerun a write with `--execute`.
- After an ambiguous timeout or transport error, read back attendance state
  before proposing another submission.
- CAPTCHA and attended login steps must be completed by the user.

Example safe flow:

```bash
shaon attendance overview --month 2026-09
shaon attendance report day 2026-09-03 --type regular --hours 09:00-17:30
# Only after the user confirms that preview:
shaon attendance report day 2026-09-03 --type regular --hours 09:00-17:30 --execute
```

Use `shaon attendance report range --help` before a range correction; resolve
weekends, holidays, leave, and partial days instead of assuming identical work
hours for every date.

Range reports and auto-fill skip Friday and Saturday unless explicitly
overridden. Auto-fill is capped at 10 days by default; use its help before
changing that limit.

## Payroll and sensitive output

List available payslips before downloading one. Keep salary documents and
values within the user's requested destination and scope.

`shaon payroll payslip password --force-sensitive-output` prints the current
Hilan password in plaintext. It may be captured by terminals, logs, remote
sessions, screenshots, and agent transcripts. Run it only when the user
explicitly requests password recovery in a private interactive terminal. It
does not recover historical PDF passwords.

## Troubleshooting

- Use the command's structured error before changing configuration.
- If authentication expired, use the supported login flow; do not scrape or
  expose session material.
- State-changing requests must not be retried automatically.
- Do not disable TLS validation or credential protections to bypass an error.

For repository architecture and development checks, read `CLAUDE.md`.
