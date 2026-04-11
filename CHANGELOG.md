# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/),
and this project adheres to [Semantic Versioning](https://semver.org/).

## [Unreleased]

## [0.6.0] - 2026-04-11
### Added
- **`AttendanceSource` enum**: distinguishes user-reported, system-auto-filled, holiday, and unreported days
- **Direct month navigation**: jump to any month via `calendar_monthChanged` postback instead of clicking prev/next repeatedly
- **ASP.NET async postback support**: `post_aspx_async()` with delta response parser for UpdatePanel content
- **401/403 re-authentication**: automatic re-login on session expiry (not just login redirects)
- **Request rate-pacing**: 200ms minimum delay between HTTP requests to prevent WAF blocks
- **`SHAON_SESSION_KEY` env var**: bypass OS keychain for session cookie decryption in headless/CI environments
- **Clanker logo**: robot punching a time clock

### Fixed
- **Calendar parsing**: parse the visual calendar grid (`td[Days]` cells) instead of the never-rendered attendance data grid — was returning only 1 day per month
- **Attendance type extraction**: read `title` attribute for type names (e.g., "work from home") and `fh-x` icon for system auto-fill
- **Month navigation**: use `calendar_monthChanged` event for direct jumps; fall back to step-by-step prev/next with iteration cap (24 steps max)
- **Build cache**: `run.sh` now checks `crates/` directory for changes, not just `src/`

### Changed
- `CalendarDay.is_reported()` now checks `AttendanceSource` instead of presence of entry_time/attendance_type
- `CalendarDay` includes `source` field in JSON output

## [0.5.0] - 2026-04-10
### Added

- **Workspace architecture**: split into `hr-core` (generic traits/DTOs), `provider-hilan`, `shaon-cli`, `shaon-mcp`
- **`AttendanceProvider` trait**: generic interface for HR providers with optional `SalaryProvider`, `PayslipProvider`, `ReportProvider`, `AbsenceProvider`
- **`overview` command**: full agent context in one call (identity, summary, types, errors, suggestions)
- **`auto-fill` command**: batch fill missing days with safety cap (`--max-days`)
- **MCP server** (`shaon serve`): 12 tools via rmcp 1.3 stdio transport
- **Encrypted session cookies**: AES-256-GCM with random DEK in OS keychain
- **Session reuse**: persistent cookies across CLI invocations (no re-login)
- **Salary via ASMX API**: direct JSON parsing instead of HTML scraping
- **Fix param discovery**: auto-extract `reportId`/`errorType` from tasks API
- **Lazy ontology sync**: types auto-refresh with 24h TTL
- **Shell completions**: `shaon completions bash|zsh|fish`
- **Version stamping**: dev builds show git hash (`0.5.0+abc1234`)
- **Tracing**: `--verbose`/`--quiet` flags with structured logging to stderr

### Changed

- Config path: `~/.shaon/` (simple dotfile, no platform-specific paths)
- Calendar parser: reads `ov` attribute for attendance type (not dropdown text)
- ASMX API: parse raw JSON directly (no `{"d": ...}` wrapper assumption)
- Salary: uses `PaymentsAndDeductionsApiapi/GetInitialData` JSON API
- Codesign: stable identifier `com.avivsinai.shaon` for keychain persistence
- Release script: PR-based workflow (creates branch + PR, not direct push)

### Fixed

- Calendar parser fabricating days from time strings like "09:00"
- `fill` re-authenticating per day (session reuse)
- `clock-in` silently clearing existing exit time/comment
- Salary percent_diff losing sign on decreases
- OrgId regex failing on escaped JSON in homepage
- Keychain not persisting (missing `apple-native` feature)
- Rate limiting from excessive logins

### Security

- Credentials in OS keychain (macOS Keychain, Linux Secret Service)
- Session cookies encrypted at rest (AES-256-GCM)
- Native-TLS eliminated (rustls-only)
- Subdomain validation prevents URL manipulation
- Username masked in logs (PII protection)
- Gitleaks + cargo-deny in CI


## [0.4.0] - 2026-04-10

### Added

- Codex plugin manifest for the shared agent distribution
- Shared skill symlinks under `.claude/skills` and `.agents/skills`
- Release automation hooks for Homebrew tap updates


## [0.3.0] - 2026-04-10

Initial public release.

### Added

- Auth and setup: `auth`, `sync-types`, `types`
- Attendance reads: `status`, `errors`, `report`, `sheet`, `corrections`, `absences`
- Attendance writes: `clock-in`, `clock-out`, `fill`, `fix`
- Payroll: `payslip`, `salary`
- Safe-by-default write model (`--execute` required for live submission)
- ASP.NET WebForms form replay for attendance pages
- Direct ASMX JSON endpoint support
- TOML config with per-platform paths
- Claude Code skill and plugin manifest
- CI workflow (Ubuntu + macOS, clippy, fmt, test)
- MIT license

[0.3.0]: https://github.com/avivsinai/shaon/releases/tag/v0.3.0
[0.4.0]: https://github.com/avivsinai/shaon/releases/tag/v0.4.0
[Unreleased]: https://github.com/avivsinai/shaon/compare/v0.4.0...HEAD
