use chrono::{Datelike, NaiveDate};
use std::collections::BTreeMap;

use crate::core::{
    AttendanceProvider, AttendanceType, CalendarDay, FixTarget, MonthCalendar, ProviderError,
    UserIdentity,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverviewSummary {
    pub total_work_days: u32,
    pub reported: u32,
    pub missing: u32,
    pub errors: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverviewErrorDay {
    pub day: CalendarDay,
    pub fix_target: Option<FixTarget>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SuggestedActionPlan {
    FixErrors {
        month: NaiveDate,
        count: u32,
        fixable_targets: Vec<FixTarget>,
    },
    FillMissing {
        from: NaiveDate,
        to: NaiveDate,
        count: u32,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverviewData {
    pub identity: UserIdentity,
    pub month: NaiveDate,
    pub calendar: MonthCalendar,
    pub attendance_types: Vec<AttendanceType>,
    pub summary: OverviewSummary,
    pub error_days: Vec<OverviewErrorDay>,
    pub missing_days: Vec<NaiveDate>,
    pub suggested_actions: Vec<SuggestedActionPlan>,
}

pub async fn build_overview<P: AttendanceProvider>(
    provider: &mut P,
    month: NaiveDate,
    today: NaiveDate,
) -> Result<OverviewData, ProviderError> {
    let identity = provider.identity().await?;
    let calendar = provider.month_calendar(month).await?;
    let attendance_types = load_attendance_types(provider).await;
    let fix_targets = load_fix_targets(provider, month).await;
    let fix_targets_by_date: BTreeMap<NaiveDate, FixTarget> = fix_targets
        .iter()
        .cloned()
        .map(|target| (target.date, target))
        .collect();

    let is_current_month = month.year() == today.year() && month.month() == today.month();
    let is_past = |day: &CalendarDay| !(is_current_month && day.date > today);

    let error_days: Vec<OverviewErrorDay> = calendar
        .days
        .iter()
        .filter(|day| day.has_error)
        .cloned()
        .map(|day| OverviewErrorDay {
            fix_target: fix_targets_by_date.get(&day.date).cloned(),
            day,
        })
        .collect();

    let missing_days: Vec<NaiveDate> = calendar
        .days
        .iter()
        .filter(|day| day.is_work_day() && !day.is_reported() && !day.has_error && is_past(day))
        .map(|day| day.date)
        .collect();

    let total_work_days = calendar
        .days
        .iter()
        .filter(|day| day.is_work_day() && is_past(day))
        .count() as u32;

    let summary = OverviewSummary {
        total_work_days,
        reported: calendar.days.iter().filter(|day| day.is_reported()).count() as u32,
        missing: missing_days.len() as u32,
        errors: error_days.len() as u32,
    };

    let mut suggested_actions = Vec::new();
    if !error_days.is_empty() {
        let fixable_targets = error_days
            .iter()
            .filter_map(|day| day.fix_target.clone())
            .collect();
        suggested_actions.push(SuggestedActionPlan::FixErrors {
            month,
            count: error_days.len() as u32,
            fixable_targets,
        });
    }
    if let (Some(from), Some(to)) = (missing_days.first().copied(), missing_days.last().copied()) {
        suggested_actions.push(SuggestedActionPlan::FillMissing {
            from,
            to,
            count: missing_days.len() as u32,
        });
    }

    Ok(OverviewData {
        identity,
        month,
        calendar,
        attendance_types,
        summary,
        error_days,
        missing_days,
        suggested_actions,
    })
}

pub fn error_days(calendar: &MonthCalendar) -> Vec<&CalendarDay> {
    calendar.days.iter().filter(|day| day.has_error).collect()
}

pub fn print_error_days(calendar: &MonthCalendar) {
    let error_days = error_days(calendar);
    if error_days.is_empty() {
        println!("No attendance errors found.");
        return;
    }

    println!(
        "Attendance errors for {} ({}): {} day(s)",
        calendar.month.format("%Y-%m"),
        calendar.employee_id,
        error_days.len()
    );
    println!();

    let date_w = 10;
    let day_w = 5;
    let msg_w = 40;

    println!(
        "{:<date_w$}  {:<day_w$}  {:<msg_w$}",
        "Date", "Day", "Error",
    );
    println!("{:-<date_w$}  {:-<day_w$}  {:-<msg_w$}", "", "", "",);

    for day in error_days {
        let msg = day.error_message.as_deref().unwrap_or("missing report");
        println!(
            "{:<date_w$}  {:<day_w$}  {:<msg_w$}",
            day.date.format("%Y-%m-%d"),
            &day.day_name,
            msg,
        );
    }
}

async fn load_attendance_types<P: AttendanceProvider>(provider: &mut P) -> Vec<AttendanceType> {
    match provider.attendance_types().await {
        Ok(types) => types,
        Err(err) => {
            tracing::debug!("attendance types lookup failed: {err}");
            Vec::new()
        }
    }
}

async fn load_fix_targets<P: AttendanceProvider>(
    provider: &mut P,
    month: NaiveDate,
) -> Vec<FixTarget> {
    match provider.fix_targets(month).await {
        Ok(targets) => targets,
        Err(err) => {
            tracing::debug!("fix target lookup failed: {err}");
            Vec::new()
        }
    }
}
