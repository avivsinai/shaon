use chrono::{Datelike, NaiveDate};
use serde::Serialize;
use std::collections::BTreeMap;

use crate::{
    AbsenceProvider, AbsenceSymbol, AttendanceChange, AttendanceProvider, AttendanceType,
    CalendarDay, FixTarget, MonthCalendar, ProviderError, ReportTable, UserIdentity, WriteMode,
    WritePreview,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedAttendanceType {
    pub code: String,
    pub display: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FillRangeOptions {
    pub attendance_type_code: Option<String>,
    pub hours: Option<(String, String)>,
    pub include_weekends: bool,
    pub mode: WriteMode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutoFillOptions {
    pub type_code: Option<String>,
    pub type_display: String,
    pub hours: Option<(String, String)>,
    pub include_weekends: bool,
    pub mode: WriteMode,
    pub max_days: u32,
    pub today: NaiveDate,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct AutoFillDayResult {
    pub date: String,
    pub attendance_type: String,
    pub hours: Option<String>,
    pub status: String,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SkippedDay {
    pub date: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct AutoFillSummary {
    pub total_candidates: u32,
    pub filled: u32,
    pub skipped: u32,
    pub failed: u32,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct AutoFillResult {
    pub month: String,
    pub mode: String,
    pub attendance_type: String,
    pub filled: Vec<AutoFillDayResult>,
    pub skipped: Vec<SkippedDay>,
    pub summary: AutoFillSummary,
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

pub async fn fill_range<P: AttendanceProvider>(
    provider: &mut P,
    from: NaiveDate,
    to: NaiveDate,
    options: FillRangeOptions,
) -> Result<Vec<WritePreview>, ProviderError> {
    if from > to {
        return Err(ProviderError::new(
            "invalid_fill_range",
            "--from must be before or equal to --to",
        ));
    }

    if options.attendance_type_code.is_none() && options.hours.is_none() {
        return Err(ProviderError::new(
            "invalid_fill_request",
            "fill requires at least one of attendance_type_code or hours",
        ));
    }

    let mut previews = Vec::new();
    let mut current = from;
    while current <= to {
        if options.include_weekends || !is_weekend(current) {
            let change = fill_change(
                current,
                options.attendance_type_code.clone(),
                options.hours.clone(),
            );
            previews.push(provider.submit_day(&change, options.mode).await?);
        }
        current = current.succ_opt().expect("valid next date");
    }

    Ok(previews)
}

pub async fn fix_day<P: AttendanceProvider>(
    provider: &mut P,
    target: &FixTarget,
    attendance_type_code: Option<String>,
    hours: Option<(String, String)>,
    mode: WriteMode,
) -> Result<WritePreview, ProviderError> {
    if attendance_type_code.is_none() && hours.is_none() {
        return Err(ProviderError::new(
            "invalid_fix_request",
            "fix requires at least one of attendance_type_code or hours",
        ));
    }

    let (entry_time, exit_time, clear_entry, clear_exit) = match hours {
        Some((entry, exit)) => (Some(entry), Some(exit), false, false),
        None => (None, None, false, false),
    };

    let change = AttendanceChange {
        date: target.date,
        attendance_type_code,
        use_default_attendance_type: false,
        entry_time,
        exit_time,
        comment: None,
        clear_entry,
        clear_exit,
        clear_comment: false,
    };

    provider.fix_day(target, &change, mode).await
}

pub async fn auto_fill<P: AttendanceProvider>(
    provider: &mut P,
    calendar: &MonthCalendar,
    options: AutoFillOptions,
) -> Result<AutoFillResult, ProviderError> {
    let hours_display = options
        .hours
        .as_ref()
        .map(|(entry, exit)| format!("{entry}-{exit}"));
    let mut candidates = Vec::new();
    let mut skipped = Vec::new();

    for day in &calendar.days {
        let date_str = day.date.format("%Y-%m-%d").to_string();
        let day_label = day.date.format("%a").to_string();
        let date_display = format!("{date_str} {day_label}");

        if day.date > options.today {
            skipped.push(SkippedDay {
                date: date_display,
                reason: "future".to_string(),
            });
            continue;
        }
        if day.has_error {
            skipped.push(SkippedDay {
                date: date_display,
                reason: "has_error".to_string(),
            });
            continue;
        }
        if day.is_reported() && !is_day_partial(day) && !is_day_missing(day) {
            skipped.push(SkippedDay {
                date: date_display,
                reason: "already_filled".to_string(),
            });
            continue;
        }
        if is_day_partial(day) {
            skipped.push(SkippedDay {
                date: date_display,
                reason: "partial_data".to_string(),
            });
            continue;
        }
        if !options.include_weekends && is_weekend(day.date) {
            skipped.push(SkippedDay {
                date: date_display,
                reason: "weekend".to_string(),
            });
            continue;
        }

        candidates.push((date_display, auto_fill_change(day.date, &options)));
    }

    let candidate_count = candidates.len() as u32;
    if matches!(options.mode, WriteMode::Execute) && candidate_count > options.max_days {
        return Err(ProviderError::new(
            "auto_fill_limit_exceeded",
            format!(
                "{candidate_count} days exceeds safety limit of {}",
                options.max_days
            ),
        ));
    }

    let mut filled = Vec::new();
    for (date, change) in &candidates {
        let (status, error) = if matches!(options.mode, WriteMode::Execute) {
            match provider.submit_day(change, WriteMode::Execute).await {
                Ok(_) => ("success".to_string(), None),
                Err(err) => ("failed".to_string(), Some(err.to_string())),
            }
        } else {
            ("would_fill".to_string(), None)
        };

        filled.push(AutoFillDayResult {
            date: date.clone(),
            attendance_type: options.type_display.clone(),
            hours: hours_display.clone(),
            status,
            error,
        });
    }

    let failed = filled.iter().filter(|day| day.status == "failed").count() as u32;
    let filled_ok = filled.iter().filter(|day| day.status != "failed").count() as u32;

    Ok(AutoFillResult {
        month: calendar.month.format("%Y-%m").to_string(),
        mode: if matches!(options.mode, WriteMode::Execute) {
            "executed".to_string()
        } else {
            "dry_run".to_string()
        },
        attendance_type: options.type_display,
        filled,
        skipped: skipped.clone(),
        summary: AutoFillSummary {
            total_candidates: candidate_count,
            filled: filled_ok,
            skipped: skipped.len() as u32,
            failed,
        },
    })
}

pub async fn resolve_attendance_type<P: AttendanceProvider>(
    provider: &mut P,
    requested: Option<&str>,
) -> Result<Option<ResolvedAttendanceType>, ProviderError> {
    let Some(requested) = requested else {
        return Ok(None);
    };

    if requested.chars().all(|ch| ch.is_ascii_digit()) {
        return Ok(Some(ResolvedAttendanceType {
            code: requested.to_string(),
            display: requested.to_string(),
        }));
    }

    let requested_lower = requested.to_lowercase();
    let types = provider.attendance_types().await?;
    types
        .into_iter()
        .find(|attendance_type| {
            attendance_type.code.to_lowercase() == requested_lower
                || attendance_type.name_he.to_lowercase() == requested_lower
                || attendance_type
                    .name_en
                    .as_deref()
                    .is_some_and(|name| name.to_lowercase() == requested_lower)
        })
        .map(|attendance_type| ResolvedAttendanceType {
            display: attendance_type
                .name_en
                .unwrap_or(attendance_type.name_he.clone()),
            code: attendance_type.code,
        })
        .ok_or_else(|| {
            ProviderError::new(
                "unknown_attendance_type",
                format!("Unknown attendance type '{requested}'"),
            )
        })
        .map(Some)
}

pub async fn describe_attendance_types<P: AttendanceProvider>(
    provider: &mut P,
) -> Result<String, ProviderError> {
    let types = provider.attendance_types().await?;
    if types.is_empty() {
        return Ok("No attendance types available.".to_string());
    }

    let mut out = String::from("Available attendance types:\n");
    for attendance_type in types {
        let en = attendance_type
            .name_en
            .as_deref()
            .map(|name| format!(" ({name})"))
            .unwrap_or_default();
        out.push_str(&format!(
            "  {} -- {}{}\n",
            attendance_type.code, attendance_type.name_he, en
        ));
    }
    Ok(out)
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

pub fn print_auto_fill(result: &AutoFillResult) {
    let mode_tag = if result.mode == "dry_run" {
        " [DRY RUN]"
    } else {
        ""
    };
    println!("Auto-fill {}{}", result.month, mode_tag);
    println!();

    for day in &result.filled {
        let hours_part = day
            .hours
            .as_deref()
            .map(|hours| format!(" {hours}"))
            .unwrap_or_default();
        let status_part = match day.status.as_str() {
            "would_fill" => " [would fill]".to_string(),
            "success" => " [filled]".to_string(),
            "failed" => format!(
                " [FAILED: {}]",
                day.error.as_deref().unwrap_or("unknown error")
            ),
            other => format!(" [{other}]"),
        };
        println!(
            "  {} \u{2014} {}{}{}",
            day.date, day.attendance_type, hours_part, status_part
        );
    }

    if result
        .skipped
        .iter()
        .any(|day| day.reason == "partial_data")
    {
        println!();
        println!("Skipped (partial data):");
        for day in result
            .skipped
            .iter()
            .filter(|day| day.reason == "partial_data")
        {
            println!("  {} \u{2014} skipped (partial data)", day.date);
        }
    }

    println!();
    if result.mode == "dry_run" {
        println!(
            "Summary: {} days to fill, {} skipped",
            result.summary.total_candidates, result.summary.skipped,
        );
        println!("Run with --execute to submit.");
    } else {
        println!(
            "Summary: {} filled, {} failed, {} skipped",
            result.summary.filled, result.summary.failed, result.summary.skipped,
        );
    }
}

pub fn print_calendar(calendar: &MonthCalendar) {
    println!(
        "Attendance for {} (employee {})",
        calendar.month.format("%Y-%m"),
        calendar.employee_id
    );
    println!();

    let date_w = 10;
    let day_w = 5;
    let entry_w = 5;
    let exit_w = 5;
    let type_w = 15;
    let hours_w = 5;
    let status_w = 6;

    println!(
        "{:<date_w$}  {:<day_w$}  {:<entry_w$}  {:<exit_w$}  {:<type_w$}  {:<hours_w$}  {:<status_w$}",
        "Date", "Day", "Entry", "Exit", "Type", "Hours", "Status",
    );
    println!(
        "{:-<date_w$}  {:-<day_w$}  {:-<entry_w$}  {:-<exit_w$}  {:-<type_w$}  {:-<hours_w$}  {:-<status_w$}",
        "", "", "", "", "", "", "",
    );

    for day in &calendar.days {
        let entry = day.entry_time.as_deref().unwrap_or("-");
        let exit = day.exit_time.as_deref().unwrap_or("-");
        let attendance_type = day.attendance_type.as_deref().unwrap_or("-");
        let hours = day.total_hours.as_deref().unwrap_or("-");
        let status = if day.has_error {
            "x"
        } else if day.is_reported() {
            "ok"
        } else {
            "?"
        };

        println!(
            "{:<date_w$}  {:<day_w$}  {:<entry_w$}  {:<exit_w$}  {:<type_w$}  {:<hours_w$}  {:<status_w$}",
            day.date.format("%Y-%m-%d"),
            day.day_name,
            entry,
            exit,
            attendance_type,
            hours,
            status,
        );
    }

    let total = calendar.days.len();
    let errors = calendar.days.iter().filter(|day| day.has_error).count();
    let reported = calendar.days.iter().filter(|day| day.is_reported()).count();
    let missing = total.saturating_sub(reported).saturating_sub(errors);

    println!();
    println!("{total} days: {reported} reported, {errors} errors, {missing} missing");
}

pub fn print_attendance_types(types: &[AttendanceType]) {
    if types.is_empty() {
        println!("No attendance types found.");
        return;
    }

    let code_w = types
        .iter()
        .map(|item| item.code.len())
        .max()
        .unwrap_or(4)
        .max(4);
    let he_w = types
        .iter()
        .map(|item| item.name_he.len())
        .max()
        .unwrap_or(6)
        .max(6);
    let en_w = types
        .iter()
        .map(|item| item.name_en.as_deref().map_or(0, str::len))
        .max()
        .unwrap_or(7)
        .max(7);

    println!(
        "{:<code_w$}  {:<he_w$}  {:<en_w$}",
        "Code", "Hebrew", "English",
    );
    println!("{:-<code_w$}  {:-<he_w$}  {:-<en_w$}", "", "", "",);
    for item in types {
        println!(
            "{:<code_w$}  {:<he_w$}  {:<en_w$}",
            item.code,
            item.name_he,
            item.name_en.as_deref().unwrap_or(""),
        );
    }
}

pub fn print_absence_symbols(symbols: &[AbsenceSymbol]) {
    if symbols.is_empty() {
        println!("No absence symbols found.");
        return;
    }

    println!("{:<6}  {:<20}  Display", "ID", "Name");
    println!("{:-<6}  {:-<20}  {:-<30}", "", "", "");
    for symbol in symbols {
        println!(
            "{:<6}  {:<20}  {}",
            symbol.id,
            symbol.name,
            symbol.display_name.as_deref().unwrap_or(""),
        );
    }
}

pub fn print_report_table(table: &ReportTable) {
    if table.headers.is_empty() && table.rows.is_empty() {
        println!("(empty report - no data rows found)");
        return;
    }

    let col_count = table
        .headers
        .len()
        .max(table.rows.iter().map(|row| row.len()).max().unwrap_or(0));

    if col_count == 0 {
        println!("(empty report - no columns found)");
        return;
    }

    let mut widths = vec![0usize; col_count];
    for (index, header) in table.headers.iter().enumerate() {
        widths[index] = widths[index].max(header.chars().count());
    }
    for row in &table.rows {
        for (index, cell) in row.iter().enumerate() {
            if index < col_count {
                widths[index] = widths[index].max(cell.chars().count());
            }
        }
    }
    for width in &mut widths {
        *width = (*width).clamp(2, 40);
    }

    if !table.headers.is_empty() {
        let header_line: Vec<String> = (0..col_count)
            .map(|index| {
                let value = table.headers.get(index).map(String::as_str).unwrap_or("");
                pad_or_truncate(value, widths[index])
            })
            .collect();
        println!("{}", header_line.join("  "));

        let separator: Vec<String> = widths.iter().map(|width| "-".repeat(*width)).collect();
        println!("{}", separator.join("  "));
    }

    for row in &table.rows {
        let line: Vec<String> = (0..col_count)
            .map(|index| {
                let value = row.get(index).map(String::as_str).unwrap_or("");
                pad_or_truncate(value, widths[index])
            })
            .collect();
        println!("{}", line.join("  "));
    }

    println!("\n({} rows)", table.rows.len());
}

pub async fn load_absence_symbols<P: AbsenceProvider>(
    provider: &mut P,
) -> Result<Vec<AbsenceSymbol>, ProviderError> {
    provider.absence_symbols().await
}

fn pad_or_truncate(value: &str, width: usize) -> String {
    let truncated: String = value.chars().take(width).collect();
    format!("{truncated:<width$}")
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

fn fill_change(
    date: NaiveDate,
    attendance_type_code: Option<String>,
    hours: Option<(String, String)>,
) -> AttendanceChange {
    let (entry_time, exit_time, clear_entry, clear_exit) = match hours {
        Some((entry, exit)) => (Some(entry), Some(exit), false, false),
        None => (None, None, true, true),
    };

    AttendanceChange {
        date,
        use_default_attendance_type: attendance_type_code.is_none() && entry_time.is_some(),
        attendance_type_code,
        entry_time,
        exit_time,
        comment: None,
        clear_entry,
        clear_exit,
        clear_comment: true,
    }
}

fn auto_fill_change(date: NaiveDate, options: &AutoFillOptions) -> AttendanceChange {
    let (entry_time, exit_time, clear_entry, clear_exit) = match &options.hours {
        Some((entry, exit)) => (Some(entry.clone()), Some(exit.clone()), false, false),
        None => (None, None, true, true),
    };

    AttendanceChange {
        date,
        use_default_attendance_type: options.type_code.is_none() && entry_time.is_some(),
        attendance_type_code: options.type_code.clone(),
        entry_time,
        exit_time,
        comment: None,
        clear_entry,
        clear_exit,
        clear_comment: true,
    }
}

fn is_day_missing(day: &CalendarDay) -> bool {
    day.entry_time.is_none()
        && day.exit_time.is_none()
        && day.attendance_type.is_none()
        && day.total_hours.is_none()
}

fn is_day_partial(day: &CalendarDay) -> bool {
    if is_day_missing(day) {
        return false;
    }
    day.entry_time.is_some() != day.exit_time.is_some()
}

fn is_weekend(date: NaiveDate) -> bool {
    matches!(date.weekday(), chrono::Weekday::Fri | chrono::Weekday::Sat)
}
