use chrono::NaiveDate;
use hilan::core::{
    AttendanceChange, AttendanceProvider, AttendanceType, FixTarget, MonthCalendar, ProviderError,
    WriteMode, WritePreview,
};
use hilan::use_cases::{build_overview, SuggestedActionPlan};
use std::collections::BTreeMap;

struct OverviewProvider;

#[async_trait::async_trait]
impl AttendanceProvider for OverviewProvider {
    async fn identity(&mut self) -> Result<hilan::core::UserIdentity, ProviderError> {
        Ok(hilan::core::UserIdentity {
            user_id: "u-1".to_string(),
            employee_id: "e-1".to_string(),
            display_name: "Overview User".to_string(),
            is_manager: false,
        })
    }

    async fn month_calendar(&mut self, month: NaiveDate) -> Result<MonthCalendar, ProviderError> {
        Ok(MonthCalendar {
            month,
            employee_id: "e-1".to_string(),
            days: vec![
                hilan::core::CalendarDay {
                    date: NaiveDate::from_ymd_opt(2026, 4, 6).unwrap(),
                    day_name: "Sun".to_string(),
                    has_error: true,
                    error_message: Some("missing report".to_string()),
                    entry_time: None,
                    exit_time: None,
                    attendance_type: Some("work day".to_string()),
                    total_hours: None,
                },
                hilan::core::CalendarDay {
                    date: NaiveDate::from_ymd_opt(2026, 4, 7).unwrap(),
                    day_name: "Mon".to_string(),
                    has_error: false,
                    error_message: None,
                    entry_time: None,
                    exit_time: None,
                    attendance_type: None,
                    total_hours: None,
                },
                hilan::core::CalendarDay {
                    date: NaiveDate::from_ymd_opt(2026, 4, 8).unwrap(),
                    day_name: "Tue".to_string(),
                    has_error: false,
                    error_message: None,
                    entry_time: Some("09:00".to_string()),
                    exit_time: Some("18:00".to_string()),
                    attendance_type: Some("work day".to_string()),
                    total_hours: Some("09:00".to_string()),
                },
            ],
        })
    }

    async fn attendance_types(&mut self) -> Result<Vec<AttendanceType>, ProviderError> {
        Ok(vec![AttendanceType {
            code: "0".to_string(),
            name_he: "יום עבודה".to_string(),
            name_en: Some("work day".to_string()),
        }])
    }

    async fn fix_targets(&mut self, _month: NaiveDate) -> Result<Vec<FixTarget>, ProviderError> {
        Ok(vec![FixTarget {
            date: NaiveDate::from_ymd_opt(2026, 4, 6).unwrap(),
            issue_kind: Some("missing".to_string()),
            provider_ref: "report-1:63".to_string(),
            metadata: BTreeMap::from([
                ("report_id".to_string(), "report-1".to_string()),
                ("error_type".to_string(), "63".to_string()),
            ]),
        }])
    }

    async fn submit_day(
        &mut self,
        _change: &AttendanceChange,
        _mode: WriteMode,
    ) -> Result<WritePreview, ProviderError> {
        unreachable!("submit_day is not used by overview")
    }

    async fn fix_day(
        &mut self,
        _target: &FixTarget,
        _change: &AttendanceChange,
        _mode: WriteMode,
    ) -> Result<WritePreview, ProviderError> {
        unreachable!("fix_day is not used by overview")
    }
}

#[tokio::test]
async fn overview_is_planned_from_provider_traits() {
    let mut provider = OverviewProvider;
    let month = NaiveDate::from_ymd_opt(2026, 4, 1).unwrap();
    let today = NaiveDate::from_ymd_opt(2026, 4, 10).unwrap();

    let overview = build_overview(&mut provider, month, today)
        .await
        .expect("overview");

    assert_eq!(overview.identity.display_name, "Overview User");
    assert_eq!(overview.summary.total_work_days, 3);
    assert_eq!(overview.summary.reported, 2);
    assert_eq!(overview.summary.errors, 1);
    assert_eq!(overview.summary.missing, 1);
    assert_eq!(overview.attendance_types.len(), 1);
    assert_eq!(overview.error_days.len(), 1);
    assert_eq!(
        overview.error_days[0]
            .fix_target
            .as_ref()
            .and_then(|target| target.metadata.get("report_id"))
            .map(String::as_str),
        Some("report-1")
    );
    assert_eq!(
        overview.missing_days,
        vec![NaiveDate::from_ymd_opt(2026, 4, 7).unwrap()]
    );
    assert!(matches!(
        overview.suggested_actions[0],
        SuggestedActionPlan::FixErrors { count: 1, .. }
    ));
    assert!(matches!(
        overview.suggested_actions[1],
        SuggestedActionPlan::FillMissing { count: 1, .. }
    ));
}
