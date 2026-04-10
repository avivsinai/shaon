# Generic HR/Attendance Provider Architecture Plan

## Goal

Turn `shaon` from "a Hilan CLI that happens to expose a library" into:

1. a small provider-agnostic core for attendance/HR workflows,
2. a Hilan provider implementation on top of that core,
3. CLI and MCP frontends that depend on traits/use-cases instead of Hilan internals.

The target is not a grand framework. The target is a reusable library that lets someone build:

- a different HR CLI,
- a different MCP server,
- or a second provider implementation,

without depending on Hilan-specific transport, config, HTML parsing, or keychain code.

## Design Principles

1. Abstract at the use-case boundary, not at the HTTP/HTML boundary.
2. Keep provider-owned auth, session, config, and caching private to the provider crate.
3. Prefer a small core plus optional capability traits over one giant trait.
4. Keep Hilan as the reference implementation; do not freeze the codebase into an over-generalized architecture before a second provider exists.
5. Split crates only after module boundaries are proven inside the current repo.

## Audit: What Is Already Generic?

### Already close to generic

- `attendance::CalendarDay`
- `attendance::MonthCalendar`
- `ontology::AttendanceType`
- `client::SalaryEntry`
- `client::SalarySummary`
- `reports::ReportTable`

These are already domain-facing DTOs. They need naming cleanup and decoupling from Hilan modules, but not conceptual redesign.

### Generic with rename or shape cleanup

- `api::BootstrapInfo`
  - Generic idea: authenticated user / employee identity.
  - Better core name: `UserIdentity`.

- `api::ErrorTask`
  - Generic idea: fix target discovered from provider-side task/error surfaces.
  - Better core name: `FixTarget` or `AttendanceIssueRef`.

- `attendance::AttendanceSubmit`
  - Generic idea: requested attendance mutation.
  - Current shape leaks Hilan/WebForms details:
    - `clear_entry`
    - `clear_exit`
    - `clear_comment`
    - `default_work_day`
  - Better core name: `AttendanceChange`.
  - Better core shape: semantic fields plus explicit write mode.

- `attendance::SubmitPreview`
  - Generic idea: dry-run / write preview.
  - Current shape leaks HTML form replay fields.
  - Better core name: `WritePreview`.
  - Hilan-specific payload diagnostics should remain provider-private or live in provider-specific extensions.

- `client::PayslipDownload`
  - Generic enough, but likely wants a name like `DocumentDownload`.

- `api::AbsenceSymbol` / `api::AbsencesInitialData`
  - Generic enough for a provider-specific DTO, but not a good core abstraction yet.
  - In core, this likely becomes part of `AttendanceTypeCatalog` or `LeaveTypeCatalog`, not a first-class cross-provider type in v1.

### Hilan-specific and should stay provider-specific

- `client::HilanClient`
- all ASMX endpoint knowledge in `api.rs`
- HTML/WebForms parsing and replay logic in `attendance.rs`
- `config.rs` (`~/.shaon`, keychain services, cookie encryption)
- `ontology.rs` sync/cache behavior tied to Hilan calendar + absences APIs
- report names and report URL constants in `reports.rs`
- the current MCP tool handler names and wiring in `mcp.rs`

## What The Core Should Contain

The core should be brand-free and transport-free:

- domain DTOs,
- trait contracts,
- capability flags,
- reusable use-cases for CLI/MCP,
- shared error envelopes,
- shared write-mode semantics.

It should not contain:

- `reqwest`,
- `scraper`,
- `keyring`,
- `cookie_store`,
- `.aspx`/ASMX details,
- filesystem layout like `~/.shaon`.

## Proposed Domain Types

Start with a very small set:

```rust
pub struct UserIdentity {
    pub user_id: String,
    pub employee_id: String,
    pub display_name: String,
    pub is_manager: bool,
}

pub struct AttendanceType {
    pub code: String,
    pub name_he: String,
    pub name_en: Option<String>,
}

pub struct CalendarDay {
    pub date: NaiveDate,
    pub day_name: String,
    pub has_error: bool,
    pub error_message: Option<String>,
    pub entry_time: Option<String>,
    pub exit_time: Option<String>,
    pub attendance_type: Option<String>,
    pub total_hours: Option<String>,
}

pub struct MonthCalendar {
    pub month: NaiveDate,
    pub employee_id: String,
    pub days: Vec<CalendarDay>,
}

pub struct FixTarget {
    pub date: NaiveDate,
    pub issue_kind: Option<String>,
    pub provider_ref: String,
    pub metadata: BTreeMap<String, String>,
}

pub struct AttendanceChange {
    pub date: NaiveDate,
    pub attendance_type_code: Option<String>,
    pub entry_time: Option<String>,
    pub exit_time: Option<String>,
    pub comment: Option<String>,
    pub clear_entry: bool,
    pub clear_exit: bool,
    pub clear_comment: bool,
}

pub enum WriteMode {
    DryRun,
    Execute,
}

pub struct WritePreview {
    pub executed: bool,
    pub summary: String,
    pub provider_debug: Option<serde_json::Value>,
}

pub struct SalaryEntry {
    pub month: NaiveDate,
    pub amount: u64,
}

pub struct SalarySummary {
    pub label: String,
    pub entries: Vec<SalaryEntry>,
    pub percent_diff: Option<f64>,
}
```

