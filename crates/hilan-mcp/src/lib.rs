use anyhow::Result;
use chrono::{Datelike, Local, NaiveDate};
use hr_core::use_cases;
use hr_core::{
    AttendanceChange, AttendanceProvider, FixTarget, ProviderError, ReportProvider, ReportSpec,
    SalaryProvider, WriteMode, WritePreview,
};
use provider_hilan::{Config, HilanProvider};
use rmcp::{
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{ServerCapabilities, ServerInfo},
    schemars, tool, tool_handler, tool_router, ServerHandler,
};
use serde::Deserialize;

const SHEET_REPORT_PATH: &str = "/Hilannetv2/Attendance/HoursAnalysis.aspx";
const CORRECTIONS_REPORT_PATH: &str = "/Hilannetv2/Attendance/HoursReportLog.aspx";

#[derive(Debug)]
struct ToolError {
    code: String,
    message: String,
    retryable: bool,
    details: Option<serde_json::Value>,
}

impl ToolError {
    fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            retryable: false,
            details: None,
        }
    }
}

impl From<ProviderError> for ToolError {
    fn from(value: ProviderError) -> Self {
        Self {
            code: value.code.into_owned(),
            message: value.message,
            retryable: value.retryable,
            details: value.details,
        }
    }
}

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

pub async fn serve_stdio() -> Result<()> {
    use rmcp::ServiceExt;

    let server = HilanMcpServer::new();
    let transport = rmcp::transport::io::stdio();
    server.serve(transport).await?.waiting().await?;
    Ok(())
}

async fn new_provider() -> Result<HilanProvider, ToolError> {
    let config =
        Config::load().map_err(|e| ToolError::new("config_error", format!("config error: {e}")))?;
    provider_hilan::build_provider(config)
        .map_err(|e| ToolError::new("provider_init_failed", format!("provider error: {e}")))
}

/// Convert a fallible async block result into a JSON string, wrapping errors
/// with a structured envelope so downstream MCP clients can branch on `code`.
async fn json_or_error<F, Fut>(f: F) -> String
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<serde_json::Value, ToolError>>,
{
    match f().await {
        Ok(val) => serde_json::to_string_pretty(&val).unwrap_or_else(|e| {
            serde_json::to_string(&serde_json::json!({
                "error": {
                    "code": "serialization_failed",
                    "message": format!("serialization failed: {e}"),
                    "retryable": false,
                }
            }))
            .unwrap_or_default()
        }),
        Err(err) => serde_json::to_string(&serde_json::json!({
            "error": {
                "code": err.code,
                "message": err.message,
                "retryable": err.retryable,
                "details": err.details,
            }
        }))
        .unwrap_or_default(),
    }
}

fn parse_month(value: &str) -> Result<NaiveDate, ToolError> {
    NaiveDate::parse_from_str(&format!("{value}-01"), "%Y-%m-%d")
        .map_err(|e| ToolError::new("invalid_month", format!("invalid month '{value}': {e}")))
}

fn parse_month_or_current(value: Option<&str>) -> Result<NaiveDate, ToolError> {
    match value {
        Some(v) => parse_month(v),
        None => Local::now().date_naive().with_day(1).ok_or_else(|| {
            ToolError::new(
                "current_month_failed",
                "failed to get current month start".to_string(),
            )
        }),
    }
}

fn parse_date(value: &str) -> Result<NaiveDate, ToolError> {
    NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .map_err(|e| ToolError::new("invalid_date", format!("invalid date '{value}': {e}")))
}

fn parse_hours_range(value: &str) -> Result<(String, String), ToolError> {
    let (entry, exit) = value
        .split_once('-')
        .ok_or_else(|| ToolError::new("invalid_hours", "hours must be in HH:MM-HH:MM format"))?;
    Ok((entry.to_string(), exit.to_string()))
}

fn write_mode(execute: bool) -> WriteMode {
    if execute {
        WriteMode::Execute
    } else {
        WriteMode::DryRun
    }
}

fn fill_dates(from: NaiveDate, to: NaiveDate) -> Vec<NaiveDate> {
    let mut dates = Vec::new();
    let mut current = from;
    while current <= to {
        if !matches!(
            current.weekday(),
            chrono::Weekday::Fri | chrono::Weekday::Sat
        ) {
            dates.push(current);
        }
        current = current.succ_opt().expect("valid next date");
    }
    dates
}

