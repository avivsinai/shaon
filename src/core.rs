use async_trait::async_trait;
use chrono::{Datelike, NaiveDate};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UserIdentity {
    pub user_id: String,
    pub employee_id: String,
    pub display_name: String,
    pub is_manager: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AttendanceType {
    pub code: String,
    pub name_he: String,
    pub name_en: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
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

impl CalendarDay {
    pub fn is_reported(&self) -> bool {
        self.entry_time.is_some() || self.attendance_type.is_some()
    }

    pub fn is_work_day(&self) -> bool {
        self.date.weekday().num_days_from_sunday() < 5
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MonthCalendar {
    pub month: NaiveDate,
    pub employee_id: String,
    pub days: Vec<CalendarDay>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
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

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum WriteMode {
    DryRun,
    Execute,
}

impl WriteMode {
    pub fn should_execute(self) -> bool {
        matches!(self, Self::Execute)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WritePreview {
    pub executed: bool,
    pub summary: String,
    pub provider_debug: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct FixTarget {
    pub date: NaiveDate,
    pub issue_kind: Option<String>,
    pub provider_ref: String,
    pub metadata: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SalaryEntry {
    pub month: NaiveDate,
    pub amount: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SalarySummary {
    pub label: String,
    pub entries: Vec<SalaryEntry>,
    pub percent_diff: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DocumentDownload {
    pub month: NaiveDate,
    pub path: PathBuf,
    pub size_bytes: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ReportSpec {
    Named(String),
    Path(String),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReportTable {
    pub name: String,
    pub headers: Vec<String>,
    pub rows: Vec<Vec<String>>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ProviderCapabilities {
    pub attendance_read: bool,
    pub attendance_write: bool,
    pub fix_errors: bool,
    pub salary_summary: bool,
    pub payslips: bool,
    pub reports: bool,
    pub attendance_types: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProviderError {
    pub code: Cow<'static, str>,
    pub message: String,
    pub retryable: bool,
    pub details: Option<serde_json::Value>,
}

impl ProviderError {
    pub fn new(code: impl Into<Cow<'static, str>>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            retryable: false,
            details: None,
        }
    }

    pub fn retryable(mut self, retryable: bool) -> Self {
        self.retryable = retryable;
        self
    }

    pub fn with_details(mut self, details: serde_json::Value) -> Self {
        self.details = Some(details);
        self
    }
}

impl fmt::Display for ProviderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl Error for ProviderError {}

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

impl From<crate::api::BootstrapInfo> for UserIdentity {
    fn from(value: crate::api::BootstrapInfo) -> Self {
        Self {
            user_id: value.user_id,
            employee_id: value.employee_id.to_string(),
            display_name: value.name,
            is_manager: value.is_manager,
        }
    }
}

impl From<crate::ontology::AttendanceType> for AttendanceType {
    fn from(value: crate::ontology::AttendanceType) -> Self {
        Self {
            code: value.code,
            name_he: value.name_he,
            name_en: value.name_en,
        }
    }
}

impl From<crate::attendance::CalendarDay> for CalendarDay {
    fn from(value: crate::attendance::CalendarDay) -> Self {
        Self {
            date: value.date,
            day_name: value.day_name,
            has_error: value.has_error,
            error_message: value.error_message,
            entry_time: value.entry_time,
            exit_time: value.exit_time,
            attendance_type: value.attendance_type,
            total_hours: value.total_hours,
        }
    }
}

impl From<crate::attendance::MonthCalendar> for MonthCalendar {
    fn from(value: crate::attendance::MonthCalendar) -> Self {
        Self {
            month: value.month,
            employee_id: value.employee_id,
            days: value.days.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<crate::attendance::AttendanceSubmit> for AttendanceChange {
    fn from(value: crate::attendance::AttendanceSubmit) -> Self {
        Self {
            date: value.date,
            attendance_type_code: value.attendance_type_code,
            entry_time: value.entry_time,
            exit_time: value.exit_time,
            comment: value.comment,
            clear_entry: value.clear_entry,
            clear_exit: value.clear_exit,
            clear_comment: value.clear_comment,
        }
    }
}

impl From<crate::api::ErrorTask> for FixTarget {
    fn from(value: crate::api::ErrorTask) -> Self {
        let crate::api::ErrorTask {
            date,
            report_id,
            error_type,
        } = value;
        Self {
            date,
            issue_kind: None,
            provider_ref: format!("{report_id}:{error_type}"),
            metadata: BTreeMap::from([
                ("report_id".to_string(), report_id),
                ("error_type".to_string(), error_type),
            ]),
        }
    }
}

impl From<crate::client::SalaryEntry> for SalaryEntry {
    fn from(value: crate::client::SalaryEntry) -> Self {
        Self {
            month: value.month,
            amount: value.amount,
        }
    }
}

impl From<crate::client::SalarySummary> for SalarySummary {
    fn from(value: crate::client::SalarySummary) -> Self {
        Self {
            label: value.label,
            entries: value.entries.into_iter().map(Into::into).collect(),
            percent_diff: value.percent_diff,
        }
    }
}

impl From<crate::client::PayslipDownload> for DocumentDownload {
    fn from(value: crate::client::PayslipDownload) -> Self {
        Self {
            month: value.month,
            path: value.path,
            size_bytes: value.size_bytes,
        }
    }
}

impl From<crate::reports::ReportTable> for ReportTable {
    fn from(value: crate::reports::ReportTable) -> Self {
        Self {
            name: String::new(),
            headers: value.headers,
            rows: value.rows,
        }
    }
}

impl From<crate::attendance::SubmitPreview> for WritePreview {
    fn from(value: crate::attendance::SubmitPreview) -> Self {
        Self {
            executed: value.executed,
            summary: if value.executed {
                "executed".to_string()
            } else {
                "dry_run".to_string()
            },
            provider_debug: Some(serde_json::json!({
                "url": value.url,
                "button_name": value.button_name,
                "button_value": value.button_value,
                "employee_id": value.employee_id,
                "payload_display": value.payload_display,
            })),
        }
    }
}