Notes:

- Keep `chrono::NaiveDate` for now. A custom `YearMonth` newtype can come later if month/day confusion becomes painful.
- `FixTarget.provider_ref` + `metadata` is deliberate. It avoids baking Hilan's `reportId`/`errorType` into the core while still supporting it cleanly.
- `WritePreview.provider_debug` should be optional and intended for CLI/MCP debugging, not stable cross-provider semantics.

## Proposed Trait Surface

### Minimum viable capability split

The correct first abstraction is not one giant `HrProvider` trait. It is one attendance-focused base trait plus optional extension traits.

```rust
#[async_trait]
pub trait AttendanceProvider: Send {
    async fn identity(&mut self) -> Result<UserIdentity, ProviderError>;
    async fn month_calendar(&mut self, month: NaiveDate) -> Result<MonthCalendar, ProviderError>;
    async fn attendance_types(&mut self) -> Result<Vec<AttendanceType>, ProviderError>;
    async fn fix_targets(&mut self, month: NaiveDate) -> Result<Vec<FixTarget>, ProviderError>;

    async fn submit_day(
        &mut self,
        change: &AttendanceChange,
        mode: WriteMode,
    ) -> Result<WritePreview, ProviderError>;

    async fn fix_day(
        &mut self,
        target: &FixTarget,
        change: &AttendanceChange,
        mode: WriteMode,
    ) -> Result<WritePreview, ProviderError>;
}

#[async_trait]
pub trait SalaryProvider: Send {
    async fn salary_summary(&mut self, months: u32) -> Result<SalarySummary, ProviderError>;
}

#[async_trait]
pub trait PayslipProvider: Send {
    async fn download_payslip(
        &mut self,
        month: NaiveDate,
        output: Option<&Path>,
    ) -> Result<DocumentDownload, ProviderError>;
}

#[async_trait]
pub trait ReportProvider: Send {
    async fn report(&mut self, spec: ReportSpec) -> Result<ReportTable, ProviderError>;
}
```

### Why this shape

- `AttendanceProvider` owns the critical shared workflows today: status, errors, fix, fill, auto-fill, overview.
- Salary, payslip, and reports are optional capabilities. Many HR systems will not expose all of them through the same mechanism.
- `fix_targets()` returns semantic fix references instead of Hilan-specific `reportId`/`errorType` fields.
- The trait stays at the business boundary. Hilan can keep internal helper methods like `asmx_call()`, `replay_submit()`, and `fetch_org_id()` without leaking them into the abstraction.

## Provider Error Model

Define a small stable error envelope in core:

```rust
pub struct ProviderError {
    pub code: Cow<'static, str>,
    pub message: String,
    pub retryable: bool,
    pub details: Option<serde_json::Value>,
}
```

This is enough for:

- CLI human output,
- JSON output,
- MCP errors,
- and provider-specific detail passthrough.

Do not try to normalize every provider error into a universal enum in v1.

## Capability Model

Borrow from OpenDAL's explicit capability model rather than implicit feature guessing.

```rust
pub struct ProviderCapabilities {
    pub attendance_read: bool,
    pub attendance_write: bool,
    pub fix_errors: bool,
    pub salary_summary: bool,
    pub payslips: bool,
    pub reports: bool,
    pub attendance_types: bool,
}
```

This lets CLI/MCP degrade gracefully:

- hide unsupported tools,
- return a clean "unsupported capability" error,
- avoid encoding Hilan assumptions into every future provider.

## Recommended Internal Layering

### Near-term layout inside the current crate

Do this before a workspace split:

- `core/`
  - domain types
  - trait definitions
  - capability structs
  - provider error types
  - shared use-cases like overview/autofill planning

- `provider/vendor/`
  - current `HilanClient`
  - current `api.rs`
  - current parsing/replay logic
  - current config/session/cache

- `app/cli/`
  - Clap command wiring
  - human/json rendering

- `app/mcp/`
  - MCP tool schema
  - MCP transport/server wiring

This lets the architecture stabilize without forcing a multi-crate migration immediately.

### Target workspace shape after boundaries harden

Recommended end state:

- `crates/hr-core`
  - provider-agnostic traits, DTOs, capabilities, errors, use-cases

- `crates/provider-hilan`
  - Hilan implementation
  - Hilan auth/session/config/cache
  - Hilan fixtures and parser tests

- `crates/shaon-cli`
  - the shipped `shaon` binary
  - currently compiled with `provider-hilan`

- `crates/shaon-mcp`
  - provider-agnostic MCP server shell
  - initially shipped with Hilan provider wiring

Optional later:

- `crates/provider-foo`
- `crates/hr-testing`

