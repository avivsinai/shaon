use anyhow::{bail, Result};
use chrono::{Datelike, Duration, Local, NaiveDate};
use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;

use hilan::{api, attendance, client, config, ontology, reports};

use client::HilanClient;
use config::Config;

#[derive(Parser)]
#[command(name = "hilan", version, about = "Hilan attendance & payslip CLI")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Args, Debug, Clone)]
struct WriteMode {
    /// Preview the payload without sending it (default behavior)
    #[arg(long)]
    dry_run: bool,

    /// Actually submit the write request to Hilan
    #[arg(long)]
    execute: bool,
}

impl WriteMode {
    fn should_execute(&self) -> Result<bool> {
        if self.execute && self.dry_run {
            bail!("Use either --execute or --dry-run, not both");
        }
        Ok(self.execute)
    }
}

#[derive(Subcommand)]
enum Commands {
    /// Authenticate with Hilan (test credentials)
    Auth,

    /// Sync attendance-type ontology from Hilan
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

    /// List available attendance types (from local cache)
    Types,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    let config = match Config::load() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    };

    let subdomain = config.subdomain.clone();
    let mut client = HilanClient::new(config)?;

    match cli.command {
        Commands::Auth => {
            client.login().await?;
        }
        Commands::SyncTypes => {
            client.login().await?;
            let ont = ontology::sync_from_calendar(&client, &subdomain).await?;
            ont.print_table();
        }
        Commands::ClockIn {
            attendance_type,
            write_mode,
        } => {
            let execute = write_mode.should_execute()?;
            let submit = attendance::AttendanceSubmit {
                date: Local::now().date_naive(),
                attendance_type_code: resolve_attendance_type_code(
                    &subdomain,
                    attendance_type.as_deref(),
                )?,
                entry_time: Some(current_local_time_hhmm()),
                exit_time: None,
                comment: None,
                clear_entry: false,
                clear_exit: true,
                clear_comment: true,
                default_work_day: attendance_type.is_none(),
            };
            let preview = attendance::submit_day(&mut client, &submit, execute).await?;
            print_submit_preview("clock-in", &preview);
        }
        Commands::ClockOut { write_mode } => {
            let execute = write_mode.should_execute()?;
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
            print_submit_preview("clock-out", &preview);
        }
        Commands::Fill {
            from,
            to,
            attendance_type,
            hours,
            write_mode,
        } => {
            let execute = write_mode.should_execute()?;
            let from_date = parse_date(&from)?;
            let to_date = parse_date(&to)?;
            if from_date > to_date {
                bail!("--from must be before or equal to --to");
            }

            let type_code = resolve_attendance_type_code(&subdomain, attendance_type.as_deref())?;
            let hours_range = hours.as_deref().map(parse_hours_range).transpose()?;

            if type_code.is_none() && hours_range.is_none() {
                bail!("fill requires at least one of --type or --hours");
            }

            for day in dates_inclusive(from_date, to_date) {
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
                print_submit_preview("fill", &preview);
            }
        }
        Commands::Errors { month } => {
            client.login().await?;
            let month = parse_month_or_current(month.as_deref())?;
            let cal = attendance::read_calendar(&client, month).await?;
            attendance::print_errors(&cal);
        }
        Commands::Fix {
            day,
            attendance_type,
            hours,
            report_id,
            error_type,
            write_mode,
        } => {
            let execute = write_mode.should_execute()?;
            let date = parse_date(&day)?;
            let type_code = resolve_attendance_type_code(&subdomain, attendance_type.as_deref())?;
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
            print_submit_preview("fix", &preview);
        }
        Commands::Status { month } => {
            client.login().await?;
            let month = parse_month_or_current(month.as_deref())?;
            let cal = attendance::read_calendar(&client, month).await?;
            attendance::print_calendar(&cal);
        }
        Commands::Payslip { month, output } => {
            let month = parse_month_or_previous(month.as_deref())?;
            let download = client.payslip(month, output.as_deref()).await?;
            println!(
                "Saved payslip for {} to {} ({} bytes)",
                download.month.format("%Y-%m"),
                download.path.display(),
                download.size_bytes
            );
        }
        Commands::Salary { months } => {
            let summary = client.salary(months).await?;
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
                println!("Change over latest month: {:.2}%", percent_diff);
            }
        }
        Commands::Report { name } => {
            client.login().await?;
            let table = reports::fetch_report(&client, &name).await?;
            reports::print_report(&table);
        }
        Commands::Absences => {
            client.login().await?;
            let data = api::get_absences_initial(&client).await?;
            if data.symbols.is_empty() {
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
            let table = reports::fetch_table_from_url(&client, reports::SHEET_URL_PATH).await?;
            reports::print_report(&table);
        }
        Commands::Corrections => {
            client.login().await?;
            let table =
                reports::fetch_table_from_url(&client, reports::CORRECTIONS_URL_PATH).await?;
            reports::print_report(&table);
        }
        Commands::Types => {
            let path = ontology::ontology_path(&subdomain);
            if path.exists() {
                let ont = ontology::OrgOntology::load(&path)?;
                ont.print_table();
            } else {
                eprintln!("No cached types. Run `hilan sync-types` first.");
                std::process::exit(1);
            }
        }
    }

    Ok(())
}

fn parse_month_or_previous(month: Option<&str>) -> Result<NaiveDate> {
    match month {
        Some(value) => parse_month(value),
        None => Ok(previous_month_start()),
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

    if !is_hhmm(entry) || !is_hhmm(exit) {
        bail!("hours must be in HH:MM-HH:MM format");
    }

    Ok((entry.to_string(), exit.to_string()))
}

fn is_hhmm(value: &str) -> bool {
    let parts: Vec<&str> = value.split(':').collect();
    if parts.len() != 2 {
        return false;
    }

    let hour: u32 = match parts[0].parse() {
        Ok(v) => v,
        Err(_) => return false,
    };
    let minute: u32 = match parts[1].parse() {
        Ok(v) => v,
        Err(_) => return false,
    };

    hour < 24 && minute < 60
}

fn resolve_attendance_type_code(
    subdomain: &str,
    requested: Option<&str>,
) -> Result<Option<String>> {
    let Some(requested) = requested else {
        return Ok(None);
    };

    let path = ontology::ontology_path(subdomain);
    if path.exists() {
        let ontology = ontology::OrgOntology::load(&path)?;
        return Ok(Some(ontology.validate_type(requested)?.code.clone()));
    }

    if requested.chars().all(|ch| ch.is_ascii_digit()) {
        return Ok(Some(requested.to_string()));
    }

    bail!(
        "Attendance type '{}' needs cached ontology. Run `hilan sync-types` first or pass a numeric type code.",
        requested
    );
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

fn previous_month_start() -> NaiveDate {
    let today = Local::now().date_naive();
    let this_month = today.with_day(1).unwrap();
    let total_months = this_month.year() * 12 + this_month.month0() as i32 - 1;
    let year = total_months.div_euclid(12);
    let month0 = total_months.rem_euclid(12) as u32;
    NaiveDate::from_ymd_opt(year, month0 + 1, 1).unwrap()
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
