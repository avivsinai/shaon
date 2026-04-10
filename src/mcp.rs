use rmcp::{
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{ServerCapabilities, ServerInfo},
    schemars, tool, tool_handler, tool_router, ServerHandler,
};
use serde::Deserialize;

use chrono::Datelike;
use std::collections::BTreeMap;

use crate::api;
use crate::attendance;
use crate::client::HilanClient;
use crate::config::Config;
use crate::ontology;
use crate::reports;

// ---------------------------------------------------------------------------
// Request schemas
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct MonthParam {
    #[schemars(description = "Month in YYYY-MM format")]
    pub month: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ClockParam {
    #[schemars(description = "Time in HH:MM format")]
    pub time: String,
    #[schemars(description = "Set to true to actually submit (default: false/preview)")]
    pub execute: Option<bool>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct FillParam {
    #[schemars(description = "Start date YYYY-MM-DD")]
    pub from: String,
    #[schemars(description = "End date YYYY-MM-DD")]
    pub to: String,
    #[schemars(description = "Entry time HH:MM")]
    pub entry: String,
    #[schemars(description = "Exit time HH:MM")]
    pub exit: String,
    #[schemars(description = "Attendance type (e.g. 'regular', 'work from home')")]
    pub r#type: Option<String>,
    #[schemars(description = "Set to true to actually submit (default: false/preview)")]
    pub execute: Option<bool>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct AutoFillParam {
    #[schemars(description = "Month in YYYY-MM format (default: current month)")]
    pub month: Option<String>,
    #[schemars(description = "Attendance type (required unless hours is provided)")]
    pub r#type: Option<String>,
    #[schemars(description = "Hours range in HH:MM-HH:MM format (e.g. '09:00-18:00')")]
    pub hours: Option<String>,
    #[schemars(description = "Include weekends (Fri/Sat) -- skipped by default")]
    pub include_weekends: Option<bool>,
    #[schemars(description = "Safety cap: max days to fill (default: 10)")]
    pub max_days: Option<u32>,
    #[schemars(description = "Set to true to actually submit (default: false/preview)")]
    pub execute: Option<bool>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SalaryParam {
    #[schemars(description = "Number of months to show (default: 3)")]
    pub months: Option<u32>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct OverviewParam {
    #[schemars(description = "Month in YYYY-MM format (default: current month)")]
    pub month: Option<String>,
}

// ---------------------------------------------------------------------------
// MCP Server
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct HilanMcpServer {
    tool_router: ToolRouter<Self>,
}

impl Default for HilanMcpServer {
    fn default() -> Self {
        Self::new()
    }
}

impl HilanMcpServer {
    pub fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
        }
    }
}

/// Create a fresh, logged-in HilanClient. Each tool call gets its own client
/// because HilanClient requires `&mut self` for login and most operations,
/// and we want no shared mutable state between tool invocations.
async fn new_client() -> Result<HilanClient, String> {
    let config = Config::load().map_err(|e| format!("config error: {e}"))?;
    let mut client = HilanClient::new(config).map_err(|e| format!("client error: {e}"))?;
    client
        .ensure_authenticated()
        .await
        .map_err(|e| format!("auth error: {e}"))?;
    Ok(client)
}

/// Convert a fallible async block result into a JSON string, wrapping errors
/// using serde_json::json!() to ensure valid JSON (no format!-based escaping).
async fn json_or_error<F, Fut>(f: F) -> String
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<serde_json::Value, String>>,
{
    match f().await {
        Ok(val) => serde_json::to_string_pretty(&val).unwrap_or_else(|e| {
            serde_json::to_string(
                &serde_json::json!({"error": format!("serialization failed: {e}")}),
            )
            .unwrap_or_default()
        }),
        Err(e) => serde_json::to_string(&serde_json::json!({"error": e})).unwrap_or_default(),
    }
}

fn parse_month(value: &str) -> Result<chrono::NaiveDate, String> {
    chrono::NaiveDate::parse_from_str(&format!("{value}-01"), "%Y-%m-%d")
        .map_err(|e| format!("invalid month '{value}': {e}"))
}

fn parse_month_or_current(value: Option<&str>) -> Result<chrono::NaiveDate, String> {
    match value {
        Some(v) => parse_month(v),
        None => Ok(chrono::Local::now()
            .date_naive()
            .with_day(1)
            .ok_or_else(|| "failed to get current month start".to_string())?),
    }
}

fn parse_date(value: &str) -> Result<chrono::NaiveDate, String> {
    chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .map_err(|e| format!("invalid date '{value}': {e}"))
}

fn parse_hours_range(value: &str) -> Result<(String, String), String> {
    let (entry, exit) = value
        .split_once('-')
        .ok_or_else(|| "hours must be in HH:MM-HH:MM format".to_string())?;
    Ok((entry.to_string(), exit.to_string()))
}

// ---------------------------------------------------------------------------
// Tool implementations
//
// EXPOSED (read-only):
//   hilan_status, hilan_errors, hilan_types, hilan_salary,
//   hilan_sheet, hilan_corrections, hilan_absences, hilan_overview
//
// EXPOSED WITH CAUTION (write, dry-run default):
//   hilan_clock_in, hilan_clock_out, hilan_fill, hilan_auto_fill
//
// SKIPPED (per Codex design review):
//   hilan_payslip (binary PDF), hilan_fix (brittle params),
//   hilan_report (generic, unstable schema), hilan_auth (implicit)
// ---------------------------------------------------------------------------

#[tool_router]
impl HilanMcpServer {
    #[tool(
        description = "Get attendance calendar for a month. Returns daily entries with entry/exit times, types, and error status."
    )]
    async fn hilan_status(&self, Parameters(req): Parameters<MonthParam>) -> String {
        json_or_error(|| async {
            let mut client = new_client().await?;
            let month = parse_month(&req.month)?;
            let cal = attendance::read_calendar(&mut client, month)
                .await
                .map_err(|e| format!("{e}"))?;

            let days: Vec<serde_json::Value> = cal
                .days
                .iter()
                .map(|d| {
                    serde_json::json!({
                        "date": d.date.format("%Y-%m-%d").to_string(),
                        "day": &d.day_name,
                        "entry": d.entry_time,
                        "exit": d.exit_time,
                        "type": d.attendance_type,
                        "hours": d.total_hours,
                        "error": d.has_error,
                        "error_message": d.error_message,
                    })
                })
                .collect();

            Ok(serde_json::json!({
                "month": cal.month.format("%Y-%m").to_string(),
                "employee_id": cal.employee_id,
                "days": days,
            }))
        })
        .await
    }

    #[tool(description = "Get attendance errors for a month. Returns only days with errors.")]
    async fn hilan_errors(&self, Parameters(req): Parameters<MonthParam>) -> String {
        json_or_error(|| async {
            let mut client = new_client().await?;
            let month = parse_month(&req.month)?;
            let cal = attendance::read_calendar(&mut client, month)
                .await
                .map_err(|e| format!("{e}"))?;

            let errors: Vec<serde_json::Value> = cal
                .days
                .iter()
                .filter(|d| d.has_error)
                .map(|d| {
                    serde_json::json!({
                        "date": d.date.format("%Y-%m-%d").to_string(),
                        "day": &d.day_name,
                        "error_message": d.error_message,
                    })
                })
                .collect();

            Ok(serde_json::json!({
                "month": cal.month.format("%Y-%m").to_string(),
                "employee_id": cal.employee_id,
                "error_count": errors.len(),
                "errors": errors,
            }))
        })
        .await
    }

    #[tool(
        description = "List available attendance types from the local ontology cache (auto-syncs if missing)."
    )]
    async fn hilan_types(&self) -> String {
        json_or_error(|| async {
            let config = Config::load().map_err(|e| format!("{e}"))?;
            let mut client =
                HilanClient::new(config.clone()).map_err(|e| format!("client error: {e}"))?;
            let ont = ontology::OrgOntology::load_or_sync(&mut client, &config.subdomain)
                .await
                .map_err(|e| format!("{e}"))?;
            let types: Vec<serde_json::Value> = ont
                .types
                .iter()
                .map(|t| {
                    serde_json::json!({
                        "code": t.code,
                        "name_he": t.name_he,
                        "name_en": t.name_en,
                    })
                })
                .collect();
            Ok(serde_json::json!({
                "subdomain": ont.subdomain,
                "types": types,
            }))
        })
        .await
    }

    #[tool(
        description = "Clock in for today. Defaults to dry-run preview unless execute is true. CAUTION: write operation."
    )]
    async fn hilan_clock_in(&self, Parameters(req): Parameters<ClockParam>) -> String {
        json_or_error(|| async {
            let config = Config::load().map_err(|e| format!("{e}"))?;
            let subdomain = config.subdomain.clone();
            let mut client = HilanClient::new(config).map_err(|e| format!("client error: {e}"))?;
            client
                .ensure_authenticated()
                .await
                .map_err(|e| format!("auth error: {e}"))?;
            let execute = req.execute.unwrap_or(false);

            let type_code = resolve_type_code(&subdomain, None)?;
            let submit = attendance::AttendanceSubmit {
                date: chrono::Local::now().date_naive(),
                attendance_type_code: type_code,
                entry_time: Some(req.time.clone()),
                exit_time: None,
                comment: None,
                // Write safety: preserve existing exit time and comment
                clear_entry: false,
                clear_exit: false,
                clear_comment: false,
                default_work_day: true,
            };
            let preview = attendance::submit_day(&mut client, &submit, execute)
                .await
                .map_err(|e| format!("{e}"))?;
            Ok(preview_json("clock_in", &preview))
        })
        .await
    }

    #[tool(
        description = "Clock out for today. Defaults to dry-run preview unless execute is true. CAUTION: write operation."
    )]
    async fn hilan_clock_out(&self, Parameters(req): Parameters<ClockParam>) -> String {
        json_or_error(|| async {
            let mut client = new_client().await?;
            let execute = req.execute.unwrap_or(false);

            let submit = attendance::AttendanceSubmit {
                date: chrono::Local::now().date_naive(),
                attendance_type_code: None,
                entry_time: None,
                exit_time: Some(req.time.clone()),
                comment: None,
                // Write safety: preserve existing entry time and comment
                clear_entry: false,
                clear_exit: false,
                clear_comment: false,
                default_work_day: false,
            };
            let preview = attendance::submit_day(&mut client, &submit, execute)
                .await
                .map_err(|e| format!("{e}"))?;
            Ok(preview_json("clock_out", &preview))
        })
        .await
    }

    #[tool(
        description = "Fill attendance for a date range. Defaults to dry-run preview. Skips weekends (Fri/Sat). CAUTION: write operation."
    )]
    async fn hilan_fill(&self, Parameters(req): Parameters<FillParam>) -> String {
        json_or_error(|| async {
            let config = Config::load().map_err(|e| format!("{e}"))?;
            let subdomain = config.subdomain.clone();
            let mut client = HilanClient::new(config).map_err(|e| format!("client error: {e}"))?;
            client
                .ensure_authenticated()
                .await
                .map_err(|e| format!("auth error: {e}"))?;
            let execute = req.execute.unwrap_or(false);
            let from = parse_date(&req.from)?;
            let to = parse_date(&req.to)?;
            if from > to {
                return Err("'from' must be before or equal to 'to'".into());
            }

            let type_code = resolve_type_code(&subdomain, req.r#type.as_deref())?;

            let mut results = Vec::new();
            let mut current = from;
            while current <= to {
                // Skip weekends (Fri=5, Sat=6 in Israeli calendar)
                let weekday = current.weekday();
                if matches!(weekday, chrono::Weekday::Fri | chrono::Weekday::Sat) {
                    current += chrono::Duration::days(1);
                    continue;
                }

                let submit = attendance::AttendanceSubmit {
                    date: current,
                    attendance_type_code: type_code.clone(),
                    entry_time: Some(req.entry.clone()),
                    exit_time: Some(req.exit.clone()),
                    comment: None,
                    // Write safety: do not clear existing data
                    clear_entry: false,
                    clear_exit: false,
                    clear_comment: false,
                    default_work_day: type_code.is_none(),
                };
                let preview = attendance::submit_day(&mut client, &submit, execute)
                    .await
                    .map_err(|e| format!("{e}"))?;
                results.push(serde_json::json!({
                    "date": current.format("%Y-%m-%d").to_string(),
                    "executed": preview.executed,
                    "employee_id": preview.employee_id,
                }));
                current += chrono::Duration::days(1);
            }

            Ok(serde_json::json!({
                "action": "fill",
                "from": req.from,
                "to": req.to,
                "entry": req.entry,
                "exit": req.exit,
                "days_processed": results.len(),
                "execute": execute,
                "results": results,
            }))
        })
        .await
    }

    #[tool(
        description = "Automatically fill all missing days in a month. Defaults to dry-run preview. Skips weekends. CAUTION: write operation."
    )]
    async fn hilan_auto_fill(&self, Parameters(req): Parameters<AutoFillParam>) -> String {
        json_or_error(|| async {
            let config = Config::load().map_err(|e| format!("{e}"))?;
            let subdomain = config.subdomain.clone();
            let mut client = HilanClient::new(config).map_err(|e| format!("client error: {e}"))?;
            client
                .ensure_authenticated()
                .await
                .map_err(|e| format!("auth error: {e}"))?;

            let execute = req.execute.unwrap_or(false);
            let include_weekends = req.include_weekends.unwrap_or(false);
            let max_days = req.max_days.unwrap_or(10);

            let month = parse_month_or_current(req.month.as_deref())?;
            let hours = req.hours.as_deref().map(parse_hours_range).transpose()?;

            if req.r#type.is_none() && hours.is_none() {
                return Err("auto-fill requires type or hours parameter".into());
            }

            // Auto-sync ontology if needed
            if req.r#type.is_some() && !ontology::ontology_path(&subdomain).exists() {
                ontology::sync_from_calendar(&mut client, &subdomain)
                    .await
                    .map_err(|e| format!("{e}"))?;
            }

            let cal = attendance::read_calendar(&mut client, month)
                .await
                .map_err(|e| format!("{e}"))?;

            let (type_code, type_display) =
                attendance::resolve_auto_fill_type(&subdomain, req.r#type.as_deref())
                    .map_err(|e| format!("{e}"))?;

            let result = attendance::auto_fill(
                &mut client,
                &cal,
                attendance::AutoFillOpts {
                    type_code,
                    type_display: &type_display,
                    hours: hours.as_ref(),
                    include_weekends,
                    execute,
                    max_days,
                },
            )
            .await
            .map_err(|e| format!("{e}"))?;

            serde_json::to_value(&result).map_err(|e| format!("{e}"))
        })
        .await
    }

    #[tool(description = "Get salary summary for recent months.")]
    async fn hilan_salary(&self, Parameters(req): Parameters<SalaryParam>) -> String {
        json_or_error(|| async {
            let config = Config::load().map_err(|e| format!("{e}"))?;
            let mut client = HilanClient::new(config).map_err(|e| format!("client error: {e}"))?;
            let months = req.months.unwrap_or(3);
            let summary = client.salary(months).await.map_err(|e| format!("{e}"))?;

            let entries: Vec<serde_json::Value> = summary
                .entries
                .iter()
                .map(|e| {
                    serde_json::json!({
                        "month": e.month.format("%Y-%m").to_string(),
                        "amount": e.amount,
                    })
                })
                .collect();

            Ok(serde_json::json!({
                "label": summary.label,
                "entries": entries,
                "percent_diff": summary.percent_diff,
            }))
        })
        .await
    }

    #[tool(description = "Show the attendance timesheet (hours analysis).")]
    async fn hilan_sheet(&self) -> String {
        json_or_error(|| async {
            let mut client = new_client().await?;
            let table = reports::fetch_table_from_url(&mut client, reports::SHEET_URL_PATH)
                .await
                .map_err(|e| format!("{e}"))?;
            serde_json::to_value(&table).map_err(|e| format!("{e}"))
        })
        .await
    }

    #[tool(description = "Show the attendance correction log (manual reporting history).")]
    async fn hilan_corrections(&self) -> String {
        json_or_error(|| async {
            let mut client = new_client().await?;
            let table = reports::fetch_table_from_url(&mut client, reports::CORRECTIONS_URL_PATH)
                .await
                .map_err(|e| format!("{e}"))?;
            serde_json::to_value(&table).map_err(|e| format!("{e}"))
        })
        .await
    }

    #[tool(description = "Show absences initial data (attendance symbols and display names).")]
    async fn hilan_absences(&self) -> String {
        json_or_error(|| async {
            let mut client = new_client().await?;
            let data = api::get_absences_initial(&mut client)
                .await
                .map_err(|e| format!("{e}"))?;

            let symbols: Vec<serde_json::Value> = data
                .symbols
                .iter()
                .map(|s| {
                    serde_json::json!({
                        "id": s.id,
                        "name": s.name,
                        "display_name": s.display_name,
                    })
                })
                .collect();

            Ok(serde_json::json!({
                "symbol_count": symbols.len(),
                "symbols": symbols,
            }))
        })
        .await
    }

    #[tool(
        description = "Get overview for a month: identity, summary, errors, missing days, and suggested actions."
    )]
    async fn hilan_overview(&self, Parameters(req): Parameters<OverviewParam>) -> String {
        json_or_error(|| async {
            let config = Config::load().map_err(|e| format!("{e}"))?;
            let subdomain = config.subdomain.clone();
            let mut client =
                HilanClient::new(config).map_err(|e| format!("client error: {e}"))?;
            client
                .ensure_authenticated()
                .await
                .map_err(|e| format!("auth error: {e}"))?;

            let bootstrap = api::bootstrap(&mut client)
                .await
                .map_err(|e| format!("{e}"))?;

            let month = parse_month_or_current(req.month.as_deref())?;
            let cal = attendance::read_calendar(&mut client, month)
                .await
                .map_err(|e| format!("{e}"))?;

            let today = chrono::Local::now().date_naive();
            let is_current_month =
                month.year() == today.year() && month.month() == today.month();
            let is_past =
                |d: &attendance::CalendarDay| !(is_current_month && d.date > today);

            let error_days_raw: Vec<&attendance::CalendarDay> =
                cal.days.iter().filter(|d| d.has_error).collect();
            let error_tasks_by_date: BTreeMap<chrono::NaiveDate, api::ErrorTask> =
                if error_days_raw.is_empty() {
                    BTreeMap::new()
                } else {
                    match api::get_error_tasks(&mut client).await {
                        Ok(tasks) => tasks.into_iter().map(|task| (task.date, task)).collect(),
                        Err(err) => {
                            tracing::debug!("error task lookup failed: {err}");
                            BTreeMap::new()
                        }
                    }
                };

            let error_days: Vec<serde_json::Value> = error_days_raw
                .iter()
                .map(|d| {
                    let fix_params = error_tasks_by_date.get(&d.date).map(|task| {
                        serde_json::json!({
                            "report_id": task.report_id,
                            "error_type": task.error_type,
                        })
                    });

                    serde_json::json!({
                        "date": d.date.format("%Y-%m-%d").to_string(),
                        "day": &d.day_name,
                        "error_message": d.error_message,
                        "fix_params": fix_params,
                    })
                })
                .collect();

            let reported_count = cal.days.iter().filter(|d| d.is_reported()).count();

            let missing_days: Vec<String> = cal
                .days
                .iter()
                .filter(|d| d.is_work_day() && !d.is_reported() && !d.has_error && is_past(d))
                .map(|d| d.date.format("%Y-%m-%d").to_string())
                .collect();

            let total_work_days = cal
                .days
                .iter()
                .filter(|d| d.is_work_day() && is_past(d))
                .count();

            // Load ontology types (non-fatal if unavailable)
            let types = match ontology::OrgOntology::load_or_sync(&mut client, &subdomain).await {
                Ok(ont) => ont
                    .types
                    .into_iter()
                    .map(|t| {
                        serde_json::json!({
                            "code": t.code,
                            "name_he": t.name_he,
                            "name_en": t.name_en,
                        })
                    })
                    .collect::<Vec<_>>(),
                Err(_) => Vec::new(),
            };

            let mut suggested_actions = Vec::new();
            if !error_days.is_empty() {
                let fixable_days: Vec<serde_json::Value> = error_days
                    .iter()
                    .filter_map(|day| {
                        day.get("fix_params").and_then(|params| {
                            if params.is_null() {
                                None
                            } else {
                                Some(serde_json::json!({
                                    "date": day["date"],
                                    "report_id": params["report_id"],
                                    "error_type": params["error_type"],
                                }))
                            }
                        })
                    })
                    .collect();
                suggested_actions.push(serde_json::json!({
                    "kind": "fix_errors",
                    "reason": format!("{} day(s) have attendance errors", error_days.len()),
                    "params": {
                        "month": month.format("%Y-%m").to_string(),
                        "count": error_days.len(),
                        "fixable_days": fixable_days,
                    },
                    "safety": "dry_run_default",
                }));
            }
            if !missing_days.is_empty() {
                suggested_actions.push(serde_json::json!({
                    "kind": "fill_missing",
                    "reason": format!("{} work day(s) have no attendance report", missing_days.len()),
                    "from": missing_days.first(),
                    "to": missing_days.last(),
                    "safety": "dry_run_default",
                }));
            }

            Ok(serde_json::json!({
                "user": {
                    "user_id": bootstrap.user_id,
                    "employee_id": bootstrap.employee_id,
                    "name": bootstrap.name,
                    "is_manager": bootstrap.is_manager,
                },
                "month": month.format("%Y-%m").to_string(),
                "summary": {
                    "total_work_days": total_work_days,
                    "reported": reported_count,
                    "missing": missing_days.len(),
                    "errors": error_days.len(),
                },
                "attendance_types": types,
                "error_days": error_days,
                "missing_days": missing_days,
                "suggested_actions": suggested_actions,
            }))
        })
        .await
    }
}