## Minimum Viable Abstraction

This is the smallest architecture that is useful now and does not paint us into a corner:

1. Extract shared DTOs and traits into a core module/crate.
2. Make Hilan implement `AttendanceProvider` plus optional extension traits.
3. Move `overview`, `errors`, `fill`, and `auto-fill` orchestration into provider-agnostic use-cases over those traits.
4. Keep config, keychain, cookies, ontology cache, ASMX, and HTML parsing fully provider-local.
5. Keep binary/tool names Shaon-branded.

This is enough to make the library reusable without solving plugin discovery, dynamic loading, or cross-provider config formats.

## What Not To Generalize Yet

Do not abstract these in the first pass:

- raw HTTP client traits,
- cookie stores,
- keychains,
- filesystem cache layout,
- generic report-name enums,
- a universal config file format,
- runtime-loaded providers,
- a full "HRIS superset" schema for every payroll/leave/benefits concept.

Those are second-order concerns. The first-order concern is: can CLI/MCP logic depend on a stable provider interface instead of Hilan internals?

## Migration Plan

### Phase 1: Prove the boundary in-place

1. Add `core` module with DTOs, traits, `ProviderCapabilities`, and `ProviderError`.
2. Introduce a Hilan provider adapter implementing those traits using current modules.
3. Refactor `overview` and `errors` to consume trait methods instead of `api::*` + `attendance::*` directly.
4. Refactor `fill` / `fix` / `auto-fill` orchestration to the same interface.
5. Keep all current CLI behavior unchanged.

Definition of done:

- CLI/MCP orchestration code no longer imports provider internals directly.
- Hilan-specific logic is isolated behind a provider adapter.

### Phase 2: Split the binaries from the provider

1. Move CLI rendering and Clap command parsing into `app/cli` or `crates/shaon-cli`.
2. Move MCP handlers into `app/mcp` or `crates/shaon-mcp`.
3. Keep provider creation in one explicit composition point.

Definition of done:

- frontends depend on traits/use-cases, not concrete Hilan parsing/client modules.

### Phase 3: Extract workspace crates

1. Move `core` into `crates/hr-core`.
2. Move Hilan implementation into `crates/provider-hilan`.
3. Move CLI/MCP into their own crates.
4. Re-export a convenient Hilan-specific facade for current downstream users.

Definition of done:

- a second provider crate can compile without depending on Hilan transport/config code.

## Reference Architecture Notes

### Terraform provider model

Useful lesson:

- separate provider, resources, and data sources by responsibility,
- create fresh instances instead of sharing mutable state accidentally,
- keep provider-private operational state private.

What to borrow:

- capability-oriented composition,
- clear read-only vs mutating surfaces,
- provider-owned private state.

What not to borrow:

- Terraform's full resource lifecycle/state model. `shaon` is an automation library, not an IaC engine.

### AWS SDK for Rust / Smithy

Useful lesson:

- modular per-service crates,
- common runtime separated from generated/service-specific crates,
- client-per-service boundary instead of one mega-client.

What to borrow:

- small shared runtime/core,
- provider/service-specific implementation crates,
- stable high-level API over provider-specific generated/transport details.

What not to borrow:

- code generation requirement. We do not need Smithy or an IDL to get value from the same modularity lesson.

### Multi-provider Rust tooling

Useful lesson from `mise`:

- a small, well-defined backend contract can support multiple implementations without making the frontend aware of backend internals.

Useful lesson from OpenDAL:

- explicit capability reporting is better than assuming every backend supports every operation.

## Recommendation

Proceed with Phase 1 only for the next implementation cycle:

- define core DTOs and traits in-place,
- implement the Hilan adapter,
- refactor `overview` and `auto-fill` first,
- postpone workspace splitting until the trait surface has survived at least one more feature cycle.

That is the highest-signal path: it delivers a reusable library boundary now, keeps current velocity, and avoids locking the repo into a speculative crate graph too early.

## References

- Terraform Providers: https://developer.hashicorp.com/terraform/plugin/framework/providers
- Terraform Resources: https://developer.hashicorp.com/terraform/plugin/framework/resources
- Terraform Data Sources: https://developer.hashicorp.com/terraform/plugin/framework/data-sources
- Terraform Private State: https://developer.hashicorp.com/terraform/plugin/framework/resources/private-state
- Terraform Provider Code Specification: https://developer.hashicorp.com/terraform/plugin/code-generation/specification
- AWS SDK for Rust client configuration: https://docs.aws.amazon.com/sdk-for-rust/latest/dg/configure.html
- Smithy Rust design overview: https://smithy-lang.github.io/smithy-rs/design/
- Smithy Rust FAQ: https://smithy-lang.github.io/smithy-rs/design/faq.html
- Apache OpenDAL capabilities: https://opendal.apache.org/docs/python/api/capability/
- Apache OpenDAL layers: https://opendal.apache.org/docs/python/api/layers/
- mise backend plugin development: https://mise.jdx.dev/backend-plugin-development.html