fn preview_debug_field<'a>(preview: &'a WritePreview, key: &str) -> Option<&'a str> {
    preview
        .provider_debug
        .as_ref()
        .and_then(|debug| debug.get(key))
        .and_then(serde_json::Value::as_str)
}

fn write_preview_json(action: &str, preview: &WritePreview) -> serde_json::Value {
    serde_json::json!({
        "action": action,
        "executed": preview.executed,
        "summary": preview.summary,
        "url": preview_debug_field(preview, "url"),
        "employee_id": preview_debug_field(preview, "employee_id"),
        "button": {
            "name": preview_debug_field(preview, "button_name"),
            "value": preview_debug_field(preview, "button_value"),
        },
        "payload_display": preview_debug_field(preview, "payload_display"),
    })
}

fn fix_params_json(target: &FixTarget) -> Option<serde_json::Value> {
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
        (Some(report_id), Some(error_type)) => Some(serde_json::json!({
            "report_id": report_id,
            "error_type": error_type,
        })),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Tool implementations
// ---------------------------------------------------------------------------

#[tool_router]
impl HilanMcpServer {
    #[tool(
        description = "Get attendance calendar for a month. Returns daily entries with entry/exit times, types, and error status."
    )]
    async fn hilan_status(&self, Parameters(req): Parameters<MonthParam>) -> String {
        json_or_error(|| async {
            let mut provider = new_provider().await?;
            let month = parse_month(&req.month)?;
            let cal = provider
                .month_calendar(month)
                .await
                .map_err(ToolError::from)?;

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
            let mut provider = new_provider().await?;
            let month = parse_month(&req.month)?;
            let overview =
                use_cases::build_overview(&mut provider, month, Local::now().date_naive())
                    .await
                    .map_err(ToolError::from)?;

            let errors: Vec<serde_json::Value> = overview
                .error_days
                .iter()
                .map(|entry| {
                    serde_json::json!({
                        "date": entry.day.date.format("%Y-%m-%d").to_string(),
                        "day": &entry.day.day_name,
                        "error_message": entry.day.error_message,
                        "fix_params": entry.fix_target.as_ref().and_then(fix_params_json),
                    })
                })
                .collect();

            Ok(serde_json::json!({
                "month": overview.month.format("%Y-%m").to_string(),
                "employee_id": overview.calendar.employee_id,
                "error_count": errors.len(),
                "errors": errors,
            }))
        })
        .await
    }

    #[tool(description = "List available attendance types from the provider cache or live sync.")]
    async fn hilan_types(&self) -> String {
        json_or_error(|| async {
            let subdomain = Config::load()
                .map_err(|e| ToolError::new("config_error", format!("config error: {e}")))?
                .subdomain;
            let mut provider = new_provider().await?;
            let types = provider.attendance_types().await.map_err(ToolError::from)?;
            let types: Vec<serde_json::Value> = types
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
                "subdomain": subdomain,
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
            let mut provider = new_provider().await?;
            let preview = provider
                .submit_day(
                    &AttendanceChange {
                        date: Local::now().date_naive(),
                        attendance_type_code: None,
                        use_default_attendance_type: true,
                        entry_time: Some(req.time.clone()),
                        exit_time: None,
                        comment: None,
                        clear_entry: false,
                        clear_exit: false,
                        clear_comment: false,
                    },
                    write_mode(req.execute.unwrap_or(false)),
                )
                .await
                .map_err(ToolError::from)?;
            Ok(write_preview_json("clock_in", &preview))
        })
        .await
    }

    #[tool(
        description = "Clock out for today. Defaults to dry-run preview unless execute is true. CAUTION: write operation."
    )]
    async fn hilan_clock_out(&self, Parameters(req): Parameters<ClockParam>) -> String {
        json_or_error(|| async {
            let mut provider = new_provider().await?;
            let preview = provider
                .submit_day(
                    &AttendanceChange {
                        date: Local::now().date_naive(),
                        attendance_type_code: None,
                        use_default_attendance_type: false,
                        entry_time: None,
                        exit_time: Some(req.time.clone()),
                        comment: None,
                        clear_entry: false,
                        clear_exit: false,
                        clear_comment: false,
                    },
                    write_mode(req.execute.unwrap_or(false)),
                )
                .await
                .map_err(ToolError::from)?;
            Ok(write_preview_json("clock_out", &preview))
        })
        .await
    }

    #[tool(
        description = "Fill attendance for a date range. Defaults to dry-run preview. Skips weekends (Fri/Sat). CAUTION: write operation."
    )]
    async fn hilan_fill(&self, Parameters(req): Parameters<FillParam>) -> String {
        json_or_error(|| async {
            let mut provider = new_provider().await?;
            let execute = req.execute.unwrap_or(false);
            let from = parse_date(&req.from)?;
            let to = parse_date(&req.to)?;
            if from > to {
                return Err(ToolError::new(
                    "invalid_fill_range",
                    "'from' must be before or equal to 'to'",
                ));
            }

            let resolved_type =
                use_cases::resolve_attendance_type(&mut provider, req.r#type.as_deref())
                    .await
                    .map_err(ToolError::from)?;

            let dates = fill_dates(from, to);
            let previews = use_cases::fill_range(
                &mut provider,
                from,
                to,
                use_cases::FillRangeOptions {
                    attendance_type_code: resolved_type
                        .as_ref()
                        .map(|resolved| resolved.code.clone()),
                    hours: Some((req.entry.clone(), req.exit.clone())),
                    include_weekends: false,
                    mode: write_mode(execute),
                },
            )
            .await
            .map_err(ToolError::from)?;

            let results: Vec<serde_json::Value> = dates
                .into_iter()
                .zip(previews.iter())
                .map(|(date, preview)| {
                    serde_json::json!({
                        "date": date.format("%Y-%m-%d").to_string(),
                        "executed": preview.executed,
                        "employee_id": preview_debug_field(preview, "employee_id"),
                    })
                })
                .collect();

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
            let mut provider = new_provider().await?;
            let execute = req.execute.unwrap_or(false);
            let include_weekends = req.include_weekends.unwrap_or(false);
            let max_days = req.max_days.unwrap_or(10);
            let month = parse_month_or_current(req.month.as_deref())?;
            let hours = req.hours.as_deref().map(parse_hours_range).transpose()?;

            if req.r#type.is_none() && hours.is_none() {
                return Err(ToolError::new(
                    "invalid_auto_fill_request",
                    "auto-fill requires type or hours parameter",
                ));
            }

            let resolved_type =
                use_cases::resolve_attendance_type(&mut provider, req.r#type.as_deref())
                    .await
                    .map_err(ToolError::from)?;
            let cal = provider
                .month_calendar(month)
                .await
                .map_err(ToolError::from)?;
            let result = use_cases::auto_fill(
                &mut provider,
                &cal,
                use_cases::AutoFillOptions {
                    type_code: resolved_type.as_ref().map(|resolved| resolved.code.clone()),
                    type_display: resolved_type
                        .map(|resolved| resolved.display)
                        .unwrap_or_default(),
                    hours,
                    include_weekends,
                    mode: write_mode(execute),
                    max_days,
                    today: Local::now().date_naive(),
                },
            )
            .await
            .map_err(ToolError::from)?;

            serde_json::to_value(&result).map_err(|e| {
                ToolError::new(
                    "serialization_failed",
                    format!("failed to serialize result: {e}"),
                )
            })
        })
        .await
    }

    #[tool(description = "Get salary summary for recent months.")]
    async fn hilan_salary(&self, Parameters(req): Parameters<SalaryParam>) -> String {
        json_or_error(|| async {
            let mut provider = new_provider().await?;
            let summary = provider
                .salary_summary(req.months.unwrap_or(3))
                .await
                .map_err(ToolError::from)?;

            let entries: Vec<serde_json::Value> = summary
                .entries
                .iter()
                .map(|entry| {
                    serde_json::json!({
                        "month": entry.month.format("%Y-%m").to_string(),
                        "amount": entry.amount,
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
            let mut provider = new_provider().await?;
            let table = provider
                .report(ReportSpec::Path(SHEET_REPORT_PATH.to_string()))
                .await
                .map_err(ToolError::from)?;
            serde_json::to_value(&table).map_err(|e| {
                ToolError::new(
                    "serialization_failed",
                    format!("failed to serialize report: {e}"),
                )
            })
        })
        .await
    }

    #[tool(description = "Show the attendance correction log (manual reporting history).")]
    async fn hilan_corrections(&self) -> String {
        json_or_error(|| async {
            let mut provider = new_provider().await?;
            let table = provider
                .report(ReportSpec::Path(CORRECTIONS_REPORT_PATH.to_string()))
                .await
                .map_err(ToolError::from)?;
            serde_json::to_value(&table).map_err(|e| {
                ToolError::new(
                    "serialization_failed",
                    format!("failed to serialize report: {e}"),
                )
            })
        })
        .await
    }

    #[tool(description = "Show absences initial data (attendance symbols and display names).")]
    async fn hilan_absences(&self) -> String {
        json_or_error(|| async {
            let mut provider = new_provider().await?;
            let symbols = use_cases::load_absence_symbols(&mut provider)
                .await
                .map_err(ToolError::from)?;

            let symbols: Vec<serde_json::Value> = symbols
                .iter()
                .map(|symbol| {
                    serde_json::json!({
                        "id": symbol.id,
                        "name": symbol.name,
                        "display_name": symbol.display_name,
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
            let mut provider = new_provider().await?;
            let month = parse_month_or_current(req.month.as_deref())?;
            let overview =
                use_cases::build_overview(&mut provider, month, Local::now().date_naive())
                    .await
                    .map_err(ToolError::from)?;

            let error_days: Vec<serde_json::Value> = overview
                .error_days
                .iter()
                .map(|entry| {
                    serde_json::json!({
                        "date": entry.day.date.format("%Y-%m-%d").to_string(),
                        "day": &entry.day.day_name,
                        "error_message": entry.day.error_message,
                        "fix_params": entry.fix_target.as_ref().and_then(fix_params_json),
                    })
                })
                .collect();

            let missing_days: Vec<String> = overview
                .missing_days
                .iter()
                .map(|date| date.format("%Y-%m-%d").to_string())
                .collect();

            let attendance_types: Vec<serde_json::Value> = overview
                .attendance_types
                .iter()
                .map(|item| {
                    serde_json::json!({
                        "code": item.code,
                        "name_he": item.name_he,
                        "name_en": item.name_en,
                    })
                })
                .collect();

            let suggested_actions: Vec<serde_json::Value> = overview
                .suggested_actions
                .iter()
                .map(|action| match action {
                    use_cases::SuggestedActionPlan::FixErrors {
                        month,
                        count,
                        fixable_targets,
                    } => serde_json::json!({
                        "kind": "fix_errors",
                        "reason": format!("{count} day(s) have attendance errors"),
                        "params": {
                            "month": month.format("%Y-%m").to_string(),
                            "count": count,
                            "fixable_days": fixable_targets
                                .iter()
                                .filter_map(|target| {
                                    fix_params_json(target).map(|params| {
                                        serde_json::json!({
                                            "date": target.date.format("%Y-%m-%d").to_string(),
                                            "report_id": params["report_id"],
                                            "error_type": params["error_type"],
                                        })
                                    })
                                })
                                .collect::<Vec<_>>(),
                        },
                        "safety": "dry_run_default",
                    }),
                    use_cases::SuggestedActionPlan::FillMissing { from, to, count } => {
                        serde_json::json!({
                            "kind": "fill_missing",
                            "reason": format!("{count} work day(s) have no attendance report"),
                            "params": {
                                "from": from.format("%Y-%m-%d").to_string(),
                                "to": to.format("%Y-%m-%d").to_string(),
                                "count": count,
                            },
                            "safety": "dry_run_default",
                        })
                    }
                })
                .collect();

            Ok(serde_json::json!({
                "user": {
                    "user_id": overview.identity.user_id,
                    "employee_id": overview.identity.employee_id,
                    "name": overview.identity.display_name,
                    "is_manager": overview.identity.is_manager,
                },
                "month": overview.month.format("%Y-%m").to_string(),
                "summary": {
                    "total_work_days": overview.summary.total_work_days,
                    "reported": overview.summary.reported,
                    "missing": overview.summary.missing,
                    "errors": overview.summary.errors,
                },
                "attendance_types": attendance_types,
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
