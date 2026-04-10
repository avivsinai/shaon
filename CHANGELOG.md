# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/),
and this project adheres to [Semantic Versioning](https://semver.org/).

## [Unreleased]

## [0.5.0] - 2026-04-10
### Added

- **Workspace architecture**: split into `hr-core` (generic traits/DTOs), `provider-hilan`, `hilan-cli`, `hilan-mcp`
- **`AttendanceProvider` trait**: generic interface for HR providers with optional `SalaryProvider`, `PayslipProvider`, `ReportProvider`, `AbsenceProvider`
- **`overview` command**: full agent context in one call (identity, summary, types, errors, suggestions)
- **`auto-fill` command**: batch fill missing days with safety cap (`--max-days`)
- **MCP server** (`hilan serve`): 12 tools via rmcp 1.3 stdio transport
- **Encrypted session cookies**: AES-256-GCM with random DEK in OS keychain
- **Session reuse**: persistent cookies across CLI invocations (no re-login)
- **Salary via ASMX API**: direct JSON parsing instead of HTML scraping
- **Fix param discovery**: auto-extract `reportId`/`errorType` from tasks API
- **Lazy ontology sync**: types auto-refresh with 24h TTL
- **Shell completions**: `hilan completions bash|zsh|fish`
- **Version stamping**: dev builds show git hash (`0.5.0+abc1234`)
- **Tracing**: `--verbose`/`--quiet` flags with structured logging to stderr

### Changed

- Config path: `~/.hilan/` (simple dotfile, no platform-specific paths)
- Calendar parser: reads `ov` attribute for attendance type (not dropdown text)
- ASMX API: parse raw JSON directly (no `{"d": ...}` wrapper assumption)
- Salary: uses `PaymentsAndDeductionsApiapi/GetInitialData` JSON API
- Codesign: stable identifier `com.avivsinai.hilan` for keychain persistence
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

[0.3.0]: https://github.com/avivsinai/hilan/releases/tag/v0.3.0
[0.4.0]: https://github.com/avivsinai/hilan/releases/tag/v0.4.0
[Unreleased]: https://github.com/avivsinai/hilan/compare/v0.4.0...HEAD
