use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::{Datelike, NaiveDate};
use hr_core::{
    AbsenceProvider, AbsenceSymbol, AttendanceChange, AttendanceProvider, DocumentDownload,
    FixTarget, MonthCalendar, PayslipProvider, ProviderError, ReportProvider, ReportSpec,
    ReportTable, SalaryProvider, SalarySummary, UserIdentity, WriteMode, WritePreview,
};
use std::path::Path;

use crate::api;
use crate::attendance::{self, AttendanceSubmit};
use crate::client::HilanClient;
use crate::config::Config;
use crate::ontology;
use crate::reports;

pub struct HilanProvider {
    client: HilanClient,
}

const MISSING_REPORT_ERROR_TYPE: &str = "63";
const WORK_FROM_HOME_TYPE_CODE: &str = "120";
const WORK_DAY_TYPE_CODE: &str = "0";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FixStrategy {
    ErrorWizardOnly,
    ErrorWizardThenCalendar,
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

    fn fix_strategy(error_type: &str, submit: &AttendanceSubmit) -> FixStrategy {
        if error_type == MISSING_REPORT_ERROR_TYPE
            && submit.attendance_type_code.as_deref() == Some(WORK_FROM_HOME_TYPE_CODE)
        {
            FixStrategy::ErrorWizardThenCalendar
        } else {
            FixStrategy::ErrorWizardOnly
        }
    }

    fn error_clear_submit(submit: &AttendanceSubmit) -> AttendanceSubmit {
        let mut clear_submit = submit.clone();
        clear_submit.attendance_type_code = Some(WORK_DAY_TYPE_CODE.to_string());
        clear_submit.default_work_day = false;
        clear_submit
    }

    fn preview_with_steps(
        steps: &[(&'static str, attendance::SubmitPreview)],
        action: &str,
        date: NaiveDate,
    ) -> WritePreview {
        let final_step = &steps[steps.len() - 1].1;
        let step_refs: Vec<(&str, &attendance::SubmitPreview)> = steps
            .iter()
            .map(|(label, preview)| (*label, preview))
            .collect();
        let payload_display = attendance::render_step_list(&step_refs);

        WritePreview {
            executed: final_step.executed,
            summary: if final_step.executed {
                format!("{action} {}", date.format("%Y-%m-%d"))
            } else {
                format!("dry run: {action} {}", date.format("%Y-%m-%d"))
            },
            provider_debug: Some(serde_json::json!({
                "url": final_step.url,
                "button_name": final_step.button_name,
                "button_value": final_step.button_value,
                "employee_id": final_step.employee_id,
                "payload_display": payload_display,
                "steps": steps.iter().map(|(label, preview)| serde_json::json!({
                    "label": label,
                    "url": preview.url,
                    "button_name": preview.button_name,
                    "button_value": preview.button_value,
                    "employee_id": preview.employee_id,
                    "payload_display": preview.payload_display,
                })).collect::<Vec<_>>(),
            })),
        }
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

    async fn attendance_types(&mut self) -> Result<Vec<hr_core::AttendanceType>, ProviderError> {
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
        match Self::fix_strategy(&error_type, &submit) {
            FixStrategy::ErrorWizardOnly => attendance::fix_error_day(
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
            .map_err(|err| Self::provider_error("attendance_fix_failed", err)),
            FixStrategy::ErrorWizardThenCalendar => {
                let clear_submit = Self::error_clear_submit(&submit);
                let mut clear_preview = attendance::fix_error_day(
                    &mut self.client,
                    &clear_submit,
                    &report_id,
                    &error_type,
                    mode.should_execute(),
                )
                .await
                .with_context(|| {
                    format!(
                        "clear Hilan missing-report error before applying work from home for {}",
                        change.date.format("%Y-%m-%d")
                    )
                })
                .map_err(|err| Self::provider_error("attendance_fix_failed", err))?;

                if !mode.should_execute() {
                    clear_preview.payload_display.push_str(
                        "\n\nNote: the follow-up calendar delete/apply steps depend on the post-clear Hilan state and are determined only during --execute.",
                    );
                    let steps = [("clear the Hilan error via the error wizard", clear_preview)];
                    return Ok(Self::preview_with_steps(
                        &steps,
                        "fixed attendance error for",
                        change.date,
                    ));
                }

                let mut steps = vec![("clear the Hilan error via the error wizard", clear_preview)];
                let mut calendar_context =
                    attendance::load_calendar_submit_context(&mut self.client, change.date)
                        .await
                        .with_context(|| {
                            format!(
                                "load calendar submit context after clearing error for {}",
                                change.date.format("%Y-%m-%d")
                            )
                        })
                        .map_err(|err| Self::provider_error("attendance_fix_failed", err))?;

                let desired_type_code = submit
                    .attendance_type_code
                    .as_deref()
                    .expect("work from home fix requires an attendance type");
                let (delete_previews, refreshed_context) =
                    attendance::delete_conflicting_absence_reports(
                        &mut self.client,
                        calendar_context,
                        desired_type_code,
                        mode.should_execute(),
                    )
                    .await
                    .with_context(|| {
                        format!(
                            "delete conflicting existing attendance before applying requested type for {}",
                            change.date.format("%Y-%m-%d")
                        )
                    })
                    .map_err(|err| Self::provider_error("attendance_fix_failed", err))?;
                steps.extend(delete_previews.into_iter().map(|preview| {
                    (
                        "delete the conflicting calendar row before applying the requested attendance",
                        preview,
                    )
                }));
                calendar_context = refreshed_context;

                let skip_submit =
                    attendance::has_matching_report(&calendar_context, desired_type_code);

                if !skip_submit {
                    let submit_preview = attendance::submit_day_with_context(
                        &mut self.client,
                        &submit,
                        mode.should_execute(),
                        calendar_context,
                    )
                    .await
                    .with_context(|| {
                        format!(
                            "apply requested attendance via calendar page after clearing error for {}",
                            change.date.format("%Y-%m-%d")
                        )
                    })
                    .map_err(|err| Self::provider_error("attendance_fix_failed", err))?;
                    steps.push((
                        "apply the requested attendance via the calendar page",
                        submit_preview,
                    ));
                }

                Ok(Self::preview_with_steps(
                    &steps,
                    "fixed attendance error for",
                    change.date,
                ))
            }
        }
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

    #[test]
    fn missing_report_wfh_uses_two_step_fix_strategy() {
        let submit = AttendanceSubmit {
            date: NaiveDate::from_ymd_opt(2026, 4, 9).unwrap(),
            attendance_type_code: Some(WORK_FROM_HOME_TYPE_CODE.to_string()),
            entry_time: Some("09:00".to_string()),
            exit_time: Some("18:00".to_string()),
            comment: None,
            clear_entry: false,
            clear_exit: false,
            clear_comment: false,
            default_work_day: false,
        };

        assert_eq!(
            HilanProvider::fix_strategy(MISSING_REPORT_ERROR_TYPE, &submit),
            FixStrategy::ErrorWizardThenCalendar
        );
    }

    #[test]
    fn non_wfh_fixes_stay_on_error_wizard() {
        let submit = AttendanceSubmit {
            date: NaiveDate::from_ymd_opt(2026, 4, 9).unwrap(),
            attendance_type_code: Some("481".to_string()),
            entry_time: None,
            exit_time: None,
            comment: None,
            clear_entry: false,
            clear_exit: false,
            clear_comment: false,
            default_work_day: false,
        };

        assert_eq!(
            HilanProvider::fix_strategy(MISSING_REPORT_ERROR_TYPE, &submit),
            FixStrategy::ErrorWizardOnly
        );
        assert_eq!(
            HilanProvider::fix_strategy("18", &submit),
            FixStrategy::ErrorWizardOnly
        );
    }

    #[test]
    fn error_clear_submit_rewrites_work_from_home_to_work_day() {
        let submit = AttendanceSubmit {
            date: NaiveDate::from_ymd_opt(2026, 4, 9).unwrap(),
            attendance_type_code: Some(WORK_FROM_HOME_TYPE_CODE.to_string()),
            entry_time: Some("09:00".to_string()),
            exit_time: Some("18:00".to_string()),
            comment: Some("home".to_string()),
            clear_entry: false,
            clear_exit: false,
            clear_comment: false,
            default_work_day: false,
        };

        let cleared = HilanProvider::error_clear_submit(&submit);

        assert_eq!(
            cleared.attendance_type_code.as_deref(),
            Some(WORK_DAY_TYPE_CODE)
        );
        assert_eq!(cleared.entry_time, submit.entry_time);
        assert_eq!(cleared.exit_time, submit.exit_time);
        assert_eq!(cleared.comment, submit.comment);
        assert!(!cleared.default_work_day);
    }
}
