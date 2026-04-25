use anyhow::Result;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use chrono::{Datelike, Local, NaiveDate};
use hr_core::use_cases;
use hr_core::{
    AttendanceChange, AttendanceProvider, FixTarget, PayslipProvider, ProviderError,
    ReportProvider, ReportSpec, SalaryProvider, WriteMode, WritePreview,
};
use provider_hilan::{Config, HilanProvider};
use rmcp::{
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{ServerCapabilities, ServerInfo},
    schemars, tool, tool_handler, tool_router, ServerHandler,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

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
    #[schemars(
        description = "Human-attested write. Only set execute=true after the user has reviewed the dry-run preview and explicitly confirmed submission."
    )]
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
    #[schemars(
        description = "Human-attested write. Only set execute=true after the user has reviewed the dry-run preview and explicitly confirmed submission."
    )]
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
    #[schemars(
        description = "Human-attested write. Only set execute=true after the user has reviewed the dry-run preview and explicitly confirmed submission."
    )]
    pub execute: Option<bool>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SalaryParam {
    #[schemars(description = "Number of months to show (default: 3)")]
    pub months: Option<u32>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ResolveParam {
    #[schemars(description = "Day to resolve in YYYY-MM-DD format")]
    pub date: String,
    #[schemars(description = "Attendance type override")]
    pub r#type: Option<String>,
    #[schemars(description = "Hours range in HH:MM-HH:MM format (e.g. '09:00-18:00')")]
    pub hours: Option<String>,
    #[schemars(
        description = "Human-attested write. Only set execute=true after the user has reviewed the dry-run preview and explicitly confirmed submission."
    )]
    pub execute: Option<bool>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct OverviewParam {
    #[schemars(description = "Month in YYYY-MM format (default: current month)")]
    pub month: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct PayslipDownloadParam {
    #[schemars(description = "Month in YYYY-MM format (default: previous month)")]
    pub month: Option<String>,
    #[schemars(description = "Optional output path for saving the PDF locally")]
    pub output_path: Option<String>,
    #[schemars(
        description = "Sensitive payroll document. Prefer output_path for local storage; include_bytes is explicit opt-in because MCP responses may be logged."
    )]
    pub include_bytes: Option<bool>,
}

#[derive(Debug, Serialize)]
struct StatusResponse {
    month: String,
    employee_id: String,
    days: Vec<StatusDay>,
}

#[derive(Debug, Serialize)]
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

#[derive(Clone, Debug, Serialize)]
struct ErrorFixParams {
    report_id: String,
    error_type: String,
}

#[derive(Debug, Serialize)]
struct ErrorDayResponse {
    date: String,
    day_name: String,
    error_message: String,
    fix_params: Option<ErrorFixParams>,
    fix_params_candidates: Vec<ErrorFixParams>,
}

#[derive(Debug, Serialize)]
struct ErrorsResponse {
    month: String,
    employee_id: String,
    error_count: usize,
    errors: Vec<ErrorDayResponse>,
}

#[derive(Debug, Serialize)]
struct MissingDay {
    date: String,
    day_name: String,
}

#[derive(Debug, Serialize)]
struct FixableDay {
    date: String,
    report_id: String,
    error_type: String,
}

#[derive(Debug, Serialize)]
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

#[derive(Debug, Serialize)]
struct OverviewUser {
    user_id: String,
    employee_id: String,
    name: String,
    is_manager: bool,
}

#[derive(Debug, Serialize)]
struct OverviewSummary {
    total_work_days: u32,
    reported: u32,
    missing: u32,
    errors: u32,
}

#[derive(Debug, Serialize)]
struct OverviewResponse {
    user: OverviewUser,
    month: String,
    summary: OverviewSummary,
    attendance_types: Vec<hr_core::AttendanceType>,
    error_days: Vec<ErrorDayResponse>,
    missing_days: Vec<MissingDay>,
    suggested_actions: Vec<SuggestedAction>,
}

// ---------------------------------------------------------------------------
// MCP Server
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct ShaonMcpServer {
    #[allow(dead_code)]
    tool_router: ToolRouter<Self>,
}

impl Default for ShaonMcpServer {
    fn default() -> Self {
        Self::new()
    }
}

impl ShaonMcpServer {
    pub fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
        }
    }
}

