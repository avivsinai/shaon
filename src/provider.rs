use anyhow::Result;
use async_trait::async_trait;
use chrono::{Datelike, NaiveDate};
use std::path::Path;

use crate::api;
use crate::attendance::{self, AttendanceSubmit};
use crate::client::HilanClient;
use crate::config::Config;
use crate::core::{
    AbsenceProvider, AbsenceSymbol, AttendanceChange, AttendanceProvider, DocumentDownload,
    FixTarget, MonthCalendar, PayslipProvider, ProviderCapabilities, ProviderError, ReportProvider,
    ReportSpec, ReportTable, SalaryProvider, SalarySummary, UserIdentity, WriteMode, WritePreview,
};
use crate::ontology;
use crate::reports;

pub struct HilanProvider {
    client: HilanClient,
}

impl HilanProvider {
    pub fn new(config: Config) -> Result<Self> {
        Ok(Self::from_client(HilanClient::new(config)?))
    }

    pub fn from_client(client: HilanClient) -> Self {
        Self { client }
    }

    pub fn client(&self) -> &HilanClient {
        &self.client
    }

    pub fn client_mut(&mut self) -> &mut HilanClient {
        &mut self.client
    }

    pub fn into_inner(self) -> HilanClient {
        self.client
    }

    pub fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            attendance_read: true,
            attendance_write: true,
            fix_errors: true,
            salary_summary: true,
            payslips: true,
            reports: true,
            attendance_types: true,
        }
    }

    fn change_to_submit(change: &AttendanceChange) -> AttendanceSubmit {
        AttendanceSubmit {
            date: change.date,
            attendance_type_code: change.attendance_type_code.clone(),
            entry_time: change.entry_time.clone(),
            exit_time: change.exit_time.clone(),
            comment: change.comment.clone(),
            clear_entry: change.clear_entry,
            clear_exit: change.clear_exit,
            clear_comment: change.clear_comment,
            default_work_day: change.use_default_attendance_type,
        }
    }

    fn fix_params(target: &FixTarget) -> Result<(String, String), ProviderError> {
        let report_id = target.metadata.get("report_id").cloned();
        let error_type = target.metadata.get("error_type").cloned();

        match (report_id, error_type) {
            (Some(report_id), Some(error_type)) => Ok((report_id, error_type)),
            _ => target
                .provider_ref
                .split_once(':')
                .map(|(report_id, error_type)| (report_id.to_string(), error_type.to_string()))
                .ok_or_else(|| {
                    ProviderError::new(
                        "invalid_fix_target",
                        format!(
                            "fix target for {} is missing Hilan error parameters",
                            target.date.format("%Y-%m-%d")
                        ),
                    )
                    .with_details(serde_json::json!({
                        "provider_ref": target.provider_ref,
                        "metadata": target.metadata,
                    }))
                }),
        }
    }

    fn preview_with_summary(
        preview: attendance::SubmitPreview,
        action: &str,
        date: NaiveDate,
    ) -> WritePreview {
        let mut preview: WritePreview = preview.into();
        preview.summary = if preview.executed {
            format!("{action} {}", date.format("%Y-%m-%d"))
        } else {
            format!("dry run: {action} {}", date.format("%Y-%m-%d"))
        };
        preview
    }

    fn provider_error(code: &'static str, err: anyhow::Error) -> ProviderError {
        ProviderError::new(code, err.to_string())
    }
}

#[async_trait]
impl AttendanceProvider for HilanProvider {
    async fn identity(&mut self) -> Result<UserIdentity, ProviderError> {
        api::bootstrap(&mut self.client)
            .await
            .map(Into::into)
            .map_err(|err| Self::provider_error("identity_failed", err))
    }

    async fn month_calendar(&mut self, month: NaiveDate) -> Result<MonthCalendar, ProviderError> {
        attendance::read_calendar(&mut self.client, month)
            .await
            .map(Into::into)
            .map_err(|err| Self::provider_error("calendar_read_failed", err))
    }

    async fn attendance_types(
        &mut self,
    ) -> Result<Vec<crate::core::AttendanceType>, ProviderError> {
        let subdomain = self.client.config().subdomain.clone();
        ontology::OrgOntology::load_or_sync(&mut self.client, &subdomain)
            .await
            .map(|ontology| ontology.types.into_iter().map(Into::into).collect())
            .map_err(|err| Self::provider_error("attendance_types_failed", err))
    }

    async fn fix_targets(&mut self, month: NaiveDate) -> Result<Vec<FixTarget>, ProviderError> {
        api::get_error_tasks(&mut self.client)
            .await
            .map(|tasks| {
                let mut targets: Vec<_> = tasks
                    .into_iter()
                    .filter(|task| {
                        task.date.year() == month.year() && task.date.month() == month.month()
                    })
                    .map(Into::into)
                    .collect();
                targets.sort();
                targets
            })
            .map_err(|err| Self::provider_error("fix_targets_failed", err))
    }

    async fn submit_day(
        &mut self,
        change: &AttendanceChange,
        mode: WriteMode,
    ) -> Result<WritePreview, ProviderError> {
        let submit = Self::change_to_submit(change);
        attendance::submit_day(&mut self.client, &submit, mode.should_execute())
            .await
            .map(|preview| {
                Self::preview_with_summary(preview, "submitted attendance for", change.date)
            })
            .map_err(|err| Self::provider_error("attendance_submit_failed", err))
    }

