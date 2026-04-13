# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/),
and this project adheres to [Semantic Versioning](https://semver.org/).

## [Unreleased]

## [0.8.3] - 2026-04-13
### Added
- Windows packaging is now part of the release pipeline, enabling Scoop distribution for future releases.
- Release metadata validation now checks skill frontmatter versions in addition to plugin manifests and `Cargo.toml`.

### Changed
- Release automation now follows the same PR-based flow as `yoetz`: merge `chore(release): vX.Y.Z` on `main`, let CI create the tag/release, and then open `gh`-driven PRs to update Homebrew and Scoop.
- `README.md` now documents the marketplace, `skills.sh`, Homebrew, Scoop, checksum verification, and release/download surfaces in the same style as the other public Aviv Sinai CLI repos.
- Plugin metadata and the `shaon` skill frontmatter are now kept in-version with the workspace release.


## [0.8.2] - 2026-04-13
### Added
- New `attendance_fix_partial_commit` provider error code with structured `details.partial_commit = { date, desired_type_code?, completed_steps[], failed_step, remaining_steps[], source }` for the multi-step `ErrorWizardThenCalendar` fix flow. All step arrays use the same shape `{ key, label, committed, outcome }` where outcome ∈ `committed` / `skipped` / `failed_outcome_unknown` / `failed_rejected` / `not_attempted`. CLI renders a human block listing committed/failed/remaining steps; MCP surfaces the JSON via `ToolError.details`.

### Changed
- `shaon auth` now re-prompts and refreshes stored credentials when the existing password fails verification, and `shaon auth --force-prompt` forces a deterministic credential refresh.
- `shaon payroll payslip password` now requires `--force-sensitive-output` and carries stronger current-password-only warning text in CLI/docs/skill routing.
- `shaon_fill` MCP responses now return full per-day preview objects instead of only `{date, executed, employee_id}` stubs.

### Security
- Async ASP.NET write responses now fail closed with an outcome-unknown error when Hilan returns HTML/login content or malformed delta instead of a valid async write response.
- Downloaded payslip PDFs are now chmod `0600` on Unix after write.

## [0.8.1] - 2026-04-12
### Added
- **Responsibility and Scope** section in README with user-responsibility language, single-user scope statement, independence disclaimer, and MIT AS-IS / no-warranty language.
- **Human-attested writes** section in `skills/shaon/SKILL.md` directing agents to run dry-run first, surface the preview, and require explicit user confirmation before any `--execute`.
- **Responsible Contributions** guardrails in `CONTRIBUTING.md` (no aggregation, no third-party scraping, no CAPTCHA/MFA bypass, no circumvention of provider terms).

### Changed
- All MCP write-tool descriptions and `execute` field descriptions now read "Human-attested write. Only set `execute=true` after the user has reviewed the dry-run preview and explicitly confirmed submission." — generic MCP clients that do not load the skill see this guidance via the tool schema directly.
- `shaon_payslip_download` MCP tool: `include_bytes` is now explicit opt-in (previously defaulted to `true` when no `output_path` was supplied). Description warns that MCP responses may be logged.
- `payroll payslip password` is demoted from common command listings and now carries a **Sensitive** warning everywhere it appears — it reveals the current Hilan login password in plaintext. Help text, README, and skill routing all call this out.
- `ARCHITECTURE.md` no longer claims "no write happens by accident"; language is now "shaon is designed to make accidental writes hard, not impossible."
- `PROTOCOL.md` manager / approver / roster protocol notes are explicitly marked as observed-only and out of scope for shaon contributions.
- Top-level descriptions in README, SKILL.md, and `plugin.json` narrowed to "personal attendance, payslip, salary, and related self-service tasks on the user's own Hilan account."
- `SECURITY.md`: vulnerability reporting now goes through GitHub Security Advisories; removed public contact email. Credential-storage note rephrased from an absolute claim to "by default, stored in the OS keychain for local interactive use."

### Security
- Reduced the default agent-visible surface for payslip bytes and the current login password — both are now opt-in with explicit warnings.

## [0.8.0] - 2026-04-12
### Added
- `attendance report day` makes single-day reporting first-class instead of routing through a one-day range fill.
- MCP now exposes `shaon_resolve` for error-day fixes and `shaon_payslip_download` for password-protected payslip retrieval.
- `payslip view` opens a payslip in Preview on macOS via `open -fa Preview`; no file is stored in shaon's cache.
- `payslip password` prints the current Hilan password used for new password-protected payslip PDFs.

### Changed
- The CLI is now a pristine hierarchical tree: `attendance`, `payroll`, `reports`, plus top-level `auth`, `serve`, and `completions`.
- `auth` is now the only local credential setup flow. Re-run `shaon auth` if you need to refresh stored credentials.
- Old flat command names were removed without aliases or migration shims.
- `attendance resolve` now auto-detects the provider fix target from overview/error data instead of exposing `report_id` / `error_type`.
- `sheet`, `corrections`, and `reports show` now use a stable JSON report schema instead of raw provider table payloads.
- `sync-types` was replaced by the hidden admin command `cache refresh attendance-types`.
- Bundled keychain credentials now store the Hilan password and a local master key together in the `shaon-cli` entry.
- `SHAON_MASTER_KEY` now controls headless local cache encryption alongside `SHAON_PASSWORD`.
- Downloaded payslip PDFs are now password-protected with the Hilan password before being written to disk.

### Security
- Session-cookie encryption keys are now derived from the local master key with HKDF-SHA256 before AES-256-GCM encryption at rest.

## [0.7.0] - 2026-04-12
### Added
- **Self-signed codesign identity** (`scripts/setup-codesign.sh`): creates a local codesigning identity so macOS Keychain "Always Allow" persists across rebuilds. Previously ad-hoc `codesign -s -` produced a new cdhash-based designated requirement per build, re-prompting every time. `scripts/run.sh` now prefers the identity; ad-hoc fallback only when it's missing.
- **`ARCHITECTURE.md`**: high-level map of workspace crates, trait boundaries, and the two-layer Hilan protocol.
- **End-to-end orchestration tests**: `spawn_test_server` coverage of `ErrorWizardThenCalendar` (no-conflict + delete-then-resubmit paths).

### Fixed
- **Unified submit/fix flow**: auto-deletes conflicting absence rows before applying a work-type. Vacation → WFH no longer silently rejected with `קיים דיווח בזמן המדווח`; one `shaon fix` or `shaon fill --execute` call handles the two-step orchestration transparently.
- **Payslip download**: `OrgId` now sourced from `api::bootstrap()` instead of a fragile homepage HTML regex. Resolves `payslip_download_failed: Could not find OrgId`.
- **Parser: `selected_row_date`**: raw string terminator bug (`"#` ended the `r#"…"#` literal) caused the fail-closed guard to trip on every valid response.
- **Parser: `parse_aspx_delta`**: UTF-8 safe slicing; previously panicked on Hebrew alert payloads (`קיים דיווח…`).
- **Parser: RowData binding**: returns the block matching the requested `ReportDate` instead of the first one on the page.
- **Multi-row conflict deletion**: deletes all blocking absence rows for the target day, not just the first.
- **Session expiry detection**: narrowed to path-based match so `/HilanCenter/Public/api/LoginApi/…` (the login endpoint itself) isn't flagged as an auth redirect.
- **Hilan async writes**: explicit `alert()` / `HWarning()` / `HError()` detection in delta responses. No more silent `executed: true` when the server actually rejected.

### Changed
- **Bootstrap cached per client/session**: invalidated on reauth. Removes 3× redundant `GetData` calls per write.
- **DRY**: calendar / error-wizard `browser_fields` allowlists hoisted to module-level constants.
- **DRY**: step-preview rendering consolidated into a shared `render_step_list` helper used by both `attendance::compose_submit_preview_steps` and `provider::preview_with_steps`.
- **LazyLock**: 16 × `Selector::parse(...).unwrap()` migrated to module-scope `LazyLock` statics.
- **Release workflow + Homebrew formula**: `caveats` block explains the ad-hoc install path and points users at `setup-codesign.sh` for a stable local signing identity. `scripts/install.sh` prints the same guidance on macOS.
- **Logo**: tightened to industry-standard ~10% safe-zone (character fills ~80% of canvas).

### Security
- **`scripts/setup-codesign.sh`**: does NOT pass `-A` to `security import`. Explicit `-T /usr/bin/codesign` allowlist only; other local processes can't use the signing key without user approval.

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
[0.5.0]: https://github.com/avivsinai/shaon/releases/tag/v0.5.0
[0.6.0]: https://github.com/avivsinai/shaon/releases/tag/v0.6.0
[0.7.0]: https://github.com/avivsinai/shaon/releases/tag/v0.7.0
[0.8.0]: https://github.com/avivsinai/shaon/releases/tag/v0.8.0
[0.8.1]: https://github.com/avivsinai/shaon/releases/tag/v0.8.1
[0.8.2]: https://github.com/avivsinai/shaon/releases/tag/v0.8.2
[Unreleased]: https://github.com/avivsinai/shaon/compare/v0.8.2...HEAD
