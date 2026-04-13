use anyhow::{bail, Context, Result};
use chrono::{Datelike, Local, NaiveDate};
use clap::{Args, CommandFactory, Parser, Subcommand};
use hr_core::{
    AttendanceProvider, AttendanceType as CoreAttendanceType, DocumentDownload,
    FixTarget as CoreFixTarget, PayslipProvider, ProviderError, ReportProvider, ReportSpec,
    ReportTable, SalaryProvider, WriteMode as CoreWriteMode, WritePreview as CoreWritePreview,
};
use secrecy::ExposeSecret;
use serde::Serialize;
use std::collections::BTreeSet;
use std::path::PathBuf;
#[cfg(target_os = "macos")]
use std::{
    io::Write,
    process::{Command, Stdio},
};

use hr_core::use_cases;
use provider_hilan::{attendance, client, ontology, Config, HilanProvider};

use attendance::is_time_pattern;
use client::HilanClient;

// ---------------------------------------------------------------------------
// Overview command response types
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct OverviewResponse {
    user: UserInfo,
    month: String,
    summary: MonthSummary,
    attendance_types: Vec<CoreAttendanceType>,
    error_days: Vec<ErrorDay>,
    missing_days: Vec<MissingDay>,
    suggested_actions: Vec<SuggestedAction>,
}

#[derive(Serialize)]
struct StatusResponse {
    month: String,
    employee_id: String,
    days: Vec<StatusDay>,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
struct StatusDay {
    date: String,
    day_name: String,
    has_error: bool,
    error_message: Option<String>,
    entry_time: Option<String>,
    exit_time: Option<String>,
    attendance_type: Option<String>,
    total_hours: Option<String>,
    source: hr_core::AttendanceSource,
}

#[derive(Serialize)]
struct UserInfo {
    user_id: String,
    employee_id: String,
    name: String,
    is_manager: bool,
}

#[derive(Serialize)]
struct MonthSummary {
    total_work_days: u32,
    reported: u32,
    missing: u32,
    errors: u32,
}

#[derive(Serialize)]
struct ErrorDay {
    date: String,
    day_name: String,
    error_message: String,
    fix_params: Option<ErrorFixParams>,
    fix_params_candidates: Vec<ErrorFixParams>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
struct ErrorFixParams {
    report_id: String,
    error_type: String,
}

#[derive(Serialize)]
struct ErrorsResponse {
    month: String,
    employee_id: String,
    error_count: usize,
    errors: Vec<ErrorDay>,
}

#[derive(Serialize)]
struct TypesResponse {
    subdomain: String,
    types: Vec<CoreAttendanceType>,
}

#[derive(Serialize)]
struct AbsencesResponse {
    symbol_count: usize,
    symbols: Vec<hr_core::AbsenceSymbol>,
}

#[derive(Serialize)]
struct SalaryResponse {
    label: String,
    entries: Vec<SalaryEntryResponse>,
    percent_diff: Option<f64>,
}

#[derive(Serialize)]
struct SalaryEntryResponse {
    month: String,
    amount: u64,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
struct MissingDay {
    date: String,
    day_name: String,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
struct FixableDay {
    date: String,
    report_id: String,
    error_type: String,
}

/// Structured suggested actions as a tagged union, not an untyped `params` bag.
#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum SuggestedAction {
    FixErrors {
        reason: String,
        safety: String,
        month: String,
        count: u32,
        fixable_days: Vec<FixableDay>,
    },
    FillMissing {
        reason: String,
        safety: String,
        from: String,
        to: String,
        count: u32,
    },
}

const SHEET_REPORT_PATH: &str = "/Hilannetv2/Attendance/HoursAnalysis.aspx";
const CORRECTIONS_REPORT_PATH: &str = "/Hilannetv2/Attendance/HoursReportLog.aspx";

#[derive(Debug, Parser)]
#[command(
    name = "shaon",
    version,
        long_version = option_env!("SHAON_LONG_VERSION").unwrap_or(env!("CARGO_PKG_VERSION")),
    about = "Shaon attendance & payslip CLI"
)]
struct Cli {
    /// Enable verbose debug output
    #[arg(global = true, long, short = 'v')]
    verbose: bool,

    /// Suppress all status messages
    #[arg(global = true, long, short = 'q', conflicts_with = "verbose")]
    quiet: bool,

    /// Output JSON instead of human-readable text
    #[arg(global = true, long)]
    json: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Args, Debug, Clone)]
struct WriteMode {
    /// Preview the payload without sending it (default behavior)
    #[arg(long)]
    dry_run: bool,

    /// Actually submit the write request to Hilan
    #[arg(long, conflicts_with = "dry_run")]
    execute: bool,
}

impl WriteMode {
    fn should_execute(&self) -> bool {
        self.execute
    }

    fn provider_mode(&self) -> CoreWriteMode {
        if self.execute {
            CoreWriteMode::Execute
        } else {
            CoreWriteMode::DryRun
        }
    }
}

#[derive(Serialize)]
struct WriteOutput<'a> {
    action: &'a str,
    mode: &'a str,
    #[serde(flatten)]
    preview: &'a attendance::SubmitPreview,
}

const PAYSLIP_PASSWORD_WARNING: &str = "Sensitive: prints the current Hilan account password in plaintext to standard output. It does not recover historical passwords used for PDFs encrypted before a password change. Output may be captured by shells, terminals, logs, remote sessions, screenshots, and agent transcripts. Run only on a private interactive terminal you control.";

impl<'a> WriteOutput<'a> {
    fn new(action: &'a str, preview: &'a attendance::SubmitPreview) -> Self {
        Self {
            action,
            mode: if preview.executed {
                "executed"
            } else {
                "dry_run"
            },
            preview,
        }
    }
}

#[derive(Serialize)]
struct ProviderWriteOutput<'a> {
    action: &'a str,
    mode: &'a str,
    executed: bool,
    summary: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    url: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    button_name: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    button_value: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    employee_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    payload_display: Option<&'a str>,
}

impl<'a> ProviderWriteOutput<'a> {
    fn new(action: &'a str, preview: &'a CoreWritePreview) -> Self {
        Self {
            action,
            mode: if preview.executed {
                "executed"
            } else {
                "dry_run"
            },
            executed: preview.executed,
            summary: &preview.summary,
            url: preview.debug_field("url"),
            button_name: preview.debug_field("button_name"),
            button_value: preview.debug_field("button_value"),
            employee_id: preview.debug_field("employee_id"),
            payload_display: preview.debug_field("payload_display"),
        }
    }
}

#[derive(Serialize)]
struct ReportResponse {
    report: ReportMetadata,
    column_count: usize,
    row_count: usize,
    columns: Vec<ReportColumn>,
    rows: Vec<ReportRow>,
}

#[derive(Serialize)]
struct ReportMetadata {
    kind: String,
    requested: String,
    provider_name: String,
}

#[derive(Serialize)]
struct ReportColumn {
    index: usize,
    name: String,
}

#[derive(Serialize)]
struct ReportRow {
    index: usize,
    cells: Vec<String>,
}

#[derive(Args, Debug, Clone)]
struct AuthArgs {
    /// Prompt for a new password even if stored credentials already exist
    #[arg(long)]
    force_prompt: bool,
}

#[derive(Args, Debug, Clone)]
struct OverviewArgs {
    /// Month in YYYY-MM format (defaults to current month)
    #[arg(long)]
    month: Option<String>,

    /// Include full per-day calendar data in output. In JSON mode this adds a
    /// top-level `days` array with the same shape as `attendance status --json`.
    #[arg(long)]
    detailed: bool,
}

#[derive(Args, Debug, Clone)]
struct AttendanceMonthArgs {
    /// Month in YYYY-MM format (defaults to current month)
    #[arg(long)]
    month: Option<String>,
}

#[derive(Args, Debug, Clone)]
struct AttendanceReportTodayArgs {
    /// Report an entry timestamp for now
    #[arg(long = "in", conflicts_with = "out")]
    r#in: bool,