#[tool_handler]
impl ServerHandler for HilanMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build()).with_instructions(
            "Hilan attendance & payslip server. \
                 Read tools return JSON data. \
                 Write tools (clock_in, clock_out, fill, auto_fill) default to dry-run; \
                 set execute=true to submit.",
        )
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn resolve_type_code(subdomain: &str, requested: Option<&str>) -> Result<Option<String>, String> {
    let Some(requested) = requested else {
        return Ok(None);
    };
    let path = ontology::ontology_path(subdomain);
    if path.exists() {
        let ont = ontology::OrgOntology::load(&path).map_err(|e| format!("{e}"))?;
        return Ok(Some(
            ont.validate_type(requested)
                .map_err(|e| format!("{e}"))?
                .code
                .clone(),
        ));
    }
    if requested.chars().all(|ch| ch.is_ascii_digit()) {
        return Ok(Some(requested.to_string()));
    }
    Err(format!(
        "Attendance type '{requested}' needs cached ontology. Run `hilan sync-types` first."
    ))
}

fn preview_json(action: &str, preview: &attendance::SubmitPreview) -> serde_json::Value {
    serde_json::json!({
        "action": action,
        "executed": preview.executed,
        "url": preview.url,
        "employee_id": preview.employee_id,
        "button": {
            "name": preview.button_name,
            "value": preview.button_value,
        },
    })
}
