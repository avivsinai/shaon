use anyhow::{bail, Context, Result};
use chrono::{Datelike, Duration, Local, NaiveDate};
use clap::{Args, CommandFactory, Parser, Subcommand};
use serde::Serialize;
use std::path::PathBuf;
use zeroize::Zeroize;

use hilan::{api, attendance, client, config, ontology, reports};

use attendance::is_time_pattern;
use client::HilanClient;
use config::Config;

// ---------------------------------------------------------------------------
// Overview command response types
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct OverviewResponse {
    user: UserInfo,
    month: String,
    summary: MonthSummary,
    attendance_types: Vec<ontology::AttendanceType>,
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

#[tokio::main]
async fn main() -> Result<()> {
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

    let config = match Config::load() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    };

    let subdomain = config.subdomain.clone();
    let mut client = HilanClient::new(config)?;
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
            client.login().await?;
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
                client.login().await?;
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
            let execute = write_mode.should_execute();
            let from_date = parse_date(&from)?;
            let to_date = parse_date(&to)?;
            if from_date > to_date {
                bail!("--from must be before or equal to --to");
            }

            if attendance_type.is_some() {
                client.login().await?;
            }
            let type_code =
                resolve_attendance_type_code(&mut client, &subdomain, attendance_type.as_deref())
                    .await?;
            let hours_range = hours.as_deref().map(parse_hours_range).transpose()?;

            if type_code.is_none() && hours_range.is_none() {
                bail!("fill requires at least one of --type or --hours");
            }

            let days = dates_inclusive(from_date, to_date);
            let mut json_outputs: Vec<serde_json::Value> = if json {
                Vec::with_capacity(days.len())
            } else {
                Vec::new()
            };
            for day in days {
                if !include_weekends
                    && matches!(day.weekday(), chrono::Weekday::Fri | chrono::Weekday::Sat)
                {
                    eprintln!("Skipping {} ({})", day, day.weekday());
                    continue;
                }
                let (entry_time, exit_time, clear_entry, clear_exit) = match &hours_range {
                    Some((entry, exit)) => (Some(entry.clone()), Some(exit.clone()), false, false),
                    None => (None, None, true, true),
                };

                let submit = attendance::AttendanceSubmit {
                    date: day,
                    attendance_type_code: type_code.clone(),
                    entry_time,
                    exit_time,
                    comment: None,
                    clear_entry,
                    clear_exit,
                    clear_comment: true,
                    default_work_day: type_code.is_none() && hours_range.is_some(),
                };
                let preview = attendance::submit_day(&mut client, &submit, execute).await?;
                if json {
                    json_outputs.push(serde_json::to_value(WriteOutput::new("fill", &preview))?);
                } else {
                    print_submit_preview("fill", &preview);
                }
            }
            if json {
                print_json(&json_outputs)?;
            }
        }
        Commands::Errors { month } => {
            client.login().await?;
            let month = parse_month_or_current(month.as_deref())?;
            let cal = attendance::read_calendar(&mut client, month).await?;
            if json {
                print_json(&cal)?;
            } else {
                attendance::print_errors(&cal);
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
            let execute = write_mode.should_execute();
            let date = parse_date(&day)?;
            if attendance_type.is_some() {
                client.login().await?;
            }
            let type_code =
                resolve_attendance_type_code(&mut client, &subdomain, attendance_type.as_deref())
                    .await?;
            let hours_range = hours.as_deref().map(parse_hours_range).transpose()?;

            if type_code.is_none() && hours_range.is_none() {
                bail!("fix requires at least one of --type or --hours");
            }

            let (entry_time, exit_time, clear_entry, clear_exit) = match hours_range {
                Some((entry, exit)) => (Some(entry), Some(exit), false, false),
                None => (None, None, false, false),
            };

            let submit = attendance::AttendanceSubmit {
                date,
                attendance_type_code: type_code,
                entry_time,
                exit_time,
                comment: None,
                clear_entry,
                clear_exit,
                clear_comment: false,
                default_work_day: false,
            };
            let preview =
                attendance::fix_error_day(&mut client, &submit, &report_id, &error_type, execute)
                    .await?;
            if json {
                print_json(&WriteOutput::new("fix", &preview))?;
            } else {
                print_submit_preview("fix", &preview);
            }
        }
        Commands::Status { month } => {
            client.login().await?;
            let month = parse_month_or_current(month.as_deref())?;
            let cal = attendance::read_calendar(&mut client, month).await?;
            if json {
                print_json(&cal)?;
            } else {
                attendance::print_calendar(&cal);
            }
        }
        Commands::Payslip { month, output } => {
            let month = parse_month_or_previous(month.as_deref())?;
            let download = client.payslip(month, output.as_deref()).await?;
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
            let summary = client.salary(months).await?;
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
            client.login().await?;
            let table = reports::fetch_report(&mut client, &name).await?;
            if json {
                print_json(&table)?;
            } else {
                reports::print_report(&table);
            }
        }
        Commands::Absences => {
            client.login().await?;
            let data = api::get_absences_initial(&mut client).await?;
            if json {
                print_json(&data)?;
            } else if data.symbols.is_empty() {
                println!("No absence symbols found.");
            } else {
                println!("{:<6}  {:<20}  Display", "ID", "Name");
                println!("{:-<6}  {:-<20}  {:-<30}", "", "", "");
                for sym in &data.symbols {
                    println!(
                        "{:<6}  {:<20}  {}",
                        sym.id,
                        sym.name,
                        sym.display_name.as_deref().unwrap_or(""),
                    );
                }
            }
        }
        Commands::Sheet => {
            client.login().await?;
            let table = reports::fetch_table_from_url(&mut client, reports::SHEET_URL_PATH).await?;
            if json {
                print_json(&table)?;
            } else {
                reports::print_report(&table);
            }
        }
        Commands::Corrections => {
            client.login().await?;
            let table =
                reports::fetch_table_from_url(&mut client, reports::CORRECTIONS_URL_PATH).await?;
            if json {
                print_json(&table)?;
            } else {
                reports::print_report(&table);
            }
        }
        Commands::Types => {
            let path = ontology::ontology_path(&subdomain);
            if path.exists() {
                let ont = ontology::OrgOntology::load(&path)?;
                if json {
                    print_json(&ont)?;
                } else {
                    ont.print_table();
                }
            } else {
                tracing::error!("No cached types. Run `hilan sync-types` or use any command with `--type` to auto-sync.");
            }
        }
        Commands::Overview { month, detailed } => {
            run_overview(&mut client, &subdomain, month.as_deref(), detailed, json).await?;
        }
        Commands::AutoFill {
            month,
            r#type,
            hours,
            include_weekends,
            max_days,
            write_mode,
        } => {
            let execute = write_mode.should_execute();
            let month_date = parse_month_or_current(month.as_deref())?;
            let hours_range = hours.as_deref().map(parse_hours_range).transpose()?;

            // Require --type or --hours — do NOT infer from most-common
            if r#type.is_none() && hours_range.is_none() {
                let ont_path = ontology::ontology_path(&subdomain);
                let mut msg = String::from("auto-fill requires --type or --hours.\n");
                if ont_path.exists() {
                    let ontology = ontology::OrgOntology::load(&ont_path)?;
                    msg.push_str("Available attendance types:\n");
                    for t in &ontology.types {
                        let en = t
                            .name_en
                            .as_deref()
                            .map(|s| format!(" ({s})"))
                            .unwrap_or_default();
                        msg.push_str(&format!("  {} -- {}{}\n", t.code, t.name_he, en));
                    }
                } else {
                    msg.push_str(
                        "Run `hilan sync-types` to see available types, or pass --type <code>.",
                    );
                }
                bail!("{msg}");
            }

            client.login().await?;

            // Auto-sync ontology if cache is missing and --type is given
            if r#type.is_some() && !ontology::ontology_path(&subdomain).exists() {
                eprintln!("Ontology cache missing -- syncing attendance types...");
                ontology::sync_from_calendar(&mut client, &subdomain).await?;
            }

            let cal = attendance::read_calendar(&mut client, month_date).await?;
            let (type_code, type_display) =
                attendance::resolve_auto_fill_type(&subdomain, r#type.as_deref())?;

            let result = attendance::auto_fill(
                &mut client,
                &cal,
                attendance::AutoFillOpts {
                    type_code,
                    type_display: &type_display,
                    hours: hours_range.as_ref(),
                    include_weekends,
                    execute,
                    max_days,
                },
            )
            .await?;

            if json {
                print_json(&result)?;
            } else {
                attendance::print_auto_fill(&result);
            }
        }
        Commands::Serve => unreachable!("handled above"),
        Commands::Completions { .. } => unreachable!("handled above"),
    }

    Ok(())
}