    /// Report an exit timestamp for now
    #[arg(long, conflicts_with = "in")]
    out: bool,

    /// Attendance type override for `--in`
    #[arg(long = "type")]
    attendance_type: Option<String>,

    #[command(flatten)]
    write_mode: WriteMode,
}

#[derive(Args, Debug, Clone)]
struct AttendanceReportDayArgs {
    /// Day to report (YYYY-MM-DD)
    day: String,

    /// Attendance type override
    #[arg(long = "type")]
    attendance_type: Option<String>,

    /// Fixed hours (e.g. "09:00-18:00")
    #[arg(long)]
    hours: Option<String>,

    #[command(flatten)]
    write_mode: WriteMode,
}

#[derive(Args, Debug, Clone)]
struct AttendanceReportRangeArgs {
    /// Start date (YYYY-MM-DD)
    #[arg(long)]
    from: String,

    /// End date (YYYY-MM-DD)
    #[arg(long)]
    to: String,

    /// Attendance type override
    #[arg(long = "type")]
    attendance_type: Option<String>,

    /// Fixed hours (e.g. "09:00-18:00")
    #[arg(long)]
    hours: Option<String>,

    /// Include Friday and Saturday (Israeli weekend) instead of skipping them
    #[arg(long)]
    include_weekends: bool,

    #[command(flatten)]
    write_mode: WriteMode,
}

#[derive(Args, Debug, Clone)]
struct AttendanceResolveArgs {
    /// Day to resolve (YYYY-MM-DD)
    day: String,

    /// Attendance type override
    #[arg(long = "type")]
    attendance_type: Option<String>,

    /// Fixed hours (e.g. "09:00-18:00")
    #[arg(long)]
    hours: Option<String>,

    #[command(flatten)]
    write_mode: WriteMode,
}

#[derive(Args, Debug, Clone)]
struct AutoFillArgs {
    /// Month to auto-fill (default: current month)
    #[arg(long)]
    month: Option<String>,

    /// Attendance type (required unless --hours is provided)
    #[arg(long, short = 't')]
    r#type: Option<String>,

    /// Hours range (e.g., "09:00-18:00")
    #[arg(long)]
    hours: Option<String>,

    /// Include weekends (Fri/Sat) -- skipped by default
    #[arg(long)]
    include_weekends: bool,

    /// Safety cap: max days to fill in one run (default: 10)
    #[arg(long, default_value = "10")]
    max_days: u32,

    #[command(flatten)]
    write_mode: WriteMode,
}

#[derive(Args, Debug, Clone)]
struct SalaryArgs {
    /// Number of months to show (default: 2)
    #[arg(long, default_value = "2")]
    months: u32,
}

#[derive(Subcommand, Debug, Clone)]
enum AttendanceReportCommand {
    /// Report attendance for today using the current local time
    Today(AttendanceReportTodayArgs),

    /// Report attendance for a single day
    Day(AttendanceReportDayArgs),

    /// Report attendance for a date range
    Range(AttendanceReportRangeArgs),
}

#[derive(Subcommand, Debug, Clone)]
enum AttendanceCommand {
    /// Show attendance status for a month
    Status(AttendanceMonthArgs),

    /// Show attendance errors for a month
    Errors(AttendanceMonthArgs),

    /// Get overview for a month: identity, summary, errors, missing days, suggested actions
    Overview(OverviewArgs),

    /// Report attendance explicitly for today, one day, or a range
    Report {
        #[command(subcommand)]
        command: AttendanceReportCommand,
    },

    /// Automatically fill all missing days in a month (dry-run by default)
    AutoFill(AutoFillArgs),

    /// Resolve a single error day using the provider-discovered fix target
    Resolve(AttendanceResolveArgs),

    /// List available attendance types (from cache or server)
    Types,

    /// Show absences initial data (symbols and display names)
    Absences,
}

#[derive(Subcommand, Debug, Clone)]
enum PayslipCommand {
    /// Download a password-protected payslip PDF
    Download {
        /// Month in YYYY-MM format (defaults to previous month)
        #[arg(long)]
        month: Option<String>,

        /// Output file path
        #[arg(long)]
        output: Option<PathBuf>,
    },

    /// Open a payslip in the system PDF viewer; no file is stored in shaon's cache
    View {
        /// Month in YYYY-MM format (defaults to previous month)
        #[arg(long)]
        month: Option<String>,
    },

    /// Sensitive: prints the current Hilan login password in plaintext to standard output. Requires --force-sensitive-output and should only be used on a private terminal you control.
    Password {
        /// Required acknowledgement for printing the current Hilan password in plaintext
        #[arg(long)]
        force_sensitive_output: bool,
    },
}

#[derive(Subcommand, Debug, Clone)]
enum PayrollCommand {
    /// Download, inspect, or unlock payslips
    Payslip {
        #[command(subcommand)]
        command: PayslipCommand,
    },

    /// Show salary summary for recent months
    Salary(SalaryArgs),
}

#[derive(Subcommand, Debug, Clone)]
enum ReportsCommand {
    /// Fetch a named Hilan report
    Show {
        /// Report name (e.g. ErrorsReportNEW, MissingReportNEW)
        name: String,
    },

    /// Show the analyzed attendance sheet
    Sheet,

    /// Show the attendance correction log
    Corrections,
}

#[derive(Subcommand, Debug, Clone)]
enum CacheRefreshCommand {
    /// Refresh the attendance-type ontology cache
    AttendanceTypes,
}

#[derive(Subcommand, Debug, Clone)]
enum CacheCommand {
    /// Refresh locally cached admin data
    Refresh {
        #[command(subcommand)]
        command: CacheRefreshCommand,
    },
}

#[derive(Subcommand, Debug, Clone)]
enum Commands {
    /// Authenticate with Hilan and store verified credentials
    Auth(AuthArgs),

    /// Attendance reads and writes
    Attendance {
        #[command(subcommand)]
        command: AttendanceCommand,
    },

    /// Payroll reads and payslip workflows
    Payroll {
        #[command(subcommand)]
        command: PayrollCommand,
    },

    /// Attendance-adjacent reports
    Reports {
        #[command(subcommand)]
        command: ReportsCommand,
    },

    /// Hidden admin cache operations
    #[command(hide = true)]
    Cache {
        #[command(subcommand)]
        command: CacheCommand,
    },

    /// Start MCP server (stdio transport)
    Serve,

    /// Generate shell completions
    Completions {
        /// Shell to generate completions for
        #[arg(value_enum)]
        shell: clap_complete::Shell,
    },
}

pub async fn run() -> Result<()> {
    let cli = Cli::parse();

    // MCP serve mode: bypass normal config/client init — each tool call
    // creates its own client. All logging goes to stderr so stdout stays
    // clean for the JSON-RPC protocol stream.
    if matches!(&cli.command, Commands::Serve) {
        return run_mcp_server().await;
    }

    let filter = if cli.verbose {
        "debug"
    } else if cli.quiet {
        "error"
    } else {
        "info"
    };
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();

    // Handle completions before config loading — no credentials needed.
    if let Commands::Completions { shell } = &cli.command {
        let mut cmd = Cli::command();
        clap_complete::generate(*shell, &mut cmd, "shaon", &mut std::io::stdout());
        return Ok(());
    }

    let config = match Config::load() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    };

    let subdomain = config.subdomain.clone();
    let json = cli.json;

    match cli.command {
        Commands::Auth(args) => run_auth(config, json, args.force_prompt).await?,
        Commands::Attendance { command } => {
            let client = provider_hilan::build_provider(config)?.into_inner();
            run_attendance_command(command, client, &subdomain, json).await?;
        }
        Commands::Payroll { command } => {
            let client = provider_hilan::build_provider(config)?.into_inner();
            run_payroll_command(command, client, json).await?;
        }
        Commands::Reports { command } => {
            let client = provider_hilan::build_provider(config)?.into_inner();
            run_reports_command(command, client, json).await?;
        }
        Commands::Cache { command } => {
            let client = provider_hilan::build_provider(config)?.into_inner();
            run_cache_command(command, client, &subdomain, json).await?;
        }
        Commands::Serve => unreachable!("handled above"),
        Commands::Completions { .. } => unreachable!("handled above"),
    }

    Ok(())
}

