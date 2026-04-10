---
name: hilan
description: Automate Hilan (חילן) / Hilanet tasks from the CLI. Use when the user mentions hilan, hilanet, attendance, presence reporting, clock in/out, payslip, salary slip, salary summary, or שעון נוכחות.
---

# Hilan

Use the local Hilan CLI wrapper for Hilanet reads and controlled attendance writes.

## CLI Location

Run:

```bash
plugins/hilan/scripts/run.sh [command] [args]
```

The wrapper:

- checks for `cargo` and `rustc`
- builds the Rust CLI from source when needed
- caches the built binary in `~/.cache/hilan/<version>/hilan`

Prefer the wrapper over `cargo run` unless debugging the CLI itself.

## Current Capability

Implemented read flows:

- `auth`
- `sync-types`
- `types`
- `status`
- `errors`
- `report`
- `sheet`
- `corrections`
- `absences`
- `payslip`
- `salary`

Implemented write flows:

- `clock-in`
- `clock-out`
- `fill`
- `fix`

Write safety model:

- writes default to dry-run
- live submission requires explicit `--execute`
- do not claim a live write happened unless `--execute` was passed and the CLI completed successfully

## Prerequisites

The user must have `~/.config/hilan/config.toml`:

```toml
subdomain = "YOUR_COMPANY"
username = "YOUR_ID_NUMBER"
password = "YOUR_PASSWORD"

# optional
payslip_folder = "/Users/you/Downloads/payslips"
payslip_format = "%Y-%m.pdf"
```

`subdomain` is the hostname prefix before `.hilan.co.il`.

## Default Workflow

When the user wants Hilan information or automation:

1. If config is missing, instruct them to create `~/.config/hilan/config.toml`.
2. Use `plugins/hilan/scripts/run.sh`.
3. Prefer `sync-types` before symbolic attendance writes such as `--type WFH`.
4. Treat attendance writes as preview-first unless the user explicitly asks to execute.

## Commands To Use

### Authenticate

```bash
plugins/hilan/scripts/run.sh auth
```

### Sync And Inspect Types

```bash
plugins/hilan/scripts/run.sh sync-types
plugins/hilan/scripts/run.sh types
```

Use `sync-types` before symbolic `--type` values. Numeric type codes can still be passed directly.

### Attendance Reads

```bash
plugins/hilan/scripts/run.sh status --month 2026-04
plugins/hilan/scripts/run.sh errors --month 2026-04
plugins/hilan/scripts/run.sh report ErrorsReportNEW
plugins/hilan/scripts/run.sh sheet
plugins/hilan/scripts/run.sh corrections
plugins/hilan/scripts/run.sh absences
```

Behavior:

- `status` and `errors` load the requested month from the attendance calendar
- `report` fetches a named generic report page and prints the parsed HTML table
- `sheet` fetches `HoursAnalysis.aspx`
- `corrections` fetches `HoursReportLog.aspx`
- `absences` prints the initial absence symbols data currently exposed by the API layer

### Attendance Writes

```bash
plugins/hilan/scripts/run.sh clock-in
plugins/hilan/scripts/run.sh clock-in --execute
plugins/hilan/scripts/run.sh clock-out --execute
plugins/hilan/scripts/run.sh fill --from 2026-04-01 --to 2026-04-03 --type WFH
plugins/hilan/scripts/run.sh fill --from 2026-04-01 --to 2026-04-03 --hours 09:00-18:00 --execute
plugins/hilan/scripts/run.sh fix 2026-04-08 --type "עבודה מהבית" --report-id 00000000-0000-0000-0000-000000000000 --error-type 63
```

Behavior:

- writes replay the full ASP.NET attendance form
- output includes the target URL, employee ID, button, and payload preview
- without `--execute`, the request is only previewed

### Payslip And Salary

```bash
plugins/hilan/scripts/run.sh payslip
plugins/hilan/scripts/run.sh payslip --month 2026-03 --output ~/Downloads/march-2026.pdf
plugins/hilan/scripts/run.sh salary --months 3
```

## Troubleshooting

- If login returns CAPTCHA, solve it in the browser and retry.
- If `payslip` returns a non-PDF response, the session may be invalid or the payslip may not exist for that month.
- If symbolic `--type` resolution fails, run `sync-types` first or pass a numeric type code.
- If a write command is meant to be live, make sure `--execute` is present.
- `dev-browser` can help with future endpoint discovery, but it is not required for current runtime behavior.

## Prior Art

- `talsalmona/hilan` for HTTP login, payslip, and salary endpoints
- `zigius/hilan-bot` for attendance UI references
