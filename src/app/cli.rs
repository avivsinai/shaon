use crate::core::{
    AttendanceProvider, AttendanceType as CoreAttendanceType, FixTarget as CoreFixTarget,
    PayslipProvider, ProviderError, ReportProvider, ReportSpec, SalaryProvider,
    WriteMode as CoreWriteMode, WritePreview as CoreWritePreview,
};
use anyhow::{bail, Context, Result};
use chrono::{Datelike, Local, NaiveDate};
use clap::{Args, CommandFactory, Parser, Subcommand};
use serde::Serialize;
use std::path::PathBuf;
use zeroize::Zeroize;

use super::{build_provider, load_config};
use crate::{attendance, client, ontology, provider::HilanProvider, use_cases};

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
    missing_days: Vec<String>,
    suggested_actions: Vec<SuggestedAction>,
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
}

#[derive(Serialize)]
struct ErrorFixParams {
    report_id: String,
    error_type: String,
}

/// Structured suggested action — NOT a shell string.
/// Human output can render commands, but the JSON contract is structured.
#[derive(Serialize)]
struct SuggestedAction {
    kind: String,
    reason: String,
    params: serde_json::Value,
    safety: String,
}

const SHEET_REPORT_PATH: &str = "/Hilannetv2/Attendance/HoursAnalysis.aspx";
const CORRECTIONS_REPORT_PATH: &str = "/Hilannetv2/Attendance/HoursReportLog.aspx";

#[derive(Parser)]
#[command(
    name = "hilan",
    version,
    long_version = env!("HILAN_LONG_VERSION"),
    about = "Hilan attendance & payslip CLI"
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
            url: preview_debug_field(preview, "url"),
            button_name: preview_debug_field(preview, "button_name"),
            button_value: preview_debug_field(preview, "button_value"),
            employee_id: preview_debug_field(preview, "employee_id"),
            payload_display: preview_debug_field(preview, "payload_display"),
        }
    }
}

#[derive(Subcommand)]
enum Commands {
    /// Authenticate with Hilan (test credentials or manage keychain)
    Auth {
        /// Migrate plaintext password from config file to OS keychain
        #[arg(long)]
        migrate: bool,
    },

    /// Sync attendance-type ontology from Hilan (optional — types auto-sync on first use)
    SyncTypes,

    /// Clock in for today
    ClockIn {
        /// Attendance type override
        #[arg(long = "type")]
        attendance_type: Option<String>,

        #[command(flatten)]
        write_mode: WriteMode,
    },

    /// Clock out for today
    ClockOut {
        #[command(flatten)]
        write_mode: WriteMode,
    },

    /// Fill attendance for a date range
    Fill {
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
    },

    /// Show attendance errors for a month
    Errors {
        /// Month in YYYY-MM format (defaults to current)
        #[arg(long)]
        month: Option<String>,
    },

    /// Fix a single day's attendance
    Fix {
        /// Day to fix (YYYY-MM-DD)
        day: String,
        /// Attendance type override
        #[arg(long = "type")]
        attendance_type: Option<String>,
        /// Fixed hours (e.g. "09:00-18:00")
        #[arg(long)]
        hours: Option<String>,

        /// Error-wizard report ID. Defaults to the sampled missing-standard-day flow.
        #[arg(long, default_value = "00000000-0000-0000-0000-000000000000")]
        report_id: String,

        /// Error-wizard type. Defaults to the sampled missing-standard-day flow.
        #[arg(long, default_value = "63")]
        error_type: String,

        #[command(flatten)]
        write_mode: WriteMode,
    },

    /// Show attendance status for a month
    Status {
        /// Month in YYYY-MM format (defaults to current)
        #[arg(long)]
        month: Option<String>,
    },

    /// Download payslip PDF
    Payslip {
        /// Month in YYYY-MM format (defaults to previous month)
        #[arg(long)]
        month: Option<String>,
        /// Output file path
        #[arg(long)]
        output: Option<PathBuf>,
    },

    /// Show salary summary for recent months
    Salary {
        /// Number of months to show (default: 2)
        #[arg(long, default_value = "2")]
        months: u32,
    },

