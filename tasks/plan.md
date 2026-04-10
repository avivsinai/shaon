# API Generalization Plan

## Vision

Make hilan a reusable library/framework for HR & attendance automation. Anyone should be able to implement a provider for their HR system (Hilan, Workday, BambooHR, etc.) and get CLI, MCP, and library access for free.

## Architecture: Provider Trait + Workspace Split

### Core Trait

```rust
#[async_trait]
pub trait AttendanceProvider: Send + Sync {
    // Lifecycle
    async fn authenticate(&mut self) -> Result<()>;

    // Core reads
    async fn calendar(&mut self, month: NaiveDate) -> Result<MonthCalendar>;
    async fn errors(&mut self, month: NaiveDate) -> Result<Vec<ErrorDay>>;
    async fn attendance_types(&mut self) -> Result<Vec<AttendanceType>>;
    async fn user_info(&mut self) -> Result<UserInfo>;

    // Core writes
    async fn submit(&mut self, submit: &AttendanceSubmit, execute: bool) -> Result<SubmitResult>;

    // Optional capabilities (default to unsupported)
    async fn salary(&mut self, _months: u32) -> Result<Option<SalarySummary>> { Ok(None) }
    async fn payslip(&mut self, _month: NaiveDate) -> Result<Option<Vec<u8>>> { Ok(None) }
    async fn reports(&mut self, _name: &str) -> Result<Option<ReportTable>> { Ok(None) }
}
```

### Design Decisions

| Decision | Choice | Why |
|----------|--------|-----|
| Dispatch | `Box<dyn AttendanceProvider>` | Runtime provider selection from config |
| Self | `&mut self` | Auth state, sessions, cookies |
| Optional features | Return `Option<T>` | Not all providers support salary/payslip |
| Config | Separate from trait | Provider-specific auth varies |
| Error handling | `anyhow::Result<T>` | Consistent across providers |

### Workspace Structure

```
hilan/
├── Cargo.toml                   # [workspace] root
├── crates/
│   ├── hilan-core/              # Generic traits, types, config
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── provider.rs      # AttendanceProvider trait
│   │       ├── types.rs         # CalendarDay, AttendanceType, SalarySummary, etc.
│   │       ├── config.rs        # Multi-provider config model
│   │       └── mock.rs          # MockProvider for testing
│   │
│   ├── hilan-provider/          # Hilan.co.il implementation
│   │   └── src/
│   │       ├── lib.rs           # HilanProvider impl AttendanceProvider
│   │       ├── client.rs        # HTTP client, session, cookies
│   │       ├── forms.rs         # ASP.NET WebForms replay
│   │       ├── api.rs           # ASMX JSON endpoints
│   │       ├── calendar.rs      # Calendar HTML parsing
│   │       ├── reports.rs       # HTML table parsing
│   │       └── ontology.rs      # Type cache
│   │
│   ├── hilan-cli/               # CLI binary
│   │   └── src/main.rs          # clap + provider dispatch
│   │
│   └── hilan-mcp/               # MCP server
│       └── src/lib.rs           # MCP tools via provider trait
│
├── skills/                      # Claude Code skill
├── scripts/                     # Release, install
└── examples/
    └── custom_provider.rs       # How to add a provider
```

### Types That Are Already Generic

These exist today and map 1:1 to the generic layer:

| Type | File | Generic? |
|------|------|----------|
| `CalendarDay` | attendance.rs | Yes — date, times, type, status |
| `MonthCalendar` | attendance.rs | Yes |
| `AttendanceType` | ontology.rs | Yes — code, names |
| `AttendanceSubmit` | attendance.rs | Yes — date, type, times |
| `SubmitPreview` | attendance.rs | Mostly — url/button are Hilan-specific |
| `SalaryEntry` | client.rs | Yes — month, amount |
| `SalarySummary` | client.rs | Yes |
| `ReportTable` | reports.rs | Yes — headers, rows |
| `UserInfo` | New | Will be generic |

### What's Hilan-Specific (stays in hilan-provider)

- ASP.NET form replay (get_aspx_form, post_aspx_form)
- ASMX JSON API (asmx_call)
- OrgId extraction from homepage HTML
- Form field naming patterns (ctl00$mp$RG_Days_...)
- Calendar HTML scraping with `scraper`
- Cookie-based session with encrypted persistence
- Keychain integration for Hilan password

### Config Model

```toml
# ~/.hilan/config.toml
default_provider = "hilan"

[providers.hilan]
type = "hilan"
subdomain = "mycompany"
username = "27"

[providers.workday]
type = "workday"
base_url = "https://mycompany.workday.com"
username = "emp123"
```

CLI: `hilan --provider workday status --month 2026-04`

## Implementation Tasks

### Phase 1: Extract core types (no behavior change)
1. Create workspace root Cargo.toml
2. Move generic types to `crates/hilan-core/src/types.rs`
3. Define `AttendanceProvider` trait in `crates/hilan-core/src/provider.rs`
4. Create `MockProvider` for testing
5. All existing tests must pass

### Phase 2: Extract Hilan provider
6. Move Hilan HTTP logic to `crates/hilan-provider/`
7. Implement `AttendanceProvider` for `HilanProvider`
8. Wire CLI to use the trait (transparent — same behavior)
9. Wire MCP to use the trait

### Phase 3: CLI + config generalization
10. Add `--provider` flag to CLI
11. Multi-provider config model
12. Provider factory with runtime selection
13. Example custom provider

### Phase 4: Polish
14. Documentation for library consumers
15. `cargo publish` readiness for core + provider crates
16. Example: minimal provider implementation

## Status

**Pending review** — waiting for Codex's independent architecture input before starting implementation.
