use async_trait::async_trait;
use chrono::{Datelike, NaiveDate};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;
use std::path::{Path, PathBuf};

pub mod use_cases;

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
pub struct AbsenceSymbol {
    pub id: String,
    pub name: String,
    pub display_name: Option<String>,
}

/// How the attendance data for a day was determined.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum AttendanceSource {
    /// The user explicitly reported this day (chose a type, entered times, etc.).
    UserReported,
    /// The system auto-filled this day (typically as vacation) because the user
    /// didn't report.
    SystemAutoFill,
    /// A holiday or day-off set by the organization, not by the user.
    Holiday,
    /// No attendance data at all — the day is truly unreported.
    #[default]
    Unreported,
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
    /// How this day's data was determined (user-reported, system auto-fill, etc.).
    #[serde(default)]
    pub source: AttendanceSource,
}

impl CalendarDay {
    /// Returns true if the user actively reported this day.
    pub fn is_reported(&self) -> bool {
        self.source == AttendanceSource::UserReported || self.source == AttendanceSource::Holiday
    }

    /// Returns true if the system auto-filled this day (user didn't report).
    pub fn is_auto_filled(&self) -> bool {
        self.source == AttendanceSource::SystemAutoFill
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
    pub use_default_attendance_type: bool,
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

impl WritePreview {
    pub fn debug_field(&self, key: &str) -> Option<&str> {
        self.provider_debug
            .as_ref()
            .and_then(|debug| debug.get(key))
            .and_then(serde_json::Value::as_str)
    }
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

pub fn is_weekend(date: NaiveDate) -> bool {
    matches!(date.weekday(), chrono::Weekday::Fri | chrono::Weekday::Sat)
}

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
pub trait AbsenceProvider: Send {
    async fn absence_symbols(&mut self) -> Result<Vec<AbsenceSymbol>, ProviderError>;
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
