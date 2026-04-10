# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/),
and this project adheres to [Semantic Versioning](https://semver.org/).

## [Unreleased]

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

[Unreleased]: https://github.com/avivsinai/hilan/compare/v0.3.0...HEAD
[0.3.0]: https://github.com/avivsinai/hilan/releases/tag/v0.3.0