    async fn fix_day(
        &mut self,
        target: &FixTarget,
        change: &AttendanceChange,
        mode: WriteMode,
    ) -> Result<WritePreview, ProviderError> {
        let submit = Self::change_to_submit(change);
        let (report_id, error_type) = Self::fix_params(target)?;
        attendance::fix_error_day(
            &mut self.client,
            &submit,
            &report_id,
            &error_type,
            mode.should_execute(),
        )
        .await
        .map(|preview| {
            Self::preview_with_summary(preview, "fixed attendance error for", change.date)
        })
        .map_err(|err| Self::provider_error("attendance_fix_failed", err))
    }
}

#[async_trait]
impl SalaryProvider for HilanProvider {
    async fn salary_summary(&mut self, months: u32) -> Result<SalarySummary, ProviderError> {
        self.client
            .salary(months)
            .await
            .map(Into::into)
            .map_err(|err| Self::provider_error("salary_summary_failed", err))
    }
}

#[async_trait]
impl AbsenceProvider for HilanProvider {
    async fn absence_symbols(&mut self) -> Result<Vec<AbsenceSymbol>, ProviderError> {
        api::get_absences_initial(&mut self.client)
            .await
            .map(|data| data.symbols.into_iter().map(Into::into).collect())
            .map_err(|err| Self::provider_error("absence_symbols_failed", err))
    }
}

#[async_trait]
impl PayslipProvider for HilanProvider {
    async fn download_payslip(
        &mut self,
        month: NaiveDate,
        output: Option<&Path>,
    ) -> Result<DocumentDownload, ProviderError> {
        self.client
            .payslip(month, output)
            .await
            .map(Into::into)
            .map_err(|err| Self::provider_error("payslip_download_failed", err))
    }
}

#[async_trait]
impl ReportProvider for HilanProvider {
    async fn report(&mut self, spec: ReportSpec) -> Result<ReportTable, ProviderError> {
        match spec {
            ReportSpec::Named(name) => reports::fetch_report(&mut self.client, &name)
                .await
                .map(|table| ReportTable {
                    name,
                    headers: table.headers,
                    rows: table.rows,
                })
                .map_err(|err| Self::provider_error("report_fetch_failed", err)),
            ReportSpec::Path(path) => reports::fetch_table_from_url(&mut self.client, &path)
                .await
                .map(|table| ReportTable {
                    name: path,
                    headers: table.headers,
                    rows: table.rows,
                })
                .map_err(|err| Self::provider_error("report_fetch_failed", err)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn change_to_submit_keeps_semantic_fields_and_disables_default_work_day() {
        let change = AttendanceChange {
            date: NaiveDate::from_ymd_opt(2026, 4, 10).unwrap(),
            attendance_type_code: Some("0".to_string()),
            use_default_attendance_type: false,
            entry_time: Some("09:00".to_string()),
            exit_time: Some("18:00".to_string()),
            comment: Some("office".to_string()),
            clear_entry: false,
            clear_exit: false,
            clear_comment: false,
        };

        let submit = HilanProvider::change_to_submit(&change);

        assert_eq!(submit.date, change.date);
        assert_eq!(submit.attendance_type_code, change.attendance_type_code);
        assert_eq!(submit.entry_time, change.entry_time);
        assert_eq!(submit.exit_time, change.exit_time);
        assert_eq!(submit.comment, change.comment);
        assert!(!submit.default_work_day);
    }

    #[test]
    fn fix_params_prefer_explicit_metadata() {
        let target = FixTarget {
            date: NaiveDate::from_ymd_opt(2026, 4, 10).unwrap(),
            issue_kind: Some("missing".to_string()),
            provider_ref: "ignored:ignored".to_string(),
            metadata: BTreeMap::from([
                ("report_id".to_string(), "report-1".to_string()),
                ("error_type".to_string(), "63".to_string()),
            ]),
        };

        let params = HilanProvider::fix_params(&target).expect("fix params");

        assert_eq!(params, ("report-1".to_string(), "63".to_string()));
    }

    #[test]
    fn fix_params_fall_back_to_provider_ref() {
        let target = FixTarget {
            date: NaiveDate::from_ymd_opt(2026, 4, 10).unwrap(),
            issue_kind: None,
            provider_ref: "report-2:18".to_string(),
            metadata: BTreeMap::new(),
        };

        let params = HilanProvider::fix_params(&target).expect("fix params");

        assert_eq!(params, ("report-2".to_string(), "18".to_string()));
    }

    #[test]
    fn invalid_fix_target_returns_structured_error() {
        let target = FixTarget {
            date: NaiveDate::from_ymd_opt(2026, 4, 10).unwrap(),
            issue_kind: None,
            provider_ref: "broken".to_string(),
            metadata: BTreeMap::new(),
        };

        let err = HilanProvider::fix_params(&target).expect_err("invalid fix target");

        assert_eq!(err.code, "invalid_fix_target");
        assert!(err.message.contains("2026-04-10"));
        assert!(err.details.is_some());
    }
}
