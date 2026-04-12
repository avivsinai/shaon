use anyhow::Result;
use hr_core::{
    AbsenceSymbol, AttendanceChange, AttendanceType, DocumentDownload, FixTarget, MonthCalendar,
    ReportTable, SalaryEntry, SalarySummary, UserIdentity, WritePreview,
};
use std::collections::BTreeMap;

pub mod api;
pub mod attendance;
pub mod client;
pub mod config;
pub mod ontology;
pub mod payslip;
pub mod provider;
pub mod reports;

pub use config::Config;
pub use provider::HilanProvider;

pub fn build_provider(config: Config) -> Result<HilanProvider> {
    HilanProvider::new(config)
}

pub async fn build_authenticated_provider(config: Config) -> Result<HilanProvider> {
    let mut provider = build_provider(config)?;
    provider.client_mut().ensure_authenticated().await?;
    Ok(provider)
}

impl From<api::BootstrapInfo> for UserIdentity {
    fn from(value: api::BootstrapInfo) -> Self {
        Self {
            user_id: value.user_id,
            employee_id: value.employee_id.to_string(),
            display_name: value.name,
            is_manager: value.is_manager,
        }
    }
}

impl From<ontology::AttendanceType> for AttendanceType {
    fn from(value: ontology::AttendanceType) -> Self {
        Self {
            code: value.code,
            name_he: value.name_he,
            name_en: value.name_en,
        }
    }
}

impl From<api::AbsenceSymbol> for AbsenceSymbol {
    fn from(value: api::AbsenceSymbol) -> Self {
        Self {
            id: value.id,
            name: value.name,
            display_name: value.display_name,
        }
    }
}

impl From<attendance::CalendarDay> for hr_core::CalendarDay {
    fn from(value: attendance::CalendarDay) -> Self {
        Self {
            date: value.date,
            day_name: value.day_name,
            has_error: value.has_error,
            error_message: value.error_message,
            entry_time: value.entry_time,
            exit_time: value.exit_time,
            attendance_type: value.attendance_type,
            total_hours: value.total_hours,
            source: value.source,
        }
    }
}

impl From<attendance::MonthCalendar> for MonthCalendar {
    fn from(value: attendance::MonthCalendar) -> Self {
        Self {
            month: value.month,
            employee_id: value.employee_id,
            days: value.days.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<attendance::AttendanceSubmit> for AttendanceChange {
    fn from(value: attendance::AttendanceSubmit) -> Self {
        Self {
            date: value.date,
            attendance_type_code: value.attendance_type_code,
            use_default_attendance_type: value.default_work_day,
            entry_time: value.entry_time,
            exit_time: value.exit_time,
            comment: value.comment,
            clear_entry: value.clear_entry,
            clear_exit: value.clear_exit,
            clear_comment: value.clear_comment,
        }
    }
}

impl From<api::ErrorTask> for FixTarget {
    fn from(value: api::ErrorTask) -> Self {
        let api::ErrorTask {
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

impl From<client::SalaryEntry> for SalaryEntry {
    fn from(value: client::SalaryEntry) -> Self {
        Self {
            month: value.month,
            amount: value.amount,
        }
    }
}

impl From<client::SalarySummary> for SalarySummary {
    fn from(value: client::SalarySummary) -> Self {
        Self {
            label: value.label,
            entries: value.entries.into_iter().map(Into::into).collect(),
            percent_diff: value.percent_diff,
        }
    }
}

impl From<client::PayslipDownload> for DocumentDownload {
    fn from(value: client::PayslipDownload) -> Self {
        Self {
            month: value.month,
            path: value.path,
            size_bytes: value.size_bytes,
        }
    }
}

impl From<reports::ReportTable> for ReportTable {
    fn from(value: reports::ReportTable) -> Self {
        Self {
            name: String::new(),
            headers: value.headers,
            rows: value.rows,
        }
    }
}

impl From<attendance::SubmitPreview> for WritePreview {
    fn from(value: attendance::SubmitPreview) -> Self {
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