pub async fn serve_stdio() -> Result<()> {
    use rmcp::ServiceExt;

    let server = ShaonMcpServer::new();
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

fn to_json_value<T: Serialize>(value: &T) -> Result<serde_json::Value, ToolError> {
    serde_json::to_value(value).map_err(|e| {
        ToolError::new(
            "serialization_failed",
            format!("failed to serialize response: {e}"),
        )
    })
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

fn parse_month_or_previous(value: Option<&str>) -> Result<NaiveDate, ToolError> {
    match value {
        Some(v) => parse_month(v),
        None => Ok(provider_hilan::client::previous_month_start(
            Local::now().date_naive(),
        )),
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

fn write_preview_json(action: &str, preview: &WritePreview) -> serde_json::Value {
    serde_json::json!({
        "action": action,
        "executed": preview.executed,
        "summary": preview.summary,
        "url": preview.debug_field("url"),
        "employee_id": preview.debug_field("employee_id"),
        "button": {
            "name": preview.debug_field("button_name"),
            "value": preview.debug_field("button_value"),
        },
        "payload_display": preview.debug_field("payload_display"),
    })
}

fn fill_result_json(date: NaiveDate, preview: &WritePreview) -> serde_json::Value {
    let mut value = write_preview_json("fill_day", preview);
    if let serde_json::Value::Object(ref mut object) = value {
        object.insert(
            "date".to_string(),
            serde_json::json!(date.format("%Y-%m-%d").to_string()),
        );
    }
    value
}

fn find_fix_target_for_date(
    fix_targets: &[FixTarget],
    date: NaiveDate,
) -> Result<FixTarget, ToolError> {
    let mut matches = fix_targets
        .iter()
        .filter(|target| target.date == date)
        .cloned();

    match (matches.next(), matches.next()) {
        (Some(target), None) => Ok(target),
        (Some(_), Some(_)) => Err(ToolError::new(
            "multiple_fix_targets",
            format!(
                "multiple fix targets found for {}. Inspect shaon_errors for that month first.",
                date.format("%Y-%m-%d")
            ),
        )),
        _ => Err(ToolError::new(
            "fix_target_not_found",
            format!(
                "no fix target found for {}. Inspect shaon_errors for {} first.",
                date.format("%Y-%m-%d"),
                date.format("%Y-%m")
            ),
        )),
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

fn fix_params_from_target(target: &FixTarget) -> Option<ErrorFixParams> {
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

fn fix_params_candidates(targets: &[FixTarget]) -> Vec<ErrorFixParams> {
    targets.iter().filter_map(fix_params_from_target).collect()
}

fn error_day_response(entry: &use_cases::OverviewErrorDay) -> ErrorDayResponse {
    let fix_params_candidates = fix_params_candidates(&entry.fix_targets);
    let fix_params = match fix_params_candidates.as_slice() {
        [candidate] => Some(candidate.clone()),
        _ => None,
    };

    ErrorDayResponse {
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

fn missing_day_from_calendar_day(day: &hr_core::CalendarDay) -> MissingDay {
    MissingDay {
        date: day.date.format("%Y-%m-%d").to_string(),
        day_name: day.day_name.clone(),
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
                    fix_params_from_target(target).map(|params| FixableDay {
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

fn build_errors_response(overview: &use_cases::OverviewData) -> ErrorsResponse {
    ErrorsResponse {
        month: overview.month.format("%Y-%m").to_string(),
        employee_id: overview.calendar.employee_id.clone(),
        error_count: overview.error_days.len(),
        errors: overview.error_days.iter().map(error_day_response).collect(),
    }
}

fn build_overview_response(overview: &use_cases::OverviewData) -> OverviewResponse {
    let missing_dates: BTreeSet<NaiveDate> = overview.missing_days.iter().copied().collect();

    OverviewResponse {
        user: OverviewUser {
            user_id: overview.identity.user_id.clone(),
            employee_id: overview.identity.employee_id.clone(),
            name: overview.identity.display_name.clone(),
            is_manager: overview.identity.is_manager,
        },
        month: overview.month.format("%Y-%m").to_string(),
        summary: OverviewSummary {
            total_work_days: overview.summary.total_work_days,
            reported: overview.summary.reported,
            missing: overview.summary.missing,
            errors: overview.summary.errors,
        },
        attendance_types: overview.attendance_types.clone(),
        error_days: overview.error_days.iter().map(error_day_response).collect(),
        missing_days: overview
            .calendar
            .days
            .iter()
            .filter(|day| missing_dates.contains(&day.date))
            .map(missing_day_from_calendar_day)
            .collect(),
        suggested_actions: overview
            .suggested_actions
            .iter()
            .map(suggested_action_from_plan)
            .collect(),
    }
}

fn report_response_json(
    kind: &str,
    requested: &str,
    table: &hr_core::ReportTable,
) -> serde_json::Value {
    serde_json::json!({
        "report": {
            "kind": kind,
            "requested": requested,
            "provider_name": table.name,
        },
        "column_count": table.headers.len(),
        "row_count": table.rows.len(),
        "columns": table.headers.iter().enumerate().map(|(index, name)| {
            serde_json::json!({
                "index": index,
                "name": name,
            })
        }).collect::<Vec<_>>(),
        "rows": table.rows.iter().enumerate().map(|(index, cells)| {
            serde_json::json!({
                "index": index,
                "cells": cells,
            })
        }).collect::<Vec<_>>(),
    })
}

fn write_bytes_to_path(path: &Path, bytes: &[u8]) -> Result<(), ToolError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| {
            ToolError::new(
                "payslip_write_failed",
                format!("failed to create {}: {e}", parent.display()),
            )
        })?;
    }

    fs::write(path, bytes).map_err(|e| {
        ToolError::new(
            "payslip_write_failed",
            format!("failed to write {}: {e}", path.display()),
        )
    })
}

// ---------------------------------------------------------------------------
// Tool implementations
// ---------------------------------------------------------------------------

#[tool_router]
impl ShaonMcpServer {
    #[tool(
        description = "Get attendance calendar for a month. Returns { month, employee_id, days[] } where each day includes date, day_name, entry_time, exit_time, attendance_type, total_hours, has_error, error_message, and source."
    )]
    async fn shaon_status(&self, Parameters(req): Parameters<MonthParam>) -> String {
        json_or_error(|| async {
            let mut provider = new_provider().await?;
            let month = parse_month(&req.month)?;
            let cal = provider
                .month_calendar(month)
                .await
                .map_err(ToolError::from)?;

            to_json_value(&build_status_response(&cal))
        })
        .await
    }

    #[tool(
        description = "Get attendance errors for a month. Returns { month, employee_id, error_count, errors[] } where each error day includes date, day_name, error_message, fix_params, and fix_params_candidates."
    )]
    async fn shaon_errors(&self, Parameters(req): Parameters<MonthParam>) -> String {
        json_or_error(|| async {
            let mut provider = new_provider().await?;
            let month = parse_month(&req.month)?;
            let overview =
                use_cases::build_overview(&mut provider, month, Local::now().date_naive())
                    .await
                    .map_err(ToolError::from)?;

            to_json_value(&build_errors_response(&overview))
        })
        .await
    }

    #[tool(description = "List available attendance types from the provider cache or live sync.")]
    async fn shaon_types(&self) -> String {
        json_or_error(|| async {
            let mut provider = new_provider().await?;
            let subdomain = provider.client().config().subdomain.clone();
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
        description = "Clock in for today. Defaults to dry-run preview unless execute is true. Human-attested write. Only set execute=true after the user has reviewed the dry-run preview and explicitly confirmed submission."
    )]
    async fn shaon_clock_in(&self, Parameters(req): Parameters<ClockParam>) -> String {
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
        description = "Clock out for today. Defaults to dry-run preview unless execute is true. Human-attested write. Only set execute=true after the user has reviewed the dry-run preview and explicitly confirmed submission."
    )]
    async fn shaon_clock_out(&self, Parameters(req): Parameters<ClockParam>) -> String {
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
        description = "Fill attendance for a date range. Defaults to dry-run preview. Skips weekends (Fri/Sat). Returns one full server-generated preview object per processed day so a client can show the dry-run before approval. Human-attested write. Only set execute=true after the user has reviewed the dry-run preview and explicitly confirmed submission."
    )]
    async fn shaon_fill(&self, Parameters(req): Parameters<FillParam>) -> String {
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
                .map(|(date, preview)| fill_result_json(date, preview))
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
        description = "Automatically fill all missing days in a month. Defaults to dry-run preview. Skips weekends. Human-attested write. Only set execute=true after the user has reviewed the dry-run preview and explicitly confirmed submission."
    )]
    async fn shaon_auto_fill(&self, Parameters(req): Parameters<AutoFillParam>) -> String {
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

    #[tool(
        description = "Resolve a single attendance error day. Auto-detects the provider fix target for the day. Defaults to dry-run preview. Human-attested write. Only set execute=true after the user has reviewed the dry-run preview and explicitly confirmed submission."
    )]
    async fn shaon_resolve(&self, Parameters(req): Parameters<ResolveParam>) -> String {
        json_or_error(|| async {
            let mut provider = new_provider().await?;
            let date = parse_date(&req.date)?;
            let month = date.with_day(1).ok_or_else(|| {
                ToolError::new(
                    "invalid_date",
                    format!("failed to derive month from {}", req.date),
                )
            })?;
            let hours = req.hours.as_deref().map(parse_hours_range).transpose()?;
            let type_code =
                use_cases::resolve_attendance_type(&mut provider, req.r#type.as_deref())
                    .await
                    .map_err(ToolError::from)?
                    .map(|resolved| resolved.code);
            let fix_targets = provider.fix_targets(month).await.map_err(ToolError::from)?;
            let target = find_fix_target_for_date(&fix_targets, date)?;
            let preview = use_cases::fix_day(
                &mut provider,
                &target,
                type_code,
                hours,
                write_mode(req.execute.unwrap_or(false)),
            )
            .await
            .map_err(ToolError::from)?;
            Ok(write_preview_json("resolve", &preview))
        })
        .await
    }

    #[tool(description = "Get salary summary for recent months.")]
    async fn shaon_salary(&self, Parameters(req): Parameters<SalaryParam>) -> String {
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

    #[tool(
        description = "Download a password-protected payslip PDF. Sensitive payroll document. Prefer output_path for local storage; include_bytes is explicit opt-in because MCP responses may be logged."
    )]
    async fn shaon_payslip_download(
        &self,
        Parameters(req): Parameters<PayslipDownloadParam>,
    ) -> String {
        json_or_error(|| async {
            let month = parse_month_or_previous(req.month.as_deref())?;
            let output_path = req.output_path.as_ref().map(PathBuf::from);
            let include_bytes = req.include_bytes.unwrap_or(false);

            if !include_bytes && output_path.is_none() {
                return Err(ToolError::new(
                    "invalid_payslip_request",
                    "set include_bytes=true or provide output_path",
                ));
            }

            if include_bytes {
                let mut client = new_provider().await?.into_inner();
                let bytes = client
                    .password_protected_payslip_bytes(month)
                    .await
                    .map_err(|err| ToolError::new("payslip_download_failed", err.to_string()))?;
                if let Some(path) = output_path.as_deref() {
                    write_bytes_to_path(path, &bytes)?;
                }
                Ok(serde_json::json!({
                    "month": month.format("%Y-%m").to_string(),
                    "mime_type": "application/pdf",
                    "password_protected": true,
                    "size_bytes": bytes.len(),
                    "path": output_path.as_ref().map(|path| path.display().to_string()),
                    "bytes_base64": STANDARD.encode(&bytes),
                }))
            } else {
                let mut provider = new_provider().await?;
                let output = output_path
                    .as_deref()
                    .expect("guard ensures output_path exists when include_bytes=false");
                let download = provider
                    .download_payslip(month, Some(output))
                    .await
                    .map_err(ToolError::from)?;
                Ok(serde_json::json!({
                    "month": download.month.format("%Y-%m").to_string(),
                    "mime_type": "application/pdf",
                    "password_protected": true,
                    "size_bytes": download.size_bytes,
                    "path": download.path.display().to_string(),
                }))
            }
        })
        .await
    }

    #[tool(description = "Show the analyzed attendance sheet as a stable report table schema.")]
    async fn shaon_sheet(&self) -> String {
        json_or_error(|| async {
            let mut provider = new_provider().await?;
            let table = provider
                .report(ReportSpec::Path(SHEET_REPORT_PATH.to_string()))
                .await
                .map_err(ToolError::from)?;
            Ok(report_response_json("sheet", "sheet", &table))
        })
        .await
    }

    #[tool(description = "Show the attendance correction log as a stable report table schema.")]
    async fn shaon_corrections(&self) -> String {
        json_or_error(|| async {
            let mut provider = new_provider().await?;
            let table = provider
                .report(ReportSpec::Path(CORRECTIONS_REPORT_PATH.to_string()))
                .await
                .map_err(ToolError::from)?;
            Ok(report_response_json("corrections", "corrections", &table))
        })
        .await
    }

    #[tool(description = "Show absences initial data (attendance symbols and display names).")]
    async fn shaon_absences(&self) -> String {
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
        description = "Get overview for a month. Returns { user, month, summary, attendance_types, error_days, missing_days, suggested_actions }. missing_days is an array of { date, day_name }. suggested_actions is a tagged union keyed by kind, with action fields at the top level rather than nested under params."
    )]
    async fn shaon_overview(&self, Parameters(req): Parameters<OverviewParam>) -> String {
        json_or_error(|| async {
            let mut provider = new_provider().await?;
            let month = parse_month_or_current(req.month.as_deref())?;
            let overview =
                use_cases::build_overview(&mut provider, month, Local::now().date_naive())
                    .await
                    .map_err(ToolError::from)?;

            to_json_value(&build_overview_response(&overview))
        })
        .await
    }
}

#[tool_handler]
impl ServerHandler for ShaonMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build()).with_instructions(
            "Shaon attendance & payslip server. \
                 Read tools return JSON data. \
                 Write tools (clock_in, clock_out, fill, auto_fill, resolve) default to dry-run; \
                 set execute=true to submit.",
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hr_core::{AttendanceSource, CalendarDay, MonthCalendar, UserIdentity};
    use std::collections::BTreeMap;

    fn sample_overview(targets: Vec<FixTarget>) -> use_cases::OverviewData {
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
    fn build_overview_response_uses_structured_missing_days_and_actions() {
        let date = NaiveDate::from_ymd_opt(2026, 4, 10).unwrap();
        let overview = sample_overview(vec![FixTarget {
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
}