async fn run_mcp_server() -> Result<()> {
    use rmcp::ServiceExt;
    let server = hilan::mcp::HilanMcpServer::new();
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

fn dates_inclusive(from: NaiveDate, to: NaiveDate) -> Vec<NaiveDate> {
    let mut dates = Vec::new();
    let mut current = from;
    while current <= to {
        dates.push(current);
        current += Duration::days(1);
    }
    dates
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
    client: &mut HilanClient,
    subdomain: &str,
    month_arg: Option<&str>,
    verbose: bool,
    json: bool,
) -> Result<()> {
    client.login().await?;
    let bootstrap = api::bootstrap(client).await?;
    let month = parse_month_or_current(month_arg)?;
    let cal = attendance::read_calendar(client, month).await?;
    let attendance_types = load_or_sync_ontology(client, subdomain).await?;

    let today = Local::now().date_naive();
    let is_current_month = month.year() == today.year() && month.month() == today.month();
    let is_past = |d: &attendance::CalendarDay| !(is_current_month && d.date > today);

    let error_days: Vec<&attendance::CalendarDay> =
        cal.days.iter().filter(|d| d.has_error).collect();

    let reported_count = cal.days.iter().filter(|d| d.is_reported()).count() as u32;

    let missing_days: Vec<&attendance::CalendarDay> = cal
        .days
        .iter()
        .filter(|d| d.is_work_day() && !d.is_reported() && !d.has_error && is_past(d))
        .collect();

    let total_work_days = cal
        .days
        .iter()
        .filter(|d| d.is_work_day() && is_past(d))
        .count() as u32;

    let month_str = month.format("%Y-%m").to_string();

    let error_day_details: Vec<ErrorDay> = error_days
        .iter()
        .map(|d| ErrorDay {
            date: d.date.format("%Y-%m-%d").to_string(),
            day_name: d.day_name.clone(),
            error_message: d
                .error_message
                .clone()
                .unwrap_or_else(|| "missing report".to_string()),
        })
        .collect();

    let missing_day_strings: Vec<String> = missing_days
        .iter()
        .map(|d| d.date.format("%Y-%m-%d").to_string())
        .collect();

    let mut suggested_actions = Vec::new();

    if !error_day_details.is_empty() {
        let count = error_day_details.len();
        suggested_actions.push(SuggestedAction {
            kind: "fix_errors".to_string(),
            reason: format!("{count} day(s) have attendance errors"),
            params: serde_json::json!({
                "month": month_str,
                "count": count,
            }),
            safety: "dry_run_default".to_string(),
        });
    }

    if !missing_day_strings.is_empty() {
        let count = missing_day_strings.len();
        let first = &missing_day_strings[0];
        let last = missing_day_strings.last().unwrap();
        suggested_actions.push(SuggestedAction {
            kind: "fill_missing".to_string(),
            reason: format!("{count} work day(s) have no attendance report"),
            params: serde_json::json!({
                "from": first,
                "to": last,
                "count": count,
            }),
            safety: "dry_run_default".to_string(),
        });
    }

    let response = OverviewResponse {
        user: UserInfo {
            user_id: bootstrap.user_id,
            employee_id: bootstrap.employee_id.to_string(),
            name: bootstrap.name,
            is_manager: bootstrap.is_manager,
        },
        month: month_str,
        summary: MonthSummary {
            total_work_days,
            reported: reported_count,
            missing: missing_days.len() as u32,
            errors: error_days.len() as u32,
        },
        attendance_types,
        error_days: error_day_details,
        missing_days: missing_day_strings,
        suggested_actions,
    };

    if json {
        if verbose {
            // Include per-day data only behind --verbose
            let mut value = serde_json::to_value(&response)?;
            value["days"] = serde_json::to_value(&cal.days)?;
            println!("{}", serde_json::to_string_pretty(&value)?);
        } else {
            print_json(&response)?;
        }
    } else {
        print_overview_human(&response);
        if verbose {
            println!();
            attendance::print_calendar(&cal);
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
            println!("  {} ({}) -- {}", ed.date, ed.day_name, ed.error_message);
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

async fn load_or_sync_ontology(
    client: &mut HilanClient,
    subdomain: &str,
) -> Result<Vec<ontology::AttendanceType>> {
    // Use load_or_sync which respects 24h TTL cache freshness
    match ontology::OrgOntology::load_or_sync(client, subdomain).await {
        Ok(ont) => Ok(ont.types),
        Err(_) => {
            // Non-fatal: overview/auto-fill are still useful without types
            Ok(Vec::new())
        }
    }
}