async fn run_auth(config: Config, json: bool, force_prompt: bool) -> Result<()> {
    if force_prompt {
        prompt_and_store_credentials(config).await?;
    } else {
        match config.get_password() {
            Ok(_) => {
                eprintln!("Password already available. Testing login...");
                let mut client = provider_hilan::build_provider(config.clone())?.into_inner();
                if let Err(err) = client.login().await {
                    if should_prompt_for_fresh_password(&err) {
                        eprintln!(
                            "Stored credentials failed verification. Prompting for a new password..."
                        );
                        prompt_and_store_credentials(config).await?;
                    } else {
                        return Err(err);
                    }
                }
            }
            Err(_) => {
                prompt_and_store_credentials(config).await?;
            }
        }
    }

    if json {
        print_json(&serde_json::json!({"status": "ok"}))?;
    }
    Ok(())
}

fn should_prompt_for_fresh_password(err: &anyhow::Error) -> bool {
    let message = err.to_string().to_lowercase();
    message.contains("login failed") || message.contains("password change required")
}

async fn prompt_and_store_credentials(config: Config) -> Result<Config> {
    let password = rpassword::prompt_password("Enter your Hilan password (input is hidden): ")
        .context("read password from terminal")?;
    let pending = config.prepare_stored_credentials(password);
    let mut client = provider_hilan::build_provider(config.clone())?.into_inner();
    client.login_with_password(pending.password()).await?;
    pending.commit()
}

async fn run_attendance_command(
    command: AttendanceCommand,
    client: HilanClient,
    subdomain: &str,
    json: bool,
) -> Result<()> {
    match command {
        AttendanceCommand::Status(args) => {
            let month = parse_month_or_current(args.month.as_deref())?;
            let mut provider = HilanProvider::from_client(client);
            let cal = provider
                .month_calendar(month)
                .await
                .map_err(provider_error)?;
            if json {
                print_json(&build_status_response(&cal))?;
            } else {
                use_cases::print_calendar(&cal);
            }
        }
        AttendanceCommand::Errors(args) => {
            let month = parse_month_or_current(args.month.as_deref())?;
            let mut provider = HilanProvider::from_client(client);
            let overview =
                use_cases::build_overview(&mut provider, month, Local::now().date_naive())
                    .await
                    .map_err(provider_error)?;
            if json {
                print_json(&build_errors_response(&overview))?;
            } else {
                use_cases::print_error_days(&overview.calendar);
            }
        }
        AttendanceCommand::Overview(args) => {
            let mut provider = HilanProvider::from_client(client);
            run_overview(&mut provider, args.month.as_deref(), args.detailed, json).await?;
        }
        AttendanceCommand::Report { command } => {
            run_attendance_report_command(command, client, subdomain, json).await?;
        }
        AttendanceCommand::AutoFill(args) => {
            let month_date = parse_month_or_current(args.month.as_deref())?;
            let hours_range = args.hours.as_deref().map(parse_hours_range).transpose()?;
            let mut provider = HilanProvider::from_client(client);

            if args.r#type.is_none() && hours_range.is_none() {
                let mut msg = String::from("attendance auto-fill requires --type or --hours.\n");
                match use_cases::describe_attendance_types(&mut provider).await {
                    Ok(help) => msg.push_str(&help),
                    Err(_) => {
                        msg.push_str(
                            "Use `shaon attendance types` to inspect available types, or pass --type <code>.",
                        );
                    }
                }
                bail!("{msg}");
            }

            let resolved_type =
                use_cases::resolve_attendance_type(&mut provider, args.r#type.as_deref())
                    .await
                    .map_err(provider_error)?;
            let cal = provider
                .month_calendar(month_date)
                .await
                .map_err(provider_error)?;
            let result = use_cases::auto_fill(
                &mut provider,
                &cal,
                use_cases::AutoFillOptions {
                    type_code: resolved_type.as_ref().map(|resolved| resolved.code.clone()),
                    type_display: resolved_type
                        .map(|resolved| resolved.display)
                        .unwrap_or_default(),
                    hours: hours_range,
                    include_weekends: args.include_weekends,
                    mode: args.write_mode.provider_mode(),
                    max_days: args.max_days,
                    today: Local::now().date_naive(),
                },
            )
            .await
            .map_err(provider_error)?;

            if json {
                print_json(&result)?;
            } else {
                use_cases::print_auto_fill(&result);
            }
        }
        AttendanceCommand::Resolve(args) => {
            let date = parse_date(&args.day)?;
            let hours_range = args.hours.as_deref().map(parse_hours_range).transpose()?;
            let mut provider = HilanProvider::from_client(client);
            let type_code =
                use_cases::resolve_attendance_type(&mut provider, args.attendance_type.as_deref())
                    .await
                    .map_err(provider_error)?
                    .map(|resolved| resolved.code);
            let month = date.with_day(1).expect("valid day has month start");
            let fix_targets = provider.fix_targets(month).await.map_err(provider_error)?;
            let target = find_fix_target_for_date(&fix_targets, date)?;
            let preview = use_cases::fix_day(
                &mut provider,
                &target,
                type_code,
                hours_range,
                args.write_mode.provider_mode(),
            )
            .await
            .map_err(provider_error)?;
            if json {
                print_json(&ProviderWriteOutput::new("resolve", &preview))?;
            } else {
                print_provider_preview("resolve", &preview);
            }
        }
        AttendanceCommand::Types => {
            let mut provider = HilanProvider::from_client(client);
            let types = provider.attendance_types().await.map_err(provider_error)?;
            if json {
                print_json(&build_types_response(subdomain, &types))?;
            } else {
                use_cases::print_attendance_types(&types);
            }
        }
        AttendanceCommand::Absences => {
            let mut provider = HilanProvider::from_client(client);
            let symbols = use_cases::load_absence_symbols(&mut provider)
                .await
                .map_err(provider_error)?;
            if json {
                print_json(&build_absences_response(&symbols))?;
            } else {
                use_cases::print_absence_symbols(&symbols);
            }
        }
    }

    Ok(())
}

