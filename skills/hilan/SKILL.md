---
name: hilan
description: Automate Hilan / Hilanet tasks from the CLI. Use when the user mentions hilan, hilanet, attendance, presence reporting, clock in/out, payslip, salary slip, salary summary, or שעון נוכחות.
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

## Commands

### Setup

| Command | Purpose |
| --- | --- |
| `auth` | Test credentials and store them in the system keychain |
| `auth --migrate` | Migrate plaintext config password into the keychain and remove it from `config.toml` |
| `sync-types` | Sync attendance-type ontology from Hilan calendar page |
| `types` | List cached attendance types (from local cache) |

### Attendance Reads

| Command | Purpose |
| --- | --- |
| `status [--month YYYY-MM]` | Show attendance calendar for a month |
| `errors [--month YYYY-MM]` | Show attendance errors for a month |
| `report <REPORT_NAME>` | Fetch a named generic report and print its HTML table |
| `sheet` | Fetch the analyzed attendance sheet (`HoursAnalysis.aspx`) |
| `corrections` | Fetch the attendance correction log (`HoursReportLog.aspx`) |
| `absences` | Print absence symbols and display names |

### Attendance Writes

| Command | Purpose |
| --- | --- |
| `clock-in [--type TYPE] [--execute]` | Clock in for today |
| `clock-out [--execute]` | Clock out for today |
| `fill --from DATE --to DATE [--type TYPE] [--hours HH:MM-HH:MM] [--include-weekends] [--execute]` | Fill attendance for a date range |
| `fix DATE [--type TYPE] [--hours HH:MM-HH:MM] [--report-id UUID] [--error-type N] [--execute]` | Fix a single day via the error wizard |

### Payroll

| Command | Purpose |
| --- | --- |
| `payslip [--month YYYY-MM] [--output PATH]` | Download payslip PDF |
| `salary [--months N]` | Show salary summary for recent months |

### MCP Server

| Command | Purpose |
| --- | --- |
| `serve` | Start the MCP server (stdio transport) |

`serve` exposes all read commands as MCP tools: `status`, `errors`, `report`, `sheet`, `corrections`, `absences`, `types`, `payslip`, `salary`. Write tools (`clock_in`, `clock_out`, `fill`, `fix`) require the caller to pass `execute: true` explicitly.

## Report Name Constants

These are the valid report names for the `report` command:

| Constant | Value |
| --- | --- |
| Errors report | `ErrorsReportNEW` |
| Missing report | `MissingReportNEW` |
| Status report | `AttendanceStatusReportNew2` |
| Absence report | `AbsenceReportNEW` |
| All report | `AllReportNEW` |
| Manual reporting | `ManualReportingReportNEW` |

Usage:

```bash
plugins/hilan/scripts/run.sh report ErrorsReportNEW
plugins/hilan/scripts/run.sh report MissingReportNEW
plugins/hilan/scripts/run.sh report AllReportNEW
```

## JSON Output

All commands support `--json` for machine-readable output:

```bash
plugins/hilan/scripts/run.sh status --month 2026-04 --json
```

Example JSON for `status`:

```json
{
  "month": "2026-04",
  "employee_id": "12345",
  "days": [
    {
      "date": "2026-04-01",
      "day_name": "Wed",
      "has_error": false,
      "entry_time": "09:02",
      "exit_time": "18:15",
      "attendance_type": "work day",
      "total_hours": "9:13"
    }
  ],
  "summary": {
    "total": 30,
    "reported": 8,
    "errors": 2,
    "missing": 20
  }
}
```

Example JSON for `errors`:

```json
{
  "month": "2026-04",
  "employee_id": "12345",
  "errors": [
    {
      "date": "2026-04-06",
      "day_name": "Mon",
      "message": "missing report"
    }
  ]
}
```

Example JSON for `types`:

```json
{
  "subdomain": "mycompany",
  "types": [
    { "code": "1", "name_he": "יום עבודה", "name_en": "Work Day" },
    { "code": "22", "name_he": "עבודה מהבית", "name_en": "Work From Home" }
  ],
  "fetched_at": "2026-04-10T08:00:00Z"
}
```

Example JSON for `salary`:

```json
{
  "label": "Net Pay",
  "entries": [
    { "month": "2026-02", "amount": 25000 },
    { "month": "2026-03", "amount": 25500 }
  ],
  "percent_diff": 2.0
}
```

