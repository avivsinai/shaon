use chrono::NaiveDate;
use shaon::core::{
    AbsenceProvider, AbsenceSymbol, AttendanceChange, AttendanceProvider, AttendanceType,
    CalendarDay, DocumentDownload, FixTarget, MonthCalendar, PayslipProvider, ProviderError,
    ReportProvider, ReportSpec, ReportTable, SalaryEntry, SalaryProvider, SalarySummary,
    UserIdentity, WriteMode, WritePreview,
};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

struct DummyProvider;

#[async_trait::async_trait]
impl AttendanceProvider for DummyProvider {
    async fn identity(&mut self) -> Result<UserIdentity, ProviderError> {
        Ok(UserIdentity {
            user_id: "u-1".to_string(),
            employee_id: "e-1".to_string(),
            display_name: "Test User".to_string(),
            is_manager: false,
        })
    }

    async fn month_calendar(&mut self, month: NaiveDate) -> Result<MonthCalendar, ProviderError> {
        Ok(MonthCalendar {
            month,
            employee_id: "e-1".to_string(),
            days: vec![CalendarDay {
                date: month,
                day_name: "Sun".to_string(),
                has_error: true,
                error_message: Some("missing report".to_string()),
                entry_time: None,
                exit_time: None,
                attendance_type: Some("work day".to_string()),
                total_hours: None,
            }],
        })
    }

    async fn attendance_types(&mut self) -> Result<Vec<AttendanceType>, ProviderError> {
        Ok(vec![AttendanceType {
            code: "0".to_string(),
            name_he: "יום עבודה".to_string(),
            name_en: Some("work day".to_string()),
        }])
    }

    async fn fix_targets(&mut self, month: NaiveDate) -> Result<Vec<FixTarget>, ProviderError> {
        Ok(vec![FixTarget {
            date: month,
            issue_kind: Some("missing_standard_day".to_string()),
            provider_ref: "fix-1".to_string(),
            metadata: BTreeMap::from([
                (
                    "report_id".to_string(),
                    "00000000-0000-0000-0000-000000000000".to_string(),
                ),
                ("error_type".to_string(), "63".to_string()),
            ]),
        }])
    }

    async fn submit_day(
        &mut self,
        _change: &AttendanceChange,
        mode: WriteMode,
    ) -> Result<WritePreview, ProviderError> {
        Ok(WritePreview {
            executed: matches!(mode, WriteMode::Execute),
            summary: "submit".to_string(),
            provider_debug: None,
        })
    }

    async fn fix_day(
        &mut self,
        _target: &FixTarget,
        _change: &AttendanceChange,
        mode: WriteMode,
    ) -> Result<WritePreview, ProviderError> {
        Ok(WritePreview {
            executed: matches!(mode, WriteMode::Execute),
            summary: "fix".to_string(),
            provider_debug: None,
        })
    }
}

#[async_trait::async_trait]
impl SalaryProvider for DummyProvider {
    async fn salary_summary(&mut self, months: u32) -> Result<SalarySummary, ProviderError> {
        let base_month = NaiveDate::from_ymd_opt(2026, 4, 1).unwrap();
        Ok(SalarySummary {
            label: "Bruto".to_string(),
            entries: (0..months)
                .map(|offset| SalaryEntry {
                    month: base_month - chrono::Months::new(offset),
                    amount: 10_000 + u64::from(offset),
                })
                .collect(),
            percent_diff: Some(0.1),
        })
    }
}

#[async_trait::async_trait]
impl AbsenceProvider for DummyProvider {
    async fn absence_symbols(&mut self) -> Result<Vec<AbsenceSymbol>, ProviderError> {
        Ok(vec![AbsenceSymbol {
            id: "481".to_string(),
            name: "חופשה".to_string(),
            display_name: Some("Vacation".to_string()),
        }])
    }
}

#[async_trait::async_trait]
impl PayslipProvider for DummyProvider {
    async fn download_payslip(
        &mut self,
        month: NaiveDate,
        output: Option<&Path>,
    ) -> Result<DocumentDownload, ProviderError> {
        Ok(DocumentDownload {
            month,
            path: output
                .map(Path::to_path_buf)
                .unwrap_or_else(|| PathBuf::from("payslip.pdf")),
            size_bytes: 42,
        })
    }
}

#[async_trait::async_trait]
impl ReportProvider for DummyProvider {
    async fn report(&mut self, spec: ReportSpec) -> Result<ReportTable, ProviderError> {
        let name = match spec {
            ReportSpec::Named(name) | ReportSpec::Path(name) => name,
        };
        Ok(ReportTable {
            name,
            headers: vec!["A".to_string()],
            rows: vec![vec!["1".to_string()]],
        })
    }
}

#[tokio::test]
async fn core_traits_are_usable_from_library_consumers() {
    let mut provider = DummyProvider;
    let month = NaiveDate::from_ymd_opt(2026, 4, 1).unwrap();

    let identity = provider.identity().await.expect("identity");
    assert_eq!(identity.display_name, "Test User");

    let calendar = provider.month_calendar(month).await.expect("calendar");
    assert_eq!(calendar.days.len(), 1);
    assert!(calendar.days[0].has_error);

    let fix_targets = provider.fix_targets(month).await.expect("fix targets");
    assert_eq!(fix_targets[0].provider_ref, "fix-1");

    let change = AttendanceChange {
        date: month,
        attendance_type_code: Some("0".to_string()),
        use_default_attendance_type: false,
        entry_time: Some("09:00".to_string()),
        exit_time: Some("18:00".to_string()),
        comment: None,
        clear_entry: false,
        clear_exit: false,
        clear_comment: false,
    };
    let preview = provider
        .submit_day(&change, WriteMode::DryRun)
        .await
        .expect("submit");
    assert!(!preview.executed);

    let salary = provider.salary_summary(2).await.expect("salary");
    assert_eq!(salary.entries.len(), 2);

    let absences = provider.absence_symbols().await.expect("absences");
    assert_eq!(absences[0].id, "481");

    let payslip = provider
        .download_payslip(month, Some(Path::new("/tmp/payslip.pdf")))
        .await
        .expect("payslip");
    assert_eq!(payslip.path, PathBuf::from("/tmp/payslip.pdf"));

    let report = provider
        .report(ReportSpec::Named("errors".to_string()))
        .await
        .expect("report");
    assert_eq!(report.name, "errors");
}
