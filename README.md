# hilan

Claude Code plugin and Rust CLI for Hilan (חילן) / Hilanet automation.

## Status

Version `0.3.0` implements:

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
- `clock-in`
- `clock-out`
- `fill`
- `fix`

The write commands are intentionally safe by default: `clock-in`, `clock-out`, `fill`, and `fix`
run as dry-runs unless `--execute` is passed.

## Prerequisites

- Rust toolchain with `cargo` and `rustc`
- Network access to `https://*.hilan.co.il`
- A Hilan subdomain, username, and password

## Configuration

Create `~/.config/hilan/config.toml`:

```toml
subdomain = "YOUR_COMPANY"
username = "YOUR_ID_NUMBER"
password = "YOUR_PASSWORD"

# optional
payslip_folder = "/Users/you/Downloads/payslips"
payslip_format = "%Y-%m.pdf"
```

The `subdomain` is the part before `.hilan.co.il`.

## Running The CLI

Use the wrapper script:

```bash
plugins/hilan/scripts/run.sh --help
```

On first run it:

- checks that Rust is installed
- builds the CLI from `plugins/hilan/cli/Cargo.toml`
- caches the release binary under `~/.cache/hilan/<version>/hilan`

Optional convenience install:

```bash
mkdir -p ~/bin
ln -sf "$PWD/plugins/hilan/scripts/run.sh" ~/bin/hilan
```

## Quick Start

```bash
# Verify credentials
plugins/hilan/scripts/run.sh auth

# Cache attendance types for symbolic --type values
plugins/hilan/scripts/run.sh sync-types

# Show this month's attendance status
plugins/hilan/scripts/run.sh status

# Preview a clock-in payload without submitting it
plugins/hilan/scripts/run.sh clock-in

# Actually submit a clock-in
plugins/hilan/scripts/run.sh clock-in --execute

# Download the previous month's payslip
plugins/hilan/scripts/run.sh payslip

# Show the analyzed attendance sheet
plugins/hilan/scripts/run.sh sheet
```

## Command Reference

```bash
hilan auth
hilan sync-types
hilan types

hilan status [--month YYYY-MM]
hilan errors [--month YYYY-MM]
hilan report <REPORT_NAME>
hilan sheet
hilan corrections
hilan absences

hilan payslip [--month YYYY-MM] [--output PATH]
hilan salary [--months N]

hilan clock-in [--type TYPE] [--dry-run | --execute]
hilan clock-out [--dry-run | --execute]
hilan fill --from YYYY-MM-DD --to YYYY-MM-DD [--type TYPE] [--hours HH:MM-HH:MM] [--dry-run | --execute]
hilan fix YYYY-MM-DD [--type TYPE] [--hours HH:MM-HH:MM] [--report-id UUID] [--error-type N] [--dry-run | --execute]
```

## Notes

- `sync-types` reads the attendance calendar and caches the local type ontology under `~/.config/hilan/<subdomain>/types.json`.
- `status` and `errors` load the requested attendance month by replaying the full ASP.NET form when month navigation is needed.
- `clock-in`, `clock-out`, `fill`, and `fix` replay the full attendance form payload and print the exact request preview before any live submission.
- `sheet` reads `HoursAnalysis.aspx` and prints the parsed HTML table.
- `corrections` reads `HoursReportLog.aspx` and prints the parsed HTML table.
- `absences` currently exposes the initial absence symbols data, not full absence submission.
- `payslip` validates PDF magic bytes before writing the file.
- `salary` posts the date range to `SalaryAllSummary.aspx` and falls back to ASP.NET hidden-field replay when required.
- CAPTCHA is not bypassed. If Hilan requests a CAPTCHA, solve it in the browser first and retry.

## Prior Art

- [zigius/hilan-bot](https://github.com/zigius/hilan-bot)
- [talsalmona/hilan](https://github.com/talsalmona/hilan)
- `/Users/aviv.s/workspace/dev-browser` was useful as documentation inspiration and for future endpoint discovery, not as a runtime dependency
