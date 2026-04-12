# Architecture

This document is the high-level map of the shaon codebase. It sits between the user-facing [README.md](README.md) and the wire-level [PROTOCOL.md](PROTOCOL.md).

## Documentation Map

- [README.md](README.md): how to install, configure, and use the CLI, MCP server, and Claude Code plugin
- [ARCHITECTURE.md](ARCHITECTURE.md): crate boundaries, runtime flows, and maintenance rules
- [PROTOCOL.md](PROTOCOL.md): Hilanet endpoint behavior, form replay details, and reverse-engineering notes
- [skills/shaon/SKILL.md](skills/shaon/SKILL.md): the Claude Code skill shipped with the plugin
- [CLAUDE.md](CLAUDE.md): repo instructions for coding agents and maintainers
- [CONTRIBUTING.md](CONTRIBUTING.md): contributor workflow, checks, and safety policy

## System Overview

shaon has three runtime surfaces built on one shared provider stack:

| Surface | Entry point | Audience |
|---------|-------------|----------|
| CLI | `crates/shaon-cli` | humans and shell scripts |
| MCP server | `crates/shaon-mcp` | agent platforms and tool clients |
| Claude Code plugin / skill | `.claude-plugin/plugin.json`, `skills/shaon/SKILL.md` | Claude Code users |

All three surfaces eventually go through `provider-hilan`, which implements the domain traits defined in `hr-core`.

## Workspace Layout

| Path | Role |
|------|------|
| `crates/hr-core` | provider-agnostic DTOs, traits, error types, and use-cases |
| `crates/provider-hilan` | Hilan-specific transport, parsing, config, session reuse, retry, and protocol replay |
| `crates/shaon-cli` | clap command definitions, rendering, JSON mode, auth UX |
| `crates/shaon-mcp` | stdio MCP server and tool schemas |
| `src/` | compatibility facade and binary entrypoint |
| `skills/shaon/SKILL.md` | Claude Code skill text |
| `.claude-plugin/plugin.json` | Claude Code plugin manifest |
| `scripts/run.sh` | local build/cache wrapper |
| `scripts/install.sh` | release installer |
| `scripts/setup-codesign.sh` | local macOS signing identity setup |

## Runtime Flow

### 1. Config and Secrets

- Non-secret config lives in `~/.shaon/config.toml`
- Password and session-key secrets live in the OS keychain by default
- `SHAON_PASSWORD` and `SHAON_SESSION_KEY` bypass keychain access for CI and headless automation

### 2. Transport and Session

`HilanClient` in `provider-hilan` owns:

- the `reqwest` client and cookie jar
- persisted encrypted cookies
- pre-login org-id lookup for the initial login flow
- cached bootstrap identity after authentication
- retry policy for idempotent operations only
- reauthentication on expired sessions

State-changing requests are intentionally not retried automatically.

### 3. Domain Provider

`HilanProvider` adapts Hilan-specific behavior to the generic traits in `hr-core`:

- attendance calendar reads
- attendance writes and error fixes
- absence/type lookup
- salary summary
- payslip download
- named report fetches

### 4. Frontends

- `shaon-cli` turns the provider into explicit shell commands and optional JSON output
- `shaon-mcp` exposes a curated stdio tool surface for agents
- the Claude Code skill gives natural-language entry points and can delegate to the CLI or MCP

## Safety Model

The project has one hard rule: no write happens by accident.

- CLI writes require `--execute`
- MCP writes require `execute: true`
- previews are the default everywhere
- `fill` and `auto-fill` skip Israeli weekends (Fri/Sat) unless explicitly overridden
- `auto-fill` has a `--max-days` / `max_days` safety cap

## Protocol Strategy

shaon talks to Hilanet through two main mechanisms:

1. ASP.NET WebForms pages
   Used for calendar pages, error-fix flows, and classic reports. The code replays fresh hidden fields from the current page state instead of hardcoding stale payloads.

2. ASMX JSON endpoints
   Used for identity/bootstrap data, absences, tasks, and salary-related data.

When a flow touches the wire behavior directly, update [PROTOCOL.md](PROTOCOL.md).

## Write-Path Shape

Most attendance writes follow this pattern:

1. load fresh page state
2. resolve employee/bootstrap context
3. build a minimal set of field overrides on top of the current form state
4. preview the reconstructed payload
5. submit only when explicitly executing

Some error-fix flows require multiple steps, for example clearing a Hilan error and then applying the desired calendar state. Those are implemented as explicit, ordered steps rather than implicit retries.

## What To Update When Behavior Changes

Use this table to keep docs from drifting:

| If you change... | Update... |
|------------------|-----------|
| CLI commands, defaults, or examples | `README.md`, `skills/shaon/SKILL.md`, command help snapshots if referenced |
| MCP tools or request schemas | `README.md`, `CLAUDE.md` |
| crate boundaries or ownership | `ARCHITECTURE.md`, `CLAUDE.md` |
| Hilanet endpoint behavior or replay details | `PROTOCOL.md` |
| contributor workflow or required checks | `CONTRIBUTING.md`, `CLAUDE.md` |
| macOS signing / install behavior | `README.md`, `CONTRIBUTING.md`, `scripts/*.sh` comments as needed |

## Drift-Resistant Documentation Rules

- Avoid hard-coding command or tool counts unless the count itself matters
- Prefer linking to `shaon --help` or code-defined surfaces when possible
- Keep user-facing docs in `README.md`; keep reverse-engineering details in `PROTOCOL.md`
- Do not document a feature as supported in MCP or the skill unless it exists in `crates/shaon-mcp` or the plugin surface today