When using `--json`, prefer parsing the structured output over scraping the human-readable table format.

## Exit Codes

| Code | Meaning |
| --- | --- |
| `0` | Success |
| `1` | Error (network failure, auth failure, parse failure, invalid input) |

There is no distinction between error types at the exit code level. Check stderr for the error message.

## Write Safety Model

All write commands (`clock-in`, `clock-out`, `fill`, `fix`) default to dry-run mode:

- Without `--execute`: prints the reconstructed request payload and exits. Nothing is submitted.
- With `--execute`: submits the request to Hilan.
- Do not claim a live write happened unless `--execute` was passed and the CLI exited 0.

## Idempotency

Read commands (`status`, `errors`, `report`, `sheet`, `corrections`, `absences`, `types`, `payslip`, `salary`) are always safe to retry.

Write commands are NOT idempotent:

- `clock-in --execute`: may reset the entry time if called again. Check `status` first to see if an entry already exists for today.
- `fill --execute`: re-submits all days in the range, including days already filled. Check `status` for the target month before running.
- `clock-out --execute` and `fix --execute`: same concern -- they overwrite existing values.

Recommended pattern: always run the corresponding read command (`status`, `errors`) before issuing a write, and verify the result after.

## Weekend Skip Behavior

`fill` skips Friday and Saturday by default (Israeli work week). To override:

```bash
plugins/hilan/scripts/run.sh fill --from 2026-04-01 --to 2026-04-30 --type WFH --include-weekends --execute
```

Without `--include-weekends`, days falling on Friday or Saturday are silently skipped.

## Authentication

### Keychain Auth (Recommended)

```bash
plugins/hilan/scripts/run.sh auth
```

On first run, `auth` prompts for credentials and stores them in the system keychain (macOS Keychain, Linux Secret Service). Subsequent commands read credentials from the keychain automatically.

### Migration From Plaintext Config

If you already have a `config.toml` with a `password` field:

```bash
plugins/hilan/scripts/run.sh auth --migrate
```

This moves the password into the keychain and removes the plaintext `password` field from `config.toml`.

### Config File

`config.toml` is located at the platform config directory:

| Platform | Path |
| --- | --- |
| macOS | `~/Library/Application Support/com.hilan.hilan/config.toml` |
| Linux / fallback | `~/.config/hilan/config.toml` |

Minimal config (with keychain auth):

```toml
subdomain = "YOUR_COMPANY"
username = "YOUR_ID_NUMBER"
```

Full config (legacy plaintext):

```toml
subdomain = "YOUR_COMPANY"
username = "YOUR_ID_NUMBER"
password = "YOUR_PASSWORD"

# optional
payslip_folder = "/Users/you/Downloads/payslips"
payslip_format = "%Y-%m.pdf"
```

`subdomain` is the hostname prefix before `.hilan.co.il`.

## Known `fix` Limitations

The `--report-id` and `--error-type` defaults are sampled from one specific error flow (`errorType=63`, the missing-standard-day wizard). Other error types require different `--report-id` and `--error-type` values.

To find the correct values for a different error type:

1. Run `errors --month YYYY-MM` to see which days have errors.
2. Run `report ErrorsReportNEW` to inspect the full errors report -- the report ID and error type may be visible in the table rows or need to be captured from the browser.
3. Pass the correct values: `fix DATE --report-id <UUID> --error-type <N> --execute`.

## Default Workflow

When the user wants Hilan information or automation:

1. If config is missing, instruct them to create `config.toml` and run `auth`.
2. Use `plugins/hilan/scripts/run.sh`.
3. Prefer `sync-types` before symbolic attendance writes such as `--type WFH`.
4. Treat attendance writes as preview-first unless the user explicitly asks to execute.
5. For bulk operations, check `status` first to avoid re-submitting already-filled days.

## Troubleshooting

- If login returns CAPTCHA, solve it in the browser and retry.
- If `payslip` returns a non-PDF response, the session may be invalid or the payslip may not exist for that month.
- If symbolic `--type` resolution fails, run `sync-types` first or pass a numeric type code.
- If a write command is meant to be live, make sure `--execute` is present.
- If `auth --migrate` fails, ensure the system keychain service is accessible (e.g., `security` on macOS, `secret-tool` on Linux).

## Prior Art

- `talsalmona/hilan` for HTTP login, payslip, and salary endpoints
- `zigius/hilan-bot` for attendance UI references