    /// Fetch a named Hilan report
    Report {
        /// Report name (e.g. ErrorsReportNEW, MissingReportNEW)
        name: String,
    },

    /// Show absences initial data (symbols and display names)
    Absences,

    /// Show the analyzed attendance sheet
    Sheet,

    /// Show the attendance correction log
    Corrections,

    /// List available attendance types (from cache or server)
    Types,

    /// Get overview for a month: identity, summary, errors, missing days, suggested actions
    Overview {
        /// Month in YYYY-MM format (defaults to current month)
        #[arg(long)]
        month: Option<String>,

        /// Include full per-day calendar data in output
        #[arg(long)]
        detailed: bool,
    },

    /// Automatically fill all missing days in a month (dry-run by default)
    AutoFill {
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
    if matches!(cli.command, Commands::Serve) {
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
    if let Commands::Completions { shell } = cli.command {
        let mut cmd = Cli::command();
        clap_complete::generate(shell, &mut cmd, "hilan", &mut std::io::stdout());
        return Ok(());
    }

    let config = match load_config() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    };

    let subdomain = config.subdomain.clone();
    let mut client = build_provider(config)?.into_inner();
    let json = cli.json;

    match cli.command {
        Commands::Auth { migrate } => {
            if migrate {
                // Migrate plaintext password to keychain
                let config = client.config_mut();
                config.migrate_to_keychain()?;
            } else {
                // Interactive: prompt for password if not already in keychain
                match client.config().get_password() {
                    Ok(_) => {
                        eprintln!("Password already available. Testing login...");
                    }
                    Err(_) => {
                        let mut password = rpassword::prompt_password(
                            "Enter your Hilan password (input is hidden): ",
                        )
                        .context("read password from terminal")?;
                        client.config().store_password_in_keychain(&password)?;
                        password.zeroize();
                        eprintln!("Password stored in OS keychain.");
                    }
                }
                client.login().await?;
                if json {
                    print_json(&serde_json::json!({"status": "ok"}))?;
                }
            }
        }
        Commands::SyncTypes => {
            client.ensure_authenticated().await?;
            let ont = ontology::sync_from_calendar(&mut client, &subdomain).await?;
            if json {
                print_json(&ont)?;
            } else {
                ont.print_table();
            }
        }
        Commands::ClockIn {
            attendance_type,
            write_mode,
        } => {
            let execute = write_mode.should_execute();
            if attendance_type.is_some() {
                client.ensure_authenticated().await?;
            }
            let type_code =
                resolve_attendance_type_code(&mut client, &subdomain, attendance_type.as_deref())
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
                default_work_day: attendance_type.is_none(),
            };
            let preview = attendance::submit_day(&mut client, &submit, execute).await?;
            if json {
                print_json(&WriteOutput::new("clock-in", &preview))?;
            } else {
                print_submit_preview("clock-in", &preview);
            }
        }
        Commands::ClockOut { write_mode } => {
            let execute = write_mode.should_execute();
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
                print_json(&WriteOutput::new("clock-out", &preview))?;
            } else {
                print_submit_preview("clock-out", &preview);
            }
        }
        Commands::Fill {
            from,
            to,
            attendance_type,
            hours,
            include_weekends,
            write_mode,
        } => {
            let from_date = parse_date(&from)?;
            let to_date = parse_date(&to)?;
            let hours_range = hours.as_deref().map(parse_hours_range).transpose()?;
            let mut provider = HilanProvider::from_client(client);
            let type_code =
                use_cases::resolve_attendance_type(&mut provider, attendance_type.as_deref())
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
                    include_weekends,
                    mode: write_mode.provider_mode(),
                },
            )
            .await
            .map_err(provider_error)?;

            if json {
                let outputs: Vec<ProviderWriteOutput<'_>> = previews
                    .iter()
                    .map(|preview| ProviderWriteOutput::new("fill", preview))
                    .collect();
                print_json(&outputs)?;
            } else {
                for preview in &previews {
                    print_provider_preview("fill", preview);
                }
            }
        }
        Commands::Errors { month } => {
            let month = parse_month_or_current(month.as_deref())?;
            let mut provider = HilanProvider::from_client(client);
            let cal = provider
                .month_calendar(month)
                .await
                .map_err(provider_error)?;
            if json {
                print_json(&cal)?;
            } else {
                use_cases::print_error_days(&cal);
            }
        }
        Commands::Fix {
            day,
            attendance_type,
            hours,
            report_id,
            error_type,
            write_mode,
        } => {
            let date = parse_date(&day)?;
            let hours_range = hours.as_deref().map(parse_hours_range).transpose()?;
            let mut provider = HilanProvider::from_client(client);
            let type_code =
                use_cases::resolve_attendance_type(&mut provider, attendance_type.as_deref())
                    .await
                    .map_err(provider_error)?
                    .map(|resolved| resolved.code);
            let target = CoreFixTarget {
                date,
                issue_kind: None,
                provider_ref: format!("{report_id}:{error_type}"),
                metadata: std::collections::BTreeMap::from([
                    ("report_id".to_string(), report_id),
                    ("error_type".to_string(), error_type),
                ]),
            };
            let preview = use_cases::fix_day(
                &mut provider,
                &target,
                type_code,
                hours_range,
                write_mode.provider_mode(),
            )
            .await
            .map_err(provider_error)?;
            if json {
                print_json(&ProviderWriteOutput::new("fix", &preview))?;
            } else {
                print_provider_preview("fix", &preview);
            }
        }
        Commands::Status { month } => {
            let month = parse_month_or_current(month.as_deref())?;
            let mut provider = HilanProvider::from_client(client);
            let cal = provider
                .month_calendar(month)
                .await
                .map_err(provider_error)?;
            if json {
                print_json(&cal)?;
            } else {
                use_cases::print_calendar(&cal);
            }
        }
        Commands::Payslip { month, output } => {
            let month = parse_month_or_previous(month.as_deref())?;
            let mut provider = HilanProvider::from_client(client);
            let download = provider
                .download_payslip(month, output.as_deref())
                .await
                .map_err(provider_error)?;
            if json {
                print_json(&download)?;
            } else {
                println!(
                    "Saved payslip for {} to {} ({} bytes)",
                    download.month.format("%Y-%m"),
                    download.path.display(),
                    download.size_bytes
                );
            }
        }
        Commands::Salary { months } => {
            let mut provider = HilanProvider::from_client(client);
            let summary = provider
                .salary_summary(months)
                .await
                .map_err(provider_error)?;
            if json {
                print_json(&summary)?;
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
        Commands::Report { name } => {
            let mut provider = HilanProvider::from_client(client);
            let table = provider
                .report(ReportSpec::Named(name))
                .await
                .map_err(provider_error)?;
            if json {
                print_json(&table)?;
            } else {
                use_cases::print_report_table(&table);
            }
        }
        Commands::Absences => {
            let mut provider = HilanProvider::from_client(client);
            let symbols = use_cases::load_absence_symbols(&mut provider)
                .await
                .map_err(provider_error)?;
            if json {
                print_json(&symbols)?;
            } else {
                use_cases::print_absence_symbols(&symbols);
            }
        }
        Commands::Sheet => {
            let mut provider = HilanProvider::from_client(client);
            let table = provider
                .report(ReportSpec::Path(SHEET_REPORT_PATH.to_string()))
                .await
                .map_err(provider_error)?;
            if json {
                print_json(&table)?;
            } else {
                use_cases::print_report_table(&table);
            }
        }
        Commands::Corrections => {
            let mut provider = HilanProvider::from_client(client);
            let table = provider
                .report(ReportSpec::Path(CORRECTIONS_REPORT_PATH.to_string()))
                .await
                .map_err(provider_error)?;
            if json {
                print_json(&table)?;
            } else {
                use_cases::print_report_table(&table);
            }
        }
        Commands::Types => {
            let mut provider = HilanProvider::from_client(client);
            let types = provider.attendance_types().await.map_err(provider_error)?;
            if json {
                print_json(&types)?;
            } else {
                use_cases::print_attendance_types(&types);
            }
        }
        Commands::Overview { month, detailed } => {
            let mut provider = HilanProvider::from_client(client);
            run_overview(&mut provider, month.as_deref(), detailed, json).await?;
        }
        Commands::AutoFill {
            month,
            r#type,
            hours,
            include_weekends,
            max_days,
            write_mode,
        } => {
            let month_date = parse_month_or_current(month.as_deref())?;
            let hours_range = hours.as_deref().map(parse_hours_range).transpose()?;
            let mut provider = HilanProvider::from_client(client);

            if r#type.is_none() && hours_range.is_none() {
                let mut msg = String::from("auto-fill requires --type or --hours.\n");
                match use_cases::describe_attendance_types(&mut provider).await {
                    Ok(help) => msg.push_str(&help),
                    Err(_) => {
                        msg.push_str(
                            "Use `hilan types` to inspect available types, or pass --type <code>.",
                        );
                    }
                }
                bail!("{msg}");
            }

            let resolved_type =
                use_cases::resolve_attendance_type(&mut provider, r#type.as_deref())
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
                    include_weekends,
                    mode: write_mode.provider_mode(),
                    max_days,
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
        Commands::Serve => unreachable!("handled above"),
        Commands::Completions { .. } => unreachable!("handled above"),
    }

    Ok(())
}

async fn run_mcp_server() -> Result<()> {
    use rmcp::ServiceExt;
    let server = crate::mcp::HilanMcpServer::new();
    let transport = rmcp::transport::io::stdio();
    server.serve(transport).await?.waiting().await?;
    Ok(())
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
    if let Some(url) = preview_debug_field(preview, "url") {
        println!("Target URL: {url}");
    }
    if let Some(employee_id) = preview_debug_field(preview, "employee_id") {
        println!("Employee ID: {employee_id}");
    }
    if let (Some(button_name), Some(button_value)) = (
        preview_debug_field(preview, "button_name"),
        preview_debug_field(preview, "button_value"),
    ) {
        println!("Button: {button_name} = {button_value}");
    }
    if let Some(payload_display) = preview_debug_field(preview, "payload_display") {
        println!("{payload_display}");
    } else {
        println!("{}", preview.summary);
    }
}

fn preview_debug_field<'a>(preview: &'a CoreWritePreview, key: &str) -> Option<&'a str> {
    preview
        .provider_debug
        .as_ref()
        .and_then(|debug| debug.get(key))
        .and_then(serde_json::Value::as_str)
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
    verbose: bool,
    json: bool,
) -> Result<()> {
    let month = parse_month_or_current(month_arg)?;
    let today = Local::now().date_naive();
    let overview = use_cases::build_overview(provider, month, today)
        .await
        .map_err(provider_error)?;
    let month_str = overview.month.format("%Y-%m").to_string();

    let error_day_details: Vec<ErrorDay> = overview
        .error_days
        .iter()
        .map(|entry| ErrorDay {
            date: entry.day.date.format("%Y-%m-%d").to_string(),
            day_name: entry.day.day_name.clone(),
            error_message: entry
                .day
                .error_message
                .clone()
                .unwrap_or_else(|| "missing report".to_string()),
            fix_params: entry
                .fix_target
                .as_ref()
                .and_then(error_fix_params_from_target),
        })
        .collect();

    let missing_day_strings: Vec<String> = overview
        .missing_days
        .iter()
        .map(|date| date.format("%Y-%m-%d").to_string())
        .collect();

    let suggested_actions: Vec<SuggestedAction> = overview
        .suggested_actions
        .iter()
        .map(|action| match action {
            use_cases::SuggestedActionPlan::FixErrors {
                month,
                count,
                fixable_targets,
            } => SuggestedAction {
                kind: "fix_errors".to_string(),
                reason: format!("{count} day(s) have attendance errors"),
                params: serde_json::json!({
                    "month": month.format("%Y-%m").to_string(),
                    "count": count,
                    "fixable_days": fixable_targets
                        .iter()
                        .filter_map(|target| {
                            error_fix_params_from_target(target).map(|params| {
                                serde_json::json!({
                                    "date": target.date.format("%Y-%m-%d").to_string(),
                                    "report_id": params.report_id,
                                    "error_type": params.error_type,
                                })
                            })
                        })
                        .collect::<Vec<_>>(),
                }),
                safety: "dry_run_default".to_string(),
            },
            use_cases::SuggestedActionPlan::FillMissing { from, to, count } => SuggestedAction {
                kind: "fill_missing".to_string(),
                reason: format!("{count} work day(s) have no attendance report"),
                params: serde_json::json!({
                    "from": from.format("%Y-%m-%d").to_string(),
                    "to": to.format("%Y-%m-%d").to_string(),
                    "count": count,
                }),
                safety: "dry_run_default".to_string(),
            },
        })
        .collect();

    let response = OverviewResponse {
        user: UserInfo {
            user_id: overview.identity.user_id.clone(),
            employee_id: overview.identity.employee_id.clone(),
            name: overview.identity.display_name.clone(),
            is_manager: overview.identity.is_manager,
        },
        month: month_str,
        summary: MonthSummary {
            total_work_days: overview.summary.total_work_days,
            reported: overview.summary.reported,
            missing: overview.summary.missing,
            errors: overview.summary.errors,
        },
        attendance_types: overview.attendance_types.clone(),
        error_days: error_day_details,
        missing_days: missing_day_strings,
        suggested_actions,
    };

    if json {
        if verbose {
            // Include per-day data only behind --verbose
            let mut value = serde_json::to_value(&response)?;
            value["days"] = serde_json::to_value(&overview.calendar.days)?;
            println!("{}", serde_json::to_string_pretty(&value)?);
        } else {
            print_json(&response)?;
        }
    } else {
        print_overview_human(&response);
        if verbose {
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
        for date_str in &ctx.missing_days {
            if let Ok(date) = NaiveDate::parse_from_str(date_str, "%Y-%m-%d") {
                println!("  {} ({})", date_str, date.format("%a"));
            } else {
                println!("  {}", date_str);
            }
        }
    }

    if !ctx.error_days.is_empty() {
        println!();
        println!("Error days:");
        for ed in &ctx.error_days {
            match &ed.fix_params {
                Some(params) => println!(
                    "  {} ({}) -- {} [reportId={}, errorType={}]",
                    ed.date, ed.day_name, ed.error_message, params.report_id, params.error_type
                ),
                None => println!("  {} ({}) -- {}", ed.date, ed.day_name, ed.error_message),
            }
        }
    }

    if !ctx.suggested_actions.is_empty() {
        println!();
        println!("Suggested actions:");
        for action in &ctx.suggested_actions {
            let cmd_hint = match action.kind.as_str() {
                "fix_errors" => {
                    let m = action.params["month"].as_str().unwrap_or("?");
                    format!("hilan errors --month {m}")
                }
                "fill_missing" => {
                    let from = action.params["from"].as_str().unwrap_or("?");
                    let to = action.params["to"].as_str().unwrap_or("?");
                    format!(
                        "hilan fill --from {from} --to {to} --type <type> --hours <HH:MM-HH:MM>"
                    )
                }
                _ => format!("{}: {}", action.kind, action.reason),
            };
            println!("  [{}] {} -- {}", action.kind, action.reason, cmd_hint);
        }
    }
}

fn error_fix_params_from_target(target: &crate::core::FixTarget) -> Option<ErrorFixParams> {
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

fn print_calendar_verbose(calendar: &crate::core::MonthCalendar) {
    println!(
        "Calendar {} (employee {})",
        calendar.month.format("%Y-%m"),
        calendar.employee_id
    );
    println!("Date        Day    Entry  Exit   Type             Hours  Error");
    println!("----------  -----  -----  -----  ---------------  -----  -----");

    for day in &calendar.days {
        println!(
            "{:<10}  {:<5}  {:<5}  {:<5}  {:<15}  {:<5}  {}",
            day.date.format("%Y-%m-%d"),
            day.day_name,
            day.entry_time.as_deref().unwrap_or(""),
            day.exit_time.as_deref().unwrap_or(""),
            day.attendance_type.as_deref().unwrap_or(""),
            day.total_hours.as_deref().unwrap_or(""),
            if day.has_error { "yes" } else { "" },
        );
    }
}

fn provider_error(err: ProviderError) -> anyhow::Error {
    anyhow::anyhow!("{err}")
}