async fn run_attendance_report_command(
    command: AttendanceReportCommand,
    mut client: HilanClient,
    subdomain: &str,
    json: bool,
) -> Result<()> {
    match command {
        AttendanceReportCommand::Today(args) => {
            if !args.r#in && !args.out {
                bail!("attendance report today requires one of --in or --out");
            }
            if args.out && args.attendance_type.is_some() {
                bail!("--type is only supported with `shaon attendance report today --in`");
            }

            let execute = args.write_mode.should_execute();
            if args.r#in {
                if args.attendance_type.is_some() {
                    client.ensure_authenticated().await?;
                }
                let type_code = resolve_attendance_type_code(
                    &mut client,
                    subdomain,
                    args.attendance_type.as_deref(),
                )
                .await?;
                let submit = attendance::AttendanceSubmit {
                    date: Local::now().date_naive(),
                    attendance_type_code: type_code,
                    entry_time: Some(current_local_time_hhmm()),
                    exit_time: None,
                    comment: None,
                    clear_entry: false,
                    clear_exit: false,
                    clear_comment: false,
                    default_work_day: args.attendance_type.is_none(),
                };
                let preview = attendance::submit_day(&mut client, &submit, execute).await?;
                if json {
                    print_json(&WriteOutput::new("report-today-in", &preview))?;
                } else {
                    print_submit_preview("report-today-in", &preview);
                }
            } else {
                let submit = attendance::AttendanceSubmit {
                    date: Local::now().date_naive(),
                    attendance_type_code: None,
                    entry_time: None,
                    exit_time: Some(current_local_time_hhmm()),
                    comment: None,
                    clear_entry: false,
                    clear_exit: false,
                    clear_comment: false,
                    default_work_day: false,
                };
                let preview = attendance::submit_day(&mut client, &submit, execute).await?;
                if json {
                    print_json(&WriteOutput::new("report-today-out", &preview))?;
                } else {
                    print_submit_preview("report-today-out", &preview);
                }
            }
        }
        AttendanceReportCommand::Day(args) => {
            let date = parse_date(&args.day)?;
            let hours_range = args.hours.as_deref().map(parse_hours_range).transpose()?;
            let mut provider = HilanProvider::from_client(client);
            let type_code =
                use_cases::resolve_attendance_type(&mut provider, args.attendance_type.as_deref())
                    .await
                    .map_err(provider_error)?
                    .map(|resolved| resolved.code);
            let change = build_day_change(date, type_code, hours_range)?;
            let preview = provider
                .submit_day(&change, args.write_mode.provider_mode())
                .await
                .map_err(provider_error)?;
            if json {
                print_json(&ProviderWriteOutput::new("report-day", &preview))?;
            } else {
                print_provider_preview("report-day", &preview);
            }
        }
        AttendanceReportCommand::Range(args) => {
            let from_date = parse_date(&args.from)?;
            let to_date = parse_date(&args.to)?;
            if from_date > to_date {
                bail!("--from must be before or equal to --to");
            }
            let hours_range = args.hours.as_deref().map(parse_hours_range).transpose()?;
            let mut provider = HilanProvider::from_client(client);
            let type_code =
                use_cases::resolve_attendance_type(&mut provider, args.attendance_type.as_deref())
                    .await
                    .map_err(provider_error)?
                    .map(|resolved| resolved.code);
            let previews = use_cases::fill_range(
                &mut provider,
                from_date,
                to_date,
                use_cases::FillRangeOptions {
                    attendance_type_code: type_code,
                    hours: hours_range,
                    include_weekends: args.include_weekends,
                    mode: args.write_mode.provider_mode(),
                },
            )
            .await
            .map_err(provider_error)?;

            if json {
                let outputs: Vec<ProviderWriteOutput<'_>> = previews
                    .iter()
                    .map(|preview| ProviderWriteOutput::new("report-range", preview))
                    .collect();
                print_json(&outputs)?;
            } else {
                for preview in &previews {
                    print_provider_preview("report-range", preview);
                }
            }
        }
    }

    Ok(())
}

async fn run_payroll_command(
    command: PayrollCommand,
    client: HilanClient,
    json: bool,
) -> Result<()> {
    match command {
        PayrollCommand::Payslip { command } => run_payslip_command(command, client, json).await?,
        PayrollCommand::Salary(args) => {
            let mut provider = HilanProvider::from_client(client);
            let summary = provider
                .salary_summary(args.months)
                .await
                .map_err(provider_error)?;
            if json {
                print_json(&build_salary_response(&summary))?;
            } else {
                if !summary.label.is_empty() {
                    println!("Salary row: {}", summary.label);
                }
                for entry in &summary.entries {
                    println!(
                        "{}: {}",
                        entry.month.format("%Y-%m"),
                        format_number(entry.amount)
                    );
                }
                if let Some(percent_diff) = summary.percent_diff {
                    println!("Change over latest month: {:+.2}%", percent_diff);
                }
            }
        }
    }

    Ok(())
}

async fn run_payslip_command(
    command: PayslipCommand,
    mut client: HilanClient,
    json: bool,
) -> Result<()> {
    match command {
        PayslipCommand::Download { month, output } => {
            let month = parse_month_or_previous(month.as_deref())?;
            let mut provider = HilanProvider::from_client(client);
            let download = provider
                .download_payslip(month, output.as_deref())
                .await
                .map_err(provider_error)?;
            print_payslip_download(download, json)?;
        }
        PayslipCommand::View { month } => {
            let month = parse_month_or_previous(month.as_deref())?;
            let bytes = client.viewable_payslip_pdf_bytes(month).await?;
            open_pdf_in_system_viewer(&bytes)?;
            if json {
                print_json(&serde_json::json!({
                    "status": "ok",
                    "month": month.format("%Y-%m").to_string(),
                    "viewer": viewer_name(),
                }))?;
            } else {
                println!(
                    "Opened payslip for {} in {}.",
                    month.format("%Y-%m"),
                    viewer_name()
                );
            }
        }
        PayslipCommand::Password {
            force_sensitive_output,
        } => {
            if !force_sensitive_output {
                bail!(
                    "Sensitive command. Re-run `shaon payroll payslip password --force-sensitive-output` only on a private interactive terminal you control."
                );
            }
            let password = client.config().get_password()?;
            eprintln!("{PAYSLIP_PASSWORD_WARNING}");
            if json {
                print_json(&serde_json::json!({
                    "password": password.expose_secret(),
                }))?;
            } else {
                println!("{}", password.expose_secret());
            }
        }
    }

    Ok(())
}

async fn run_reports_command(
    command: ReportsCommand,
    client: HilanClient,
    json: bool,
) -> Result<()> {
    let (kind, requested, spec) = match command {
        ReportsCommand::Show { name } => ("named", name.clone(), ReportSpec::Named(name)),
        ReportsCommand::Sheet => (
            "sheet",
            "sheet".to_string(),
            ReportSpec::Path(SHEET_REPORT_PATH.to_string()),
        ),
        ReportsCommand::Corrections => (
            "corrections",
            "corrections".to_string(),
            ReportSpec::Path(CORRECTIONS_REPORT_PATH.to_string()),
        ),
    };

    let mut provider = HilanProvider::from_client(client);
    let table = provider.report(spec).await.map_err(provider_error)?;
    if json {
        print_json(&build_report_response(kind, &requested, &table))?;
    } else {
        use_cases::print_report_table(&table);
    }

    Ok(())
}

async fn run_cache_command(
    command: CacheCommand,
    mut client: HilanClient,
    subdomain: &str,
    json: bool,
) -> Result<()> {
    match command {
        CacheCommand::Refresh {
            command: CacheRefreshCommand::AttendanceTypes,
        } => {
            client.ensure_authenticated().await?;
            let ont = ontology::sync_from_calendar(&mut client, subdomain).await?;
            if json {
                print_json(&ont)?;
            } else {
                ont.print_table();
            }
        }
    }

    Ok(())
}

fn build_day_change(
    date: NaiveDate,
    attendance_type_code: Option<String>,
    hours: Option<(String, String)>,
) -> Result<hr_core::AttendanceChange> {
    let (entry_time, exit_time) = match hours {
        Some((entry, exit)) => (Some(entry), Some(exit)),
        None => (None, None),
    };
    let use_default_attendance_type = attendance_type_code.is_none() && entry_time.is_some();

    if attendance_type_code.is_none() && entry_time.is_none() {
        bail!("attendance report day requires --type or --hours");
    }

    Ok(hr_core::AttendanceChange {
        date,
        attendance_type_code,
        use_default_attendance_type,
        entry_time,
        exit_time,
        comment: None,
        clear_entry: false,
        clear_exit: false,
        clear_comment: false,
    })
}

fn find_fix_target_for_date(
    fix_targets: &[CoreFixTarget],
    date: NaiveDate,
) -> Result<CoreFixTarget> {
    let mut matches = fix_targets
        .iter()
        .filter(|target| target.date == date)
        .cloned();

    match (matches.next(), matches.next()) {
        (Some(target), None) => Ok(target),
        (Some(_), Some(_)) => bail!(
            "Found multiple fix targets for {}. Inspect `shaon attendance errors --month {}` and retry.",
            date.format("%Y-%m-%d"),
            date.format("%Y-%m")
        ),
        _ => bail!(
            "No fix target found for {}. Run `shaon attendance errors --month {}` first and confirm the day is still fixable.",
            date.format("%Y-%m-%d"),
            date.format("%Y-%m")
        ),
    }
}

