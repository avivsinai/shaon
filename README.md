# hilan

[![CI](https://github.com/avivsinai/hilan/actions/workflows/ci.yml/badge.svg)](https://github.com/avivsinai/hilan/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Rust: 1.80+](https://img.shields.io/badge/rust-1.80%2B-orange.svg)](https://www.rust-lang.org/)

Unofficial Rust CLI for Hilan / Hilanet automation.

`hilan` logs into Hilanet, replays the legacy ASP.NET WebForms flows that still
power attendance reporting, and talks directly to the newer ASMX JSON endpoints
for data like absences and bootstrap metadata. The current release focuses on
day-to-day employee workflows: attendance status, error inspection, safe write
previews, payslips, and salary summaries.

## Why This Exists

Hilanet exposes useful functionality, but much of it still lives behind brittle
browser flows. This project makes those flows scriptable without pretending the
underlying protocol is simple:

- Full ASP.NET form replay for attendance pages and error-wizard flows
- Direct ASMX JSON calls where Hilan exposes machine-friendly endpoints
- Dry-run-by-default write commands so reporting changes are inspectable before
  they are sent

For the protocol map and reverse-engineering notes, see [PROTOCOL.md](PROTOCOL.md).

## What Works Today

Version `0.3.0` ships these commands:

- Auth and setup: `auth`, `sync-types`, `types`
- Attendance read flows: `status`, `errors`, `report`, `sheet`, `corrections`, `absences`
- Payroll and personal-file flows: `payslip`, `salary`
- Attendance write flows: `clock-in`, `clock-out`, `fill`, `fix`

## Safety Model

All write commands are safe by default.

- `clock-in`
- `clock-out`
- `fill`
- `fix`

These commands print the reconstructed request payload and do not submit
anything until `--execute` is passed.

## Installation

### Build From Source

```bash
git clone https://github.com/avivsinai/hilan.git
cd hilan
cargo build --release
```

### Install With Cargo

```bash
cargo install --path .
```

### Use The Local Wrapper

For local development, the repo includes a small wrapper that builds and caches
the release binary under `~/.cache/hilan/<version>/hilan`:

```bash
./scripts/run.sh --help
```

If you want a stable local command name during development:

```bash
mkdir -p ~/bin
ln -sf "$PWD/scripts/run.sh" ~/bin/hilan
```

## Configuration

`hilan` reads a TOML config file from the platform-specific config directory.

| Platform | Config path |
| --- | --- |
| macOS | `~/Library/Application Support/com.hilan.hilan/config.toml` |
| Linux and fallback | `~/.config/hilan/config.toml` |

Example:

```toml
subdomain = "YOUR_COMPANY"
username = "YOUR_ID_NUMBER"
password = "YOUR_PASSWORD"

# optional
payslip_folder = "/Users/you/Downloads/payslips"
payslip_format = "%Y-%m.pdf"
```

Notes:

- `subdomain` is the part before `.hilan.co.il`
- the password is currently read from the config file directly, so keep the
  file user-readable only
- CAPTCHA is not bypassed; if Hilan asks for one, solve it in the browser first

## Quick Start

```bash
# Verify credentials
./scripts/run.sh auth

# Cache attendance types for symbolic --type values
./scripts/run.sh sync-types

# Show the current month's attendance status
./scripts/run.sh status

# Preview a clock-in payload without submitting it
./scripts/run.sh clock-in

# Actually submit a clock-in
./scripts/run.sh clock-in --execute

# Download the previous month's payslip
./scripts/run.sh payslip

# Show recent salary totals
./scripts/run.sh salary --months 3
```

## Command Reference

### Setup And Discovery

```bash
hilan auth
hilan sync-types
hilan types
```

### Attendance Reads

```bash
hilan status [--month YYYY-MM]
hilan errors [--month YYYY-MM]
hilan report <REPORT_NAME>
hilan sheet
hilan corrections
hilan absences
```

### Payroll

```bash
hilan payslip [--month YYYY-MM] [--output PATH]
hilan salary [--months N]
```

### Attendance Writes

```bash
hilan clock-in [--type TYPE] [--dry-run | --execute]
hilan clock-out [--dry-run | --execute]
hilan fill --from YYYY-MM-DD --to YYYY-MM-DD [--type TYPE] [--hours HH:MM-HH:MM] [--dry-run | --execute]
hilan fix YYYY-MM-DD [--type TYPE] [--hours HH:MM-HH:MM] [--report-id UUID] [--error-type N] [--dry-run | --execute]
```

## Operational Notes

- `sync-types` caches attendance types under the per-subdomain config directory
- `status` and `errors` navigate months by replaying the full ASP.NET form
- `sheet` reads `HoursAnalysis.aspx`
- `corrections` reads `HoursReportLog.aspx`
- `absences` currently exposes initial symbol data only
- `payslip` validates PDF magic bytes before writing a file
- `salary` posts a date-range state payload to `SalaryAllSummary.aspx`

## Development

The repo is intentionally small:

- [`src/client.rs`](src/client.rs) contains the HTTP/session layer
- [`src/attendance.rs`](src/attendance.rs) contains calendar parsing and write replay
- [`src/main.rs`](src/main.rs) contains the CLI surface
- [`src/ontology.rs`](src/ontology.rs) contains attendance-type caching
- [`src/reports.rs`](src/reports.rs) contains report table parsing

Local checks:

```bash
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
```

Fixtures for parser and protocol tests live under [`tests/fixtures`](tests/fixtures).

## Prior Art

- [zigius/hilan-bot](https://github.com/zigius/hilan-bot)
- [talsalmona/hilan](https://github.com/talsalmona/hilan)

## License

MIT. See [LICENSE](LICENSE).
