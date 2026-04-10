use chrono::NaiveDate;
use shaon::core::{
    AttendanceChange, AttendanceProvider, AttendanceType, FixTarget, MonthCalendar, ProviderError,
    WriteMode, WritePreview,
};
use shaon::use_cases::{
    auto_fill, build_overview, fill_range, fix_day as run_fix_day, AutoFillOptions,
    FillRangeOptions, SuggestedActionPlan,
};
use std::collections::BTreeMap;

struct OverviewProvider;

#[async_trait::async_trait]
impl AttendanceProvider for OverviewProvider {
    async fn identity(&mut self) -> Result<shaon::core::UserIdentity, ProviderError> {
        Ok(shaon::core::UserIdentity {
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
                shaon::core::CalendarDay {
                    date: NaiveDate::from_ymd_opt(2026, 4, 6).unwrap(),
                    day_name: "Sun".to_string(),
                    has_error: true,
                    error_message: Some("missing report".to_string()),
                    entry_time: None,
                    exit_time: None,
                    attendance_type: Some("work day".to_string()),
                    total_hours: None,
                },
                shaon::core::CalendarDay {
                    date: NaiveDate::from_ymd_opt(2026, 4, 7).unwrap(),
                    day_name: "Mon".to_string(),
                    has_error: false,
                    error_message: None,
                    entry_time: None,
                    exit_time: None,
                    attendance_type: None,
                    total_hours: None,
                },
                shaon::core::CalendarDay {
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

struct RecordingProvider {
    calendar: MonthCalendar,
    submissions: Vec<(AttendanceChange, WriteMode)>,
    fixes: Vec<(FixTarget, AttendanceChange, WriteMode)>,
}

impl RecordingProvider {
    fn new(calendar: MonthCalendar) -> Self {
        Self {
            calendar,
            submissions: Vec::new(),
            fixes: Vec::new(),
        }
    }
}

#[async_trait::async_trait]
impl AttendanceProvider for RecordingProvider {
    async fn identity(&mut self) -> Result<shaon::core::UserIdentity, ProviderError> {
        Ok(shaon::core::UserIdentity {
            user_id: "u-2".to_string(),
            employee_id: self.calendar.employee_id.clone(),
            display_name: "Recorder".to_string(),
            is_manager: false,
        })
    }

    async fn month_calendar(&mut self, _month: NaiveDate) -> Result<MonthCalendar, ProviderError> {
        Ok(self.calendar.clone())
    }

    async fn attendance_types(&mut self) -> Result<Vec<AttendanceType>, ProviderError> {
        Ok(Vec::new())
    }

    async fn fix_targets(&mut self, _month: NaiveDate) -> Result<Vec<FixTarget>, ProviderError> {
        Ok(Vec::new())
    }

    async fn submit_day(
        &mut self,
        change: &AttendanceChange,
        mode: WriteMode,
    ) -> Result<WritePreview, ProviderError> {
        self.submissions.push((change.clone(), mode));
        Ok(WritePreview {
            executed: matches!(mode, WriteMode::Execute),
            summary: format!("submit {}", change.date),
            provider_debug: None,
        })
    }

    async fn fix_day(
        &mut self,
        target: &FixTarget,
        change: &AttendanceChange,
        mode: WriteMode,
    ) -> Result<WritePreview, ProviderError> {
        self.fixes.push((target.clone(), change.clone(), mode));
        Ok(WritePreview {
            executed: matches!(mode, WriteMode::Execute),
            summary: format!("fix {}", change.date),
            provider_debug: None,
        })
    }
}

#[tokio::test]
async fn fill_range_submits_each_non_weekend_day_via_provider() {
    let calendar = MonthCalendar {
        month: NaiveDate::from_ymd_opt(2026, 4, 1).unwrap(),
        employee_id: "e-2".to_string(),
        days: Vec::new(),
    };
    let mut provider = RecordingProvider::new(calendar);

    let previews = fill_range(
        &mut provider,
        NaiveDate::from_ymd_opt(2026, 4, 9).unwrap(),
        NaiveDate::from_ymd_opt(2026, 4, 12).unwrap(),
        FillRangeOptions {
            attendance_type_code: None,
            hours: Some(("09:00".to_string(), "18:00".to_string())),
            include_weekends: false,
            mode: WriteMode::DryRun,
        },
    )
    .await
    .expect("fill range");

    assert_eq!(previews.len(), 2);
    assert_eq!(provider.submissions.len(), 2);
    assert_eq!(
        provider.submissions[0].0.date,
        NaiveDate::from_ymd_opt(2026, 4, 9).unwrap()
    );
    assert_eq!(
        provider.submissions[1].0.date,
        NaiveDate::from_ymd_opt(2026, 4, 12).unwrap()
    );
    assert!(provider.submissions[0].0.use_default_attendance_type);
    assert_eq!(
        provider.submissions[0].0.entry_time.as_deref(),
        Some("09:00")
    );
    assert_eq!(
        provider.submissions[0].0.exit_time.as_deref(),
        Some("18:00")
    );
    assert!(provider.submissions[0].0.clear_comment);
}

#[tokio::test]
async fn fix_day_routes_target_and_change_through_provider() {
    let calendar = MonthCalendar {
        month: NaiveDate::from_ymd_opt(2026, 4, 1).unwrap(),
        employee_id: "e-2".to_string(),
        days: Vec::new(),
    };
    let mut provider = RecordingProvider::new(calendar);
    let target = FixTarget {
        date: NaiveDate::from_ymd_opt(2026, 4, 6).unwrap(),
        issue_kind: Some("missing".to_string()),
        provider_ref: "report-1:63".to_string(),
        metadata: BTreeMap::from([
            ("report_id".to_string(), "report-1".to_string()),
            ("error_type".to_string(), "63".to_string()),
        ]),
    };

    let preview = run_fix_day(
        &mut provider,
        &target,
        Some("0".to_string()),
        None,
        WriteMode::DryRun,
    )
    .await
    .expect("fix day");

    assert!(!preview.executed);
    assert_eq!(provider.fixes.len(), 1);
    assert_eq!(provider.fixes[0].0.provider_ref, "report-1:63");
    assert_eq!(
        provider.fixes[0].1.attendance_type_code.as_deref(),
        Some("0")
    );
}

#[tokio::test]
async fn auto_fill_only_targets_missing_days_and_respects_safety_cap() {
    let month = NaiveDate::from_ymd_opt(2026, 4, 1).unwrap();
    let calendar = MonthCalendar {
        month,
        employee_id: "e-2".to_string(),
        days: vec![
            shaon::core::CalendarDay {
                date: NaiveDate::from_ymd_opt(2026, 4, 6).unwrap(),
                day_name: "Sun".to_string(),
                has_error: false,
                error_message: None,
                entry_time: None,
                exit_time: None,
                attendance_type: None,
                total_hours: None,
            },
            shaon::core::CalendarDay {
                date: NaiveDate::from_ymd_opt(2026, 4, 7).unwrap(),
                day_name: "Mon".to_string(),
                has_error: true,
                error_message: Some("error".to_string()),
                entry_time: None,
                exit_time: None,
                attendance_type: None,
                total_hours: None,
            },
            shaon::core::CalendarDay {
                date: NaiveDate::from_ymd_opt(2026, 4, 8).unwrap(),
                day_name: "Tue".to_string(),
                has_error: false,
                error_message: None,
                entry_time: Some("09:00".to_string()),
                exit_time: None,
                attendance_type: None,
                total_hours: None,
            },
            shaon::core::CalendarDay {
                date: NaiveDate::from_ymd_opt(2026, 4, 10).unwrap(),
                day_name: "Fri".to_string(),
                has_error: false,
                error_message: None,
                entry_time: None,
                exit_time: None,
                attendance_type: None,
                total_hours: None,
            },
            shaon::core::CalendarDay {
                date: NaiveDate::from_ymd_opt(2026, 4, 13).unwrap(),
                day_name: "Mon".to_string(),
                has_error: false,
                error_message: None,
                entry_time: None,
                exit_time: None,
                attendance_type: None,
                total_hours: None,
            },
        ],
    };
    let mut provider = RecordingProvider::new(calendar.clone());

    let result = auto_fill(
        &mut provider,
        &calendar,
        AutoFillOptions {
            type_code: Some("0".to_string()),
            type_display: "work day".to_string(),
            hours: None,
            include_weekends: false,
            mode: WriteMode::DryRun,
            max_days: 10,
            today: NaiveDate::from_ymd_opt(2026, 4, 10).unwrap(),
        },
    )
    .await
    .expect("auto fill");

    assert_eq!(result.summary.total_candidates, 1);
    assert_eq!(result.summary.skipped, 4);
    assert_eq!(provider.submissions.len(), 0);

    let err = auto_fill(
        &mut provider,
        &calendar,
        AutoFillOptions {
            type_code: Some("0".to_string()),
            type_display: "work day".to_string(),
            hours: None,
            include_weekends: false,
            mode: WriteMode::Execute,
            max_days: 0,
            today: NaiveDate::from_ymd_opt(2026, 4, 10).unwrap(),
        },
    )
    .await
    .expect_err("safety cap");

    assert_eq!(err.code, "auto_fill_limit_exceeded");
}