fn build_errors_response(overview: &use_cases::OverviewData) -> ErrorsResponse {
    ErrorsResponse {
        month: overview.month.format("%Y-%m").to_string(),
        employee_id: overview.calendar.employee_id.clone(),
        error_count: overview.error_days.len(),
        errors: overview
            .error_days
            .iter()
            .map(error_day_from_overview)
            .collect(),
    }
}

fn build_status_response(calendar: &hr_core::MonthCalendar) -> StatusResponse {
    StatusResponse {
        month: calendar.month.format("%Y-%m").to_string(),
        employee_id: calendar.employee_id.clone(),
        days: calendar
            .days
            .iter()
            .map(status_day_from_calendar_day)
            .collect(),
    }
}

fn build_types_response(subdomain: &str, types: &[CoreAttendanceType]) -> TypesResponse {
    TypesResponse {
        subdomain: subdomain.to_string(),
        types: types.to_vec(),
    }
}

fn build_absences_response(symbols: &[hr_core::AbsenceSymbol]) -> AbsencesResponse {
    AbsencesResponse {
        symbol_count: symbols.len(),
        symbols: symbols.to_vec(),
    }
}

fn build_salary_response(summary: &hr_core::SalarySummary) -> SalaryResponse {
    SalaryResponse {
        label: summary.label.clone(),
        entries: summary
            .entries
            .iter()
            .map(|entry| SalaryEntryResponse {
                month: entry.month.format("%Y-%m").to_string(),
                amount: entry.amount,
            })
            .collect(),
        percent_diff: summary.percent_diff,
    }
}

fn build_overview_response(overview: &use_cases::OverviewData) -> OverviewResponse {
    let missing_dates: BTreeSet<NaiveDate> = overview.missing_days.iter().copied().collect();

    let missing_days = overview
        .calendar
        .days
        .iter()
        .filter(|day| missing_dates.contains(&day.date))
        .map(missing_day_from_calendar_day)
        .collect();

    let suggested_actions = overview
        .suggested_actions
        .iter()
        .map(suggested_action_from_plan)
        .collect();

    OverviewResponse {
        user: UserInfo {
            user_id: overview.identity.user_id.clone(),
            employee_id: overview.identity.employee_id.clone(),
            name: overview.identity.display_name.clone(),
            is_manager: overview.identity.is_manager,
        },
        month: overview.month.format("%Y-%m").to_string(),
        summary: MonthSummary {
            total_work_days: overview.summary.total_work_days,
            reported: overview.summary.reported,
            missing: overview.summary.missing,
            errors: overview.summary.errors,
        },
        attendance_types: overview.attendance_types.clone(),
        error_days: overview
            .error_days
            .iter()
            .map(error_day_from_overview)
            .collect(),
        missing_days,
        suggested_actions,
    }
}

fn status_day_from_calendar_day(day: &hr_core::CalendarDay) -> StatusDay {
    StatusDay {
        date: day.date.format("%Y-%m-%d").to_string(),
        day_name: day.day_name.clone(),
        has_error: day.has_error,
        error_message: day.error_message.clone(),
        entry_time: day.entry_time.clone(),
        exit_time: day.exit_time.clone(),
        attendance_type: day.attendance_type.clone(),
        total_hours: day.total_hours.clone(),
        source: day.source,
    }
}

fn missing_day_from_calendar_day(day: &hr_core::CalendarDay) -> MissingDay {
    MissingDay {
        date: day.date.format("%Y-%m-%d").to_string(),
        day_name: day.day_name.clone(),
    }
}

fn error_day_from_overview(entry: &use_cases::OverviewErrorDay) -> ErrorDay {
    let fix_params_candidates = error_fix_params_candidates(&entry.fix_targets);
    let fix_params = match fix_params_candidates.as_slice() {
        [candidate] => Some(candidate.clone()),
        _ => None,
    };

    ErrorDay {
        date: entry.day.date.format("%Y-%m-%d").to_string(),
        day_name: entry.day.day_name.clone(),
        error_message: entry
            .day
            .error_message
            .clone()
            .unwrap_or_else(|| "missing report".to_string()),
        fix_params,
        fix_params_candidates,
    }
}

fn suggested_action_from_plan(action: &use_cases::SuggestedActionPlan) -> SuggestedAction {
    match action {
        use_cases::SuggestedActionPlan::FixErrors {
            month,
            count,
            fixable_targets,
        } => SuggestedAction::FixErrors {
            reason: format!("{count} day(s) have attendance errors"),
            safety: "dry_run_default".to_string(),
            month: month.format("%Y-%m").to_string(),
            count: *count,
            fixable_days: fixable_targets
                .iter()
                .filter_map(|target| {
                    error_fix_params_from_target(target).map(|params| FixableDay {
                        date: target.date.format("%Y-%m-%d").to_string(),
                        report_id: params.report_id,
                        error_type: params.error_type,
                    })
                })
                .collect(),
        },
        use_cases::SuggestedActionPlan::FillMissing { from, to, count } => {
            SuggestedAction::FillMissing {
                reason: format!("{count} work day(s) have no attendance report"),
                safety: "dry_run_default".to_string(),
                from: from.format("%Y-%m-%d").to_string(),
                to: to.format("%Y-%m-%d").to_string(),
                count: *count,
            }
        }
    }
}

fn build_report_response(kind: &str, requested: &str, table: &ReportTable) -> ReportResponse {
    ReportResponse {
        report: ReportMetadata {
            kind: kind.to_string(),
            requested: requested.to_string(),
            provider_name: table.name.clone(),
        },
        column_count: table.headers.len(),
        row_count: table.rows.len(),
        columns: table
            .headers
            .iter()
            .enumerate()
            .map(|(index, name)| ReportColumn {
                index,
                name: name.clone(),
            })
            .collect(),
        rows: table
            .rows
            .iter()
            .enumerate()
            .map(|(index, cells)| ReportRow {
                index,
                cells: cells.clone(),
            })
            .collect(),
    }
}

fn print_payslip_download(download: DocumentDownload, json: bool) -> Result<()> {
    if json {
        print_json(&download)?;
    } else {
        println!(
            "Saved password-protected payslip for {} to {} ({} bytes)",
            download.month.format("%Y-%m"),
            download.path.display(),
            download.size_bytes
        );
    }

    Ok(())
}

async fn run_mcp_server() -> Result<()> {
    shaon_mcp::serve_stdio().await
}

#[cfg(target_os = "macos")]
fn open_pdf_in_system_viewer(bytes: &[u8]) -> Result<()> {
    let mut child = Command::new("/usr/bin/open")
        .args(["-f", "-a", "Preview"])
        .stdin(Stdio::piped())
        .spawn()
        .context("launch Preview for payslip view")?;

    let mut stdin = child.stdin.take().context("open Preview stdin")?;
    stdin
        .write_all(bytes)
        .context("stream payslip PDF to Preview")?;
    drop(stdin);

    let status = child.wait().context("wait for Preview launcher")?;
    if !status.success() {
        bail!("Preview launcher exited with status {status}");
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn open_pdf_in_system_viewer(_bytes: &[u8]) -> Result<()> {
    bail!("`shaon payroll payslip view` is currently supported only on macOS");
}

#[cfg(target_os = "macos")]
fn viewer_name() -> &'static str {
    "Preview"
}

#[cfg(not(target_os = "macos"))]
fn viewer_name() -> &'static str {
    "the system PDF viewer"
}

fn parse_month_or_previous(month: Option<&str>) -> Result<NaiveDate> {
    match month {
        Some(value) => parse_month(value),
        None => Ok(client::previous_month_start(Local::now().date_naive())),
    }
}

fn parse_month_or_current(month: Option<&str>) -> Result<NaiveDate> {
    match month {
        Some(value) => parse_month(value),
        None => Ok(current_month_start()),
    }
}

fn parse_month(value: &str) -> Result<NaiveDate> {
    Ok(NaiveDate::parse_from_str(
        &format!("{value}-01"),
        "%Y-%m-%d",
    )?)
}

fn parse_date(value: &str) -> Result<NaiveDate> {
    Ok(NaiveDate::parse_from_str(value, "%Y-%m-%d")?)
}

fn parse_hours_range(value: &str) -> Result<(String, String)> {
    let (entry, exit) = value
        .split_once('-')
        .ok_or_else(|| anyhow::anyhow!("hours must be in HH:MM-HH:MM format"))?;

    if !is_time_pattern(entry) || !is_time_pattern(exit) {
        bail!("hours must be in HH:MM-HH:MM format");
    }

    Ok((entry.to_string(), exit.to_string()))
}

async fn resolve_attendance_type_code(
    client: &mut HilanClient,
    subdomain: &str,
    requested: Option<&str>,
) -> Result<Option<String>> {
    let Some(requested) = requested else {
        return Ok(None);
    };

    let ontology = ontology::OrgOntology::load_or_sync(client, subdomain).await?;
    Ok(Some(ontology.validate_type(requested)?.code.clone()))
}

fn current_local_time_hhmm() -> String {
    Local::now().format("%H:%M").to_string()
}

fn print_json(value: &impl Serialize) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

fn print_submit_preview(action: &str, preview: &attendance::SubmitPreview) {
    let mode = if preview.executed {
        "EXECUTED"
    } else {
        "DRY RUN"
    };
    println!("{action} [{mode}]");
    println!("Target URL: {}", preview.url);
    println!("Employee ID: {}", preview.employee_id);
    println!("Button: {} = {}", preview.button_name, preview.button_value);
    println!("{}", preview.payload_display);
}

fn print_provider_preview(action: &str, preview: &CoreWritePreview) {
    let mode = if preview.executed {
        "EXECUTED"
    } else {
        "DRY RUN"
    };
    println!("{action} [{mode}]");
    if let Some(url) = preview.debug_field("url") {
        println!("Target URL: {url}");
    }
    if let Some(employee_id) = preview.debug_field("employee_id") {
        println!("Employee ID: {employee_id}");
    }
    if let (Some(button_name), Some(button_value)) = (
        preview.debug_field("button_name"),
        preview.debug_field("button_value"),
    ) {
        println!("Button: {button_name} = {button_value}");
    }
    if let Some(payload_display) = preview.debug_field("payload_display") {
        println!("{payload_display}");
    } else {
        println!("{}", preview.summary);
    }
}

fn current_month_start() -> NaiveDate {
    Local::now().date_naive().with_day(1).unwrap()
}

fn format_number(value: u64) -> String {
    let digits = value.to_string();
    let len = digits.len();
    let mut out = String::with_capacity(len + len / 3);

    for (idx, ch) in digits.chars().enumerate() {
        if idx > 0 && (len - idx) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }

    out
}

// ---------------------------------------------------------------------------
// Overview command implementation
// ---------------------------------------------------------------------------

async fn run_overview(
    provider: &mut impl AttendanceProvider,
    month_arg: Option<&str>,
    detailed: bool,
    json: bool,
) -> Result<()> {
    let month = parse_month_or_current(month_arg)?;
    let today = Local::now().date_naive();
    let overview = use_cases::build_overview(provider, month, today)
        .await
        .map_err(provider_error)?;
    let response = build_overview_response(&overview);

    if json {
        if detailed {
            // `--detailed` extends overview with the full status-day schema.
            let mut value = serde_json::to_value(&response)?;
            value["days"] = serde_json::to_value(build_status_response(&overview.calendar).days)?;
            println!("{}", serde_json::to_string_pretty(&value)?);
        } else {
            print_json(&response)?;
        }
    } else {
        print_overview_human(&response);
        if detailed {
            println!();
            print_calendar_verbose(&overview.calendar);
        }
    }

    Ok(())
}

fn print_overview_human(ctx: &OverviewResponse) {
    println!(
        "User: {} (employee {})",
        ctx.user.name, ctx.user.employee_id
    );
    println!(
        "Month: {} -- {}/{} reported, {} errors, {} missing",
        ctx.month,
        ctx.summary.reported,
        ctx.summary.total_work_days,
        ctx.summary.errors,
        ctx.summary.missing
    );

    if !ctx.missing_days.is_empty() {
        println!();
        println!("Missing days:");
        for day in &ctx.missing_days {
            println!("  {} ({})", day.date, day.day_name);
        }
    }

    if !ctx.error_days.is_empty() {
        println!();
        println!("Error days:");
        for ed in &ctx.error_days {
            println!("  {} ({}) -- {}", ed.date, ed.day_name, ed.error_message);
        }
    }

    if !ctx.suggested_actions.is_empty() {
        println!();
        println!("Suggested actions:");
        for action in &ctx.suggested_actions {
            let (kind, reason, cmd_hint) = match action {
                SuggestedAction::FixErrors { month, reason, .. } => (
                    "fix_errors",
                    reason.as_str(),
                    format!("shaon attendance errors --month {month}"),
                ),
                SuggestedAction::FillMissing {
                    from, to, reason, ..
                } => (
                    "fill_missing",
                    reason.as_str(),
                    format!(
                        "shaon attendance report range --from {from} --to {to} --type <type> --hours <HH:MM-HH:MM>"
                    ),
                ),
            };
            println!("  [{kind}] {reason} -- {cmd_hint}");
        }
    }
}

fn error_fix_params_from_target(target: &hr_core::FixTarget) -> Option<ErrorFixParams> {
    let provider_ref_parts = target.provider_ref.split_once(':');
    let report_id = target
        .metadata
        .get("report_id")
        .cloned()
        .or_else(|| provider_ref_parts.map(|(report_id, _)| report_id.to_string()));
    let error_type = target
        .metadata
        .get("error_type")
        .cloned()
        .or_else(|| provider_ref_parts.map(|(_, error_type)| error_type.to_string()));

    match (report_id, error_type) {
        (Some(report_id), Some(error_type)) => Some(ErrorFixParams {
            report_id,
            error_type,
        }),
        _ => None,
    }
}

fn error_fix_params_candidates(targets: &[hr_core::FixTarget]) -> Vec<ErrorFixParams> {
    targets
        .iter()
        .filter_map(error_fix_params_from_target)
        .collect()
}

fn print_calendar_verbose(calendar: &hr_core::MonthCalendar) {
    println!(
        "Calendar {} (employee {})",
        calendar.month.format("%Y-%m"),
        calendar.employee_id
    );
    println!("Date        Day    Entry  Exit   Type                  Source     Hours  Error");
    println!("----------  -----  -----  -----  --------------------  ---------  -----  -----");

    for day in &calendar.days {
        println!(
            "{:<10}  {:<5}  {:<5}  {:<5}  {:<20}  {:<9}  {:<5}  {}",
            day.date.format("%Y-%m-%d"),
            day.day_name,
            day.entry_time.as_deref().unwrap_or(""),
            day.exit_time.as_deref().unwrap_or(""),
            use_cases::display_attendance_label(day),
            attendance_source_label(day.source),
            day.total_hours.as_deref().unwrap_or(""),
            if day.has_error { "yes" } else { "" },
        );
    }
}

fn attendance_source_label(source: hr_core::AttendanceSource) -> &'static str {
    match source {
        hr_core::AttendanceSource::UserReported => "user",
        hr_core::AttendanceSource::SystemAutoFill => "auto",
        hr_core::AttendanceSource::Holiday => "holiday",
        hr_core::AttendanceSource::Unreported => "",
    }
}

fn provider_error(err: ProviderError) -> anyhow::Error {
    if err.code == "attendance_fix_partial_commit" {
        if let Some(details) = err.details.as_ref() {
            if let Some(rendered) = render_partial_commit_message(&err.message, details) {
                return anyhow::anyhow!(rendered);
            }
        }
    }
    anyhow::anyhow!("{err}")
}

fn render_partial_commit_message(headline: &str, details: &serde_json::Value) -> Option<String> {
    let pc = details.get("partial_commit")?;
    let mut out = String::new();
    out.push_str("attendance_fix_partial_commit: ");
    out.push_str(headline);
    out.push_str("\n\nCompleted steps:");
    let completed = pc.get("completed_steps").and_then(|v| v.as_array());
    if completed.map(|a| a.is_empty()).unwrap_or(true) {
        out.push_str("\n  (none)");
    } else {
        for step in completed.unwrap() {
            let outcome = step.get("outcome").and_then(|v| v.as_str()).unwrap_or("?");
            let key = step.get("key").and_then(|v| v.as_str()).unwrap_or("?");
            let label = step.get("label").and_then(|v| v.as_str()).unwrap_or("");
            let marker = if outcome == "skipped" { "-" } else { "✓" };
            out.push_str(&format!("\n  {marker} {key} ({outcome}): {label}"));
        }
    }
    out.push_str("\n\nFailed step:");
    if let Some(failed) = pc.get("failed_step") {
        let outcome = failed
            .get("outcome")
            .and_then(|v| v.as_str())
            .unwrap_or("?");
        let key = failed.get("key").and_then(|v| v.as_str()).unwrap_or("?");
        let label = failed.get("label").and_then(|v| v.as_str()).unwrap_or("");
        out.push_str(&format!("\n  ✗ {key} ({outcome}): {label}"));
    } else {
        out.push_str("\n  (unknown)");
    }
    out.push_str("\n\nRemaining steps (not attempted):");
    let remaining = pc.get("remaining_steps").and_then(|v| v.as_array());
    if remaining.map(|a| a.is_empty()).unwrap_or(true) {
        out.push_str("\n  (none)");
    } else {
        for step in remaining.unwrap() {
            let key = step.get("key").and_then(|v| v.as_str()).unwrap_or("?");
            let label = step.get("label").and_then(|v| v.as_str()).unwrap_or("");
            out.push_str(&format!("\n  - {key}: {label}"));
        }
    }
    out.push_str(
        "\n\nHilan state may be inconsistent. Open the attendance calendar for this date and verify before retrying.",
    );
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use hr_core::{
        AbsenceSymbol, AttendanceSource, CalendarDay, MonthCalendar, SalaryEntry, SalarySummary,
        UserIdentity,
    };
    use std::collections::BTreeMap;

    fn sample_overview(targets: Vec<CoreFixTarget>) -> use_cases::OverviewData {
        let month = NaiveDate::from_ymd_opt(2026, 4, 1).unwrap();
        let fixable_targets = targets.clone();
        let missing_day = CalendarDay {
            date: NaiveDate::from_ymd_opt(2026, 4, 9).unwrap(),
            day_name: "Wed".to_string(),
            has_error: false,
            error_message: None,
            entry_time: None,
            exit_time: None,
            attendance_type: None,
            total_hours: None,
            source: AttendanceSource::Unreported,
        };
        let error_day = CalendarDay {
            date: NaiveDate::from_ymd_opt(2026, 4, 10).unwrap(),
            day_name: "Thu".to_string(),
            has_error: true,
            error_message: Some("missing report".to_string()),
            entry_time: None,
            exit_time: None,
            attendance_type: None,
            total_hours: None,
            source: AttendanceSource::Unreported,
        };

        use_cases::OverviewData {
            identity: UserIdentity {
                user_id: "123".to_string(),
                employee_id: "123".to_string(),
                display_name: "Test User".to_string(),
                is_manager: false,
            },
            month,
            calendar: MonthCalendar {
                month,
                employee_id: "123".to_string(),
                days: vec![missing_day.clone(), error_day.clone()],
            },
            attendance_types: Vec::new(),
            summary: use_cases::OverviewSummary {
                total_work_days: 2,
                reported: 0,
                missing: 1,
                errors: 1,
            },
            error_days: vec![use_cases::OverviewErrorDay {
                day: error_day,
                fix_targets: targets,
            }],
            missing_days: vec![missing_day.date],
            suggested_actions: vec![
                use_cases::SuggestedActionPlan::FixErrors {
                    month,
                    count: 1,
                    fixable_targets,
                },
                use_cases::SuggestedActionPlan::FillMissing {
                    from: missing_day.date,
                    to: missing_day.date,
                    count: 1,
                },
            ],
        }
    }

    #[test]
    fn attendance_report_day_parses() {
        let cli = Cli::try_parse_from([
            "shaon",
            "attendance",
            "report",
            "day",
            "2026-04-10",
            "--hours",
            "09:00-18:00",
        ])
        .expect("parse command");

        match cli.command {
            Commands::Attendance {
                command:
                    AttendanceCommand::Report {
                        command:
                            AttendanceReportCommand::Day(AttendanceReportDayArgs { day, hours, .. }),
                    },
            } => {
                assert_eq!(day, "2026-04-10");
                assert_eq!(hours.as_deref(), Some("09:00-18:00"));
            }
            other => panic!("unexpected command shape: {other:?}"),
        }
    }

    #[test]
    fn top_level_overview_is_rejected() {
        let err = Cli::try_parse_from(["shaon", "overview"]).expect_err("old alias should fail");
        let rendered = err.to_string();
        assert!(rendered.contains("unrecognized subcommand"));
    }

    #[test]
    fn auth_migrate_flag_is_rejected() {
        let err =
            Cli::try_parse_from(["shaon", "auth", "--migrate"]).expect_err("flag should fail");
        let rendered = err.to_string();
        assert!(rendered.contains("unexpected argument '--migrate'"));
    }

    #[test]
    fn auth_force_prompt_flag_parses() {
        let cli = Cli::try_parse_from(["shaon", "auth", "--force-prompt"]).expect("parse auth");

        match cli.command {
            Commands::Auth(AuthArgs { force_prompt }) => assert!(force_prompt),
            other => panic!("unexpected command shape: {other:?}"),
        }
    }

    #[test]
    fn should_prompt_for_fresh_password_only_on_credential_failures() {
        let stale = anyhow::anyhow!("Login failed: wrong password");
        let changed = anyhow::anyhow!("Password change required. Please update your password.");
        let captcha = anyhow::anyhow!("CAPTCHA required. Please log in via browser.");

        assert!(should_prompt_for_fresh_password(&stale));
        assert!(should_prompt_for_fresh_password(&changed));
        assert!(!should_prompt_for_fresh_password(&captcha));
    }

    #[test]
    fn find_fix_target_for_date_returns_the_matching_target() {
        let target = CoreFixTarget {
            date: NaiveDate::from_ymd_opt(2026, 4, 10).unwrap(),
            issue_kind: Some("missing_report".to_string()),
            provider_ref: "report:error".to_string(),
            metadata: BTreeMap::from([
                ("report_id".to_string(), "report".to_string()),
                ("error_type".to_string(), "error".to_string()),
            ]),
        };
        let overview = sample_overview(vec![target.clone()]);

        let found = find_fix_target_for_date(
            &overview.error_days[0].fix_targets,
            NaiveDate::from_ymd_opt(2026, 4, 10).unwrap(),
        )
        .expect("find target");

        assert_eq!(found, target);
    }

    #[test]
    fn find_fix_target_for_date_fails_when_missing() {
        let overview = sample_overview(Vec::new());

        let err = find_fix_target_for_date(
            &overview.error_days[0].fix_targets,
            NaiveDate::from_ymd_opt(2026, 4, 10).unwrap(),
        )
        .expect_err("missing target should fail");

        assert!(err
            .to_string()
            .contains("No fix target found for 2026-04-10"));
    }

    #[test]
    fn find_fix_target_for_date_fails_when_multiple_targets_match() {
        let date = NaiveDate::from_ymd_opt(2026, 4, 10).unwrap();
        let overview = sample_overview(vec![
            CoreFixTarget {
                date,
                issue_kind: Some("missing_report".to_string()),
                provider_ref: "report-1:63".to_string(),
                metadata: BTreeMap::from([
                    ("report_id".to_string(), "report-1".to_string()),
                    ("error_type".to_string(), "63".to_string()),
                ]),
            },
            CoreFixTarget {
                date,
                issue_kind: Some("missing_report".to_string()),
                provider_ref: "report-2:18".to_string(),
                metadata: BTreeMap::from([
                    ("report_id".to_string(), "report-2".to_string()),
                    ("error_type".to_string(), "18".to_string()),
                ]),
            },
        ]);

        let err = find_fix_target_for_date(&overview.error_days[0].fix_targets, date)
            .expect_err("multiple targets should fail");

        assert!(err
            .to_string()
            .contains("Found multiple fix targets for 2026-04-10"));
    }

    #[test]
    fn build_errors_response_preserves_multiple_fix_param_candidates() {
        let date = NaiveDate::from_ymd_opt(2026, 4, 10).unwrap();
        let overview = sample_overview(vec![
            CoreFixTarget {
                date,
                issue_kind: Some("missing_report".to_string()),
                provider_ref: "report-1:63".to_string(),
                metadata: BTreeMap::from([
                    ("report_id".to_string(), "report-1".to_string()),
                    ("error_type".to_string(), "63".to_string()),
                ]),
            },
            CoreFixTarget {
                date,
                issue_kind: Some("missing_report".to_string()),
                provider_ref: "report-2:18".to_string(),
                metadata: BTreeMap::from([
                    ("report_id".to_string(), "report-2".to_string()),
                    ("error_type".to_string(), "18".to_string()),
                ]),
            },
        ]);

        let response = build_errors_response(&overview);

        assert_eq!(response.error_count, 1);
        assert_eq!(response.errors[0].fix_params, None);
        assert_eq!(response.errors[0].fix_params_candidates.len(), 2);
    }

    #[test]
    fn build_errors_response_keeps_empty_fix_param_candidates_visible() {
        let overview = sample_overview(Vec::new());

        let value = serde_json::to_value(build_errors_response(&overview)).expect("serialize");

        assert!(value["errors"][0].get("fix_params_candidates").is_some());
        assert_eq!(
            value["errors"][0]["fix_params_candidates"]
                .as_array()
                .expect("array")
                .len(),
            0
        );
    }

    #[test]
    fn build_status_response_uses_stable_day_schema() {
        let overview = sample_overview(Vec::new());

        let value = serde_json::to_value(build_status_response(&overview.calendar))
            .expect("serialize status response");

        assert_eq!(value["month"], "2026-04");
        assert!(value["days"][0].get("day_name").is_some());
        assert!(value["days"][0].get("day").is_none());
        assert!(value["days"][0].get("source").is_some());
    }

    #[test]
    fn build_overview_response_uses_structured_missing_days_and_actions() {
        let date = NaiveDate::from_ymd_opt(2026, 4, 10).unwrap();
        let overview = sample_overview(vec![CoreFixTarget {
            date,
            issue_kind: Some("missing_report".to_string()),
            provider_ref: "report:error".to_string(),
            metadata: BTreeMap::from([
                ("report_id".to_string(), "report".to_string()),
                ("error_type".to_string(), "error".to_string()),
            ]),
        }]);

        let value =
            serde_json::to_value(build_overview_response(&overview)).expect("serialize overview");

        assert_eq!(value["missing_days"][0]["date"], "2026-04-09");
        assert_eq!(value["missing_days"][0]["day_name"], "Wed");
        assert_eq!(value["suggested_actions"][0]["kind"], "fix_errors");
        assert_eq!(value["suggested_actions"][0]["month"], "2026-04");
        assert!(value["suggested_actions"][0].get("params").is_none());
        assert_eq!(value["suggested_actions"][1]["kind"], "fill_missing");
        assert_eq!(value["suggested_actions"][1]["from"], "2026-04-09");
    }

    #[test]
    fn build_types_and_absences_responses_match_mcp_envelopes() {
        let types = vec![CoreAttendanceType {
            code: "120".to_string(),
            name_he: "רגיל".to_string(),
            name_en: Some("regular".to_string()),
        }];
        let symbols = vec![AbsenceSymbol {
            id: "1".to_string(),
            name: "Vacation".to_string(),
            display_name: Some("Vacation".to_string()),
        }];

        let types_value =
            serde_json::to_value(build_types_response("acme", &types)).expect("serialize types");
        let absences_value =
            serde_json::to_value(build_absences_response(&symbols)).expect("serialize absences");

        assert_eq!(types_value["subdomain"], "acme");
        assert_eq!(types_value["types"][0]["code"], "120");
        assert_eq!(absences_value["symbol_count"], 1);
        assert_eq!(absences_value["symbols"][0]["id"], "1");
    }

    #[test]
    fn build_salary_response_formats_months_as_year_month() {
        let summary = SalarySummary {
            label: "Net".to_string(),
            entries: vec![SalaryEntry {
                month: NaiveDate::from_ymd_opt(2026, 3, 1).unwrap(),
                amount: 123_456,
            }],
            percent_diff: Some(1.5),
        };

        let value = serde_json::to_value(build_salary_response(&summary)).expect("serialize");

        assert_eq!(value["entries"][0]["month"], "2026-03");
        assert_eq!(value["entries"][0]["amount"], 123456);
    }

    #[test]
    fn provider_error_renders_partial_commit_human_block() {
        let err = ProviderError::new(
            "attendance_fix_partial_commit",
            "partial commit while fixing 2026-04-09: 2 step(s) committed, then 'apply ...' failed: outcome unknown",
        )
        .with_details(serde_json::json!({
            "partial_commit": {
                "date": "2026-04-09",
                "completed_steps": [
                    {"key": "clear_error", "label": "clear the Hilan error", "committed": true, "outcome": "committed"},
                    {"key": "delete_conflict", "label": "delete the conflicting calendar row", "committed": true, "outcome": "committed"},
                ],
                "failed_step": {"key": "submit_attendance", "label": "apply the requested attendance via the calendar page", "committed": false, "outcome": "failed_outcome_unknown"},
                "remaining_steps": [],
                "source": "outcome unknown",
            }
        }));

        let rendered = format!("{}", provider_error(err));

        assert!(rendered.contains("attendance_fix_partial_commit"));
        assert!(rendered.contains("Completed steps"));
        assert!(rendered.contains("✓ clear_error (committed)"));
        assert!(rendered.contains("✓ delete_conflict (committed)"));
        assert!(rendered.contains("Failed step"));
        assert!(rendered.contains("✗ submit_attendance (failed_outcome_unknown)"));
        assert!(rendered.contains("(none)")); // remaining
        assert!(rendered.contains("Hilan state may be inconsistent"));
    }

    #[test]
    fn provider_error_falls_back_to_string_when_no_partial_commit_details() {
        let err = ProviderError::new("attendance_fix_failed", "Hilan rejected submission");

        let rendered = format!("{}", provider_error(err));

        assert!(rendered.contains("attendance_fix_failed"));
        assert!(rendered.contains("Hilan rejected submission"));
    }

    #[test]
    fn payslip_password_requires_force_sensitive_output_flag() {
        let err = Cli::try_parse_from(["shaon", "payroll", "payslip", "password"])
            .expect("parse without force flag still succeeds");

        match err.command {
            Commands::Payroll {
                command:
                    PayrollCommand::Payslip {
                        command:
                            PayslipCommand::Password {
                                force_sensitive_output,
                            },
                    },
            } => assert!(!force_sensitive_output),
            other => panic!("unexpected command shape: {other:?}"),
        }
    }
}
