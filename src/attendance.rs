use anyhow::{anyhow, Context, Result};
use chrono::{Datelike, NaiveDate};
use scraper::{Html, Selector};
use serde::Serialize;

use crate::client::{format_form_fields_for_display, HilanClient};

#[derive(Debug, Serialize)]
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
    /// Whether this day has any attendance report (entry time or attendance type set).
    pub fn is_reported(&self) -> bool {
        self.entry_time.is_some() || self.attendance_type.is_some()
    }

    /// Whether this day falls on a work day (Sun-Thu, Israeli work week).
    pub fn is_work_day(&self) -> bool {
        self.date.weekday().num_days_from_sunday() < 5
    }
}

#[derive(Debug, Serialize)]
pub struct MonthCalendar {
    pub month: NaiveDate,
    pub employee_id: String,
    pub days: Vec<CalendarDay>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AttendanceSubmit {
    pub date: NaiveDate,
    pub attendance_type_code: Option<String>,
    pub entry_time: Option<String>,
    pub exit_time: Option<String>,
    pub comment: Option<String>,
    pub clear_entry: bool,
    pub clear_exit: bool,
    pub clear_comment: bool,
    pub default_work_day: bool,
}

#[derive(Debug, Serialize)]
pub struct SubmitPreview {
    pub url: String,
    pub button_name: String,
    pub button_value: String,
    pub employee_id: String,
    pub payload_display: String,
    pub executed: bool,
}

/// Fetch and parse the attendance calendar for a given month.
pub async fn read_calendar(client: &mut HilanClient, month: NaiveDate) -> Result<MonthCalendar> {
    let url = format!(
        "{}/Hilannetv2/Attendance/calendarpage.aspx?isOnSelf=true",
        client.base_url
    );

    let (mut html, mut fields) = client
        .get_aspx_form(&url)
        .await
        .context("read attendance calendar page")?;

    let requested_month = month_field_value(month);
    let current_month = fields
        .get("ctl00$mp$currentMonth")
        .cloned()
        .unwrap_or_default();

    if current_month != requested_month {
        html = client
            .post_aspx_form(
                &url,
                &fields,
                &[("ctl00$mp$currentMonth", requested_month.as_str())],
                "ctl00$mp$RefreshPeriod",
                "דיווח תקופתי",
                true, // read-only navigation, safe to retry
            )
            .await
            .with_context(|| format!("refresh attendance calendar to {}", month.format("%Y-%m")))?;
        fields = crate::client::parse_aspx_form_fields(&html);
    }

    // Extract employee_id from hidden field
    let employee_id = fields
        .iter()
        .find(|(k, _)| k.contains("hCurrentItemId"))
        .map(|(_, v)| v.clone())
        .unwrap_or_default();

    if employee_id.is_empty() {
        return Err(anyhow!(
            "Could not find employee ID (hCurrentItemId) on calendar page"
        ));
    }

    let days = parse_calendar_html(&html, month)?;

    Ok(MonthCalendar {
        month,
        employee_id,
        days,
    })
}

/// Parse the calendar HTML grid and extract day information.
fn parse_calendar_html(html: &str, month: NaiveDate) -> Result<Vec<CalendarDay>> {
    let document = Html::parse_document(html);
    let mut days = Vec::new();

    // The calendar page renders day cells in a table structure.
    // Each day cell typically contains:
    //   - A date number and day name
    //   - Status indicators (error icons, type text)
    //   - Entry/exit times
    //
    // We look for table cells that contain day information by scanning
    // for patterns in the rendered text and class attributes.

    // Strategy 1: Look for day rows in the attendance grid table.
    // The grid has rows with class patterns like "RSGrid" or "ARSGrid",
    // or the calendar may use a different table structure with day cells.

    // Try to find the calendar grid — look for <td> elements that contain
    // date-like content within the main content area.
    let td_sel = Selector::parse("td").map_err(|e| anyhow!("selector parse error: {e}"))?;

    // Look for elements that indicate day entries — these often have
    // specific class patterns or contain structured attendance data.
    // The calendar page may render days as table rows with columns for
    // date, day name, entry, exit, type, hours, status.

    // Try to find the attendance data grid (not the visual calendar,
    // but the tabular data section if present).
    let row_sel = Selector::parse("tr").map_err(|e| anyhow!("selector parse error: {e}"))?;

    // First, try to parse a tabular layout where each row is a day
    let rows: Vec<_> = document.select(&row_sel).collect();

    for row in &rows {
        let cells: Vec<String> = row
            .select(&td_sel)
            .map(|cell| {
                cell.text()
                    .map(str::trim)
                    .filter(|t| !t.is_empty())
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .collect();

        // Skip rows with too few cells or header rows
        if cells.len() < 3 {
            continue;
        }

        // Try to extract a date from the first cell or two
        if let Some(day) = try_parse_day_row(&cells, month) {
            days.push(day);
        }
    }

    // If we didn't find rows in tabular format, try parsing
    // the visual calendar grid (cells with day numbers).
    if days.is_empty() {
        days = parse_calendar_grid(&document, month);
    }

    // Sort by date
    days.sort_by_key(|d| d.date);

    // Deduplicate by date (keep the one with more information)
    days.dedup_by(|b, a| {
        if a.date == b.date {
            // Keep whichever has more data
            if b.entry_time.is_some() && a.entry_time.is_none() {
                *a = CalendarDay {
                    date: b.date,
                    day_name: std::mem::take(&mut b.day_name),
                    has_error: b.has_error || a.has_error,
                    error_message: b.error_message.take().or(a.error_message.take()),
                    entry_time: b.entry_time.take(),
                    exit_time: b.exit_time.take(),
                    attendance_type: b.attendance_type.take().or(a.attendance_type.take()),
                    total_hours: b.total_hours.take().or(a.total_hours.take()),
                };
            }
            true
        } else {
            false
        }
    });

    Ok(days)
}

/// Try to interpret a table row as a calendar day entry.
fn try_parse_day_row(cells: &[String], month: NaiveDate) -> Option<CalendarDay> {
    // Common patterns:
    // Cell 0: date like "01" or "1" or "01/04" or day name
    // Cell 1: day name or entry time
    // Cell 2+: entry, exit, type, hours, status

    let first = cells.first()?;

    // Try to extract a day number from the first cell
    let day_num = extract_day_number(first)?;

    let date = NaiveDate::from_ymd_opt(month.year(), month.month(), day_num)?;

    // Hebrew day names for matching
    let hebrew_days = ["ראשון", "שני", "שלישי", "רביעי", "חמישי", "שישי", "שבת"];
    let english_days = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

    let mut day_name = String::new();
    let mut entry_time = None;
    let mut exit_time = None;
    let mut attendance_type = None;
    let mut total_hours = None;
    let mut has_error = false;
    let mut error_message = None;

    let row_text = cells.join(" ");

    // Check for error indicators
    if row_text.contains("שגיאה")
        || row_text.contains("חסר")
        || row_text.contains("error")
        || row_text.contains("✗")
        || row_text.contains("×")
    {
        has_error = true;
    }

    // Scan cells for known patterns
    for (i, cell) in cells.iter().enumerate() {
        let trimmed = cell.trim();
        if trimmed.is_empty() {
            continue;
        }

        // Day name detection
        if i <= 2 {
            for &hd in &hebrew_days {
                if trimmed.contains(hd) {
                    day_name = hd.to_string();
                }
            }
            for &ed in &english_days {
                if trimmed.contains(ed) {
                    day_name = ed.to_string();
                }
            }
        }

        // Time pattern: HH:MM
        if is_time_pattern(trimmed) {
            if entry_time.is_none() {
                entry_time = Some(trimmed.to_string());
            } else if exit_time.is_none() {
                exit_time = Some(trimmed.to_string());
            }
        }

        // Attendance type keywords
        let type_keywords = [
            "work day",
            "work from home",
            "vacation",
            "sickness",
            "d off",
            "day off",
            "reserve",
            "mourning",
            "course",
            "work abroad",
            "conference",
            "offsite",
            "parental",
            "עבודה",
            "חופשה",
            "מחלה",
        ];
        for kw in &type_keywords {
            if trimmed.to_lowercase().contains(kw) && attendance_type.is_none() {
                attendance_type = Some(trimmed.to_string());
            }
        }

        // Hours pattern: H:MM or HH:MM (as total hours, usually after times)
        if i > 2 && is_hours_pattern(trimmed) && entry_time.is_some() && total_hours.is_none() {
            total_hours = Some(trimmed.to_string());
        }

        // Error message extraction
        if has_error
            && error_message.is_none()
            && (trimmed.contains("חסר דיווח")
                || trimmed.contains("missing")
                || trimmed.contains("שגיאה"))
        {
            error_message = Some(trimmed.to_string());
        }
    }

    // If we have no day name, derive from the date
    if day_name.is_empty() {
        day_name = date.format("%a").to_string();
    }

    Some(CalendarDay {
        date,
        day_name,
        has_error,
        error_message,
        entry_time,
        exit_time,
        attendance_type,
        total_hours,
    })
}

/// Parse the visual calendar grid (month view with clickable day cells).
fn parse_calendar_grid(document: &Html, month: NaiveDate) -> Vec<CalendarDay> {
    let mut days = Vec::new();

    // The calendar page often uses a table-based month grid.
    // Day cells may be <td> elements with class attributes indicating state.
    // We look for any element that contains a day number in the context of
    // attendance-related content.

    // Try to find elements with class containing "calendar" or "day"
    let all_td = Selector::parse("td").unwrap();
    let span_sel = Selector::parse("span").unwrap();
    let img_sel = Selector::parse("img").unwrap();

    for td in document.select(&all_td) {
        let td_text: String = td.text().map(str::trim).collect::<Vec<_>>().join(" ");
        let td_text = td_text.trim();

        if td_text.is_empty() {
            continue;
        }

        // Check if this cell looks like a day cell (contains a small number 1-31)
        let day_num = match extract_day_number_strict(td_text) {
            Some(n) => n,
            None => continue,
        };

        let date = match NaiveDate::from_ymd_opt(month.year(), month.month(), day_num) {
            Some(d) => d,
            None => continue,
        };

        // Check for error indicators via child elements
        let mut has_error = false;
        let mut error_message = None;

        // Check for error-indicating images (red X, error icons)
        for img in td.select(&img_sel) {
            let alt = img.value().attr("alt").unwrap_or("");
            let src = img.value().attr("src").unwrap_or("");
            if alt.contains("error")
                || alt.contains("שגיאה")
                || src.contains("error")
                || src.contains("Error")
                || src.contains("red")
            {
                has_error = true;
            }
        }

        // Check for error-indicating CSS classes
        let class = td.value().attr("class").unwrap_or("");
        if class.contains("error") || class.contains("Error") || class.contains("missing") {
            has_error = true;
        }

        // Check for error text in spans
        for span in td.select(&span_sel) {
            let span_class = span.value().attr("class").unwrap_or("");
            let span_text: String = span.text().collect();
            if span_class.contains("error") || span_class.contains("Error") {
                has_error = true;
                if !span_text.trim().is_empty() {
                    error_message = Some(span_text.trim().to_string());
                }
            }
        }

        // Check for title/tooltip with error info
        if let Some(title) = td.value().attr("title") {
            if title.contains("שגיאה") || title.contains("חסר") || title.contains("error") {
                has_error = true;
                error_message = Some(title.to_string());
            }
        }

        // Extract any time patterns from the cell text
        let mut entry_time = None;
        let mut exit_time = None;
        let mut attendance_type = None;

        let parts: Vec<&str> = td_text.split_whitespace().collect();
        for part in &parts {
            if is_time_pattern(part) {
                if entry_time.is_none() {
                    entry_time = Some(part.to_string());
                } else if exit_time.is_none() {
                    exit_time = Some(part.to_string());
                }
            }
        }

        // Check for attendance type text
        let type_keywords = [
            "work day",
            "work from home",
            "vacation",
            "sickness",
            "d off",
            "day off",
            "reserve",
            "mourning",
            "course",
            "work abroad",
            "conference",
            "offsite",
            "parental",
            "יום עבודה",
            "עבודה מהבית",
            "חופשה",
            "מחלה",
        ];
        let lower = td_text.to_lowercase();
        for kw in &type_keywords {
            if lower.contains(kw) {
                attendance_type = Some(kw.to_string());
                break;
            }
        }

        days.push(CalendarDay {
            date,
            day_name: date.format("%a").to_string(),
            has_error,
            error_message,
            entry_time,
            exit_time,
            attendance_type,
            total_hours: None,
        });
    }

    days
}

/// Extract a day number (1-31) from a string that starts with digits.
fn extract_day_number(s: &str) -> Option<u32> {
    let digits: String = s.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        return None;
    }
    let num: u32 = digits.parse().ok()?;
    if (1..=31).contains(&num) {
        Some(num)
    } else {
        None
    }
}

/// Stricter day number extraction: the cell text should be short
/// or the number should be prominent (not embedded in a long string).
fn extract_day_number_strict(s: &str) -> Option<u32> {
    let trimmed = s.trim();
    // If the entire content is just a number, or starts with a number
    // followed by whitespace or non-digit content
    let digits: String = trimmed.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        return None;
    }
    // Reject time strings like "09:00" or "12:30-18:00" where digits are followed by ':'
    let remaining = trimmed[digits.len()..].chars().next();
    if remaining == Some(':') {
        return None;
    }
    let num: u32 = digits.parse().ok()?;
    if (1..=31).contains(&num) {
        Some(num)
    } else {
        None
    }
}

/// Check if a string looks like a time (HH:MM).
fn is_time_pattern(s: &str) -> bool {
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() != 2 {
        return false;
    }
    let hour: u32 = match parts[0].parse() {
        Ok(h) => h,
        Err(_) => return false,
    };
    let minute: u32 = match parts[1].parse() {
        Ok(m) => m,
        Err(_) => return false,
    };
    hour < 24 && minute < 60
}

/// Check if a string looks like an hours duration (H:MM or HH:MM).
fn is_hours_pattern(s: &str) -> bool {
    // Same format as time but could represent duration
    is_time_pattern(s)
}

/// Print a formatted attendance calendar table.
pub fn print_calendar(cal: &MonthCalendar) {
    println!(
        "Attendance for {} (employee {})",
        cal.month.format("%Y-%m"),
        cal.employee_id
    );
    println!();

    // Column widths
    let date_w = 10;
    let day_w = 5;
    let entry_w = 5;
    let exit_w = 5;
    let type_w = 16;
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

    for day in &cal.days {
        let entry = day.entry_time.as_deref().unwrap_or("-");
        let exit = day.exit_time.as_deref().unwrap_or("-");
        let att_type = day.attendance_type.as_deref().unwrap_or("-");
        let hours = day.total_hours.as_deref().unwrap_or("-");

        let status = if day.has_error {
            "\u{2717}" // ✗
        } else if day.is_reported() {
            "\u{2713}" // ✓
        } else {
            "?"
        };

        println!(
            "{:<date_w$}  {:<day_w$}  {:<entry_w$}  {:<exit_w$}  {:<type_w$}  {:<hours_w$}  {:<status_w$}",
            day.date.format("%Y-%m-%d"),
            &day.day_name,
            entry,
            exit,
            att_type,
            hours,
            status,
        );
    }

    // Summary
    let total = cal.days.len();
    let errors = cal.days.iter().filter(|d| d.has_error).count();
    let reported = cal.days.iter().filter(|d| d.is_reported()).count();
    let missing = total.saturating_sub(reported).saturating_sub(errors);

    println!();
    println!("{total} days: {reported} reported, {errors} errors, {missing} missing");
}

/// Print only the error days from a calendar.
pub fn print_errors(cal: &MonthCalendar) {
    let error_days: Vec<&CalendarDay> = cal.days.iter().filter(|d| d.has_error).collect();

    if error_days.is_empty() {
        println!(
            "No errors found for {} (employee {})",
            cal.month.format("%Y-%m"),
            cal.employee_id
        );
        return;
    }

    println!(
        "Errors for {} (employee {}): {} day(s)",
        cal.month.format("%Y-%m"),
        cal.employee_id,
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

    for day in &error_days {
        let msg = day.error_message.as_deref().unwrap_or("missing report");

        println!(
            "{:<date_w$}  {:<day_w$}  {:<msg_w$}",
            day.date.format("%Y-%m-%d"),
            &day.day_name,
            msg,
        );
    }
}

pub async fn submit_day(
    client: &mut HilanClient,
    submit: &AttendanceSubmit,
    execute: bool,
) -> Result<SubmitPreview> {
    let url = format!(
        "{}/Hilannetv2/Attendance/calendarpage.aspx?isOnSelf=true",
        client.base_url
    );

    replay_submit(client, &url, submit, "שמירה", execute).await
}

pub async fn fix_error_day(
    client: &mut HilanClient,
    submit: &AttendanceSubmit,
    report_id: &str,
    error_type: &str,
    execute: bool,
) -> Result<SubmitPreview> {
    let url = format!(
        "{}/Hilannetv2/EmployeeErrorHandling.aspx?date={}&reportId={}&errorType={}&HideStrip=1&HideEmployeeStrip=1",
        client.base_url,
        submit.date.format("%d/%m/%Y"),
        report_id,
        error_type
    );

    replay_submit(client, &url, submit, "שמור וסגור", execute).await
}

async fn replay_submit(
    client: &mut HilanClient,
    url: &str,
    submit: &AttendanceSubmit,
    button_value: &str,
    execute: bool,
) -> Result<SubmitPreview> {
    client.login().await?;

    let (_html, base_fields) = client
        .get_aspx_form(url)
        .await
        .with_context(|| format!("load form for {}", submit.date.format("%Y-%m-%d")))?;

    let employee_id = match base_fields.get("ctl00$mp$Strip$hCurrentItemId") {
        Some(id) if !id.is_empty() => id.clone(),
        _ => {
            crate::api::bootstrap(client)
                .await
                .context("bootstrap employee info for form replay")?
                .user_id
        }
    };

    let prefix = day_field_prefix(&employee_id, submit.date);
    let entry_key = format!(
        "{prefix}$cellOf_ManualEntry_EmployeeReports_row_0_0$ManualEntry_EmployeeReports_row_0_0"
    );
    let exit_key = format!(
        "{prefix}$cellOf_ManualExit_EmployeeReports_row_0_0$ManualExit_EmployeeReports_row_0_0"
    );
    let comment_key =
        format!("{prefix}$cellOf_Comment_EmployeeReports_row_0_0$Comment_EmployeeReports_row_0_0");
    let type_key = format!(
        "{prefix}$cellOf_Symbol.SymbolId_EmployeeReports_row_0_0$Symbol.SymbolId_EmployeeReports_row_0_0"
    );
    let completion_key = format!(
        "{prefix}$cellOf_CompletionToStandard_EmployeeReports_row_0_0$CompletionToStandard_EmployeeReports_row_0_0"
    );
    let button_name = format!("{prefix}$btnSave");

    let mut replay_fields = base_fields;
    replay_fields.remove(&completion_key);

    let mut overrides: Vec<(String, String)> = vec![
        (
            "__calendarSelectedDays".to_string(),
            calendar_selected_day_value(submit.date),
        ),
        (
            "ctl00$mp$currentMonth".to_string(),
            month_field_value(submit.date),
        ),
        (
            "ctl00$mp$Strip$hCurrentItemId".to_string(),
            employee_id.clone(),
        ),
    ];

    if let Some(entry_time) = &submit.entry_time {
        overrides.push((entry_key.clone(), entry_time.clone()));
    } else if submit.clear_entry {
        overrides.push((entry_key.clone(), String::new()));
    }

    if let Some(exit_time) = &submit.exit_time {
        overrides.push((exit_key.clone(), exit_time.clone()));
    } else if submit.clear_exit {
        overrides.push((exit_key.clone(), String::new()));
    }

    if let Some(type_code) = &submit.attendance_type_code {
        overrides.push((type_key.clone(), type_code.clone()));
    } else if submit.default_work_day {
        overrides.push((type_key.clone(), "0".to_string()));
    }

    if let Some(comment) = &submit.comment {
        overrides.push((comment_key.clone(), comment.clone()));
    } else if submit.clear_comment {
        overrides.push((comment_key.clone(), String::new()));
    }

    let override_refs: Vec<(&str, &str)> = overrides
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();

    let payload_display = format_form_fields_for_display(&replay_fields, &override_refs);

    if execute {
        client
            .post_aspx_form(
                url,
                &replay_fields,
                &override_refs,
                &button_name,
                button_value,
                false, // state-changing write — must NOT be retried
            )
            .await
            .with_context(|| format!("submit attendance form for {}", submit.date))?;
    }

    Ok(SubmitPreview {
        url: url.to_string(),
        button_name,
        button_value: button_value.to_string(),
        employee_id,
        payload_display,
        executed: execute,
    })
}

fn day_field_prefix(employee_id: &str, date: NaiveDate) -> String {
    format!(
        "ctl00$mp$RG_Days_{}_{:04}_{:02}",
        employee_id,
        date.year(),
        date.month()
    )
}

fn month_field_value(date: NaiveDate) -> String {
    format!("01/{:02}/{:04}", date.month(), date.year())
}

fn calendar_selected_day_value(date: NaiveDate) -> String {
    // Inferred from the captured Hilanet payload where 2026-04-10 maps to 9596.
    let epoch = NaiveDate::from_ymd_opt(2000, 1, 1).unwrap();
    date.signed_duration_since(epoch).num_days().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_day_number_strict_time_strings() {
        assert_eq!(extract_day_number_strict("09:00"), None);
        assert_eq!(extract_day_number_strict("12:30-18:00"), None);
        assert_eq!(extract_day_number_strict("9:00"), None);
        assert_eq!(extract_day_number_strict("23:59"), None);
        assert_eq!(extract_day_number_strict("0:00"), None);
    }

    #[test]
    fn test_extract_day_number_strict_valid_days() {
        assert_eq!(extract_day_number_strict("9"), Some(9));
        assert_eq!(extract_day_number_strict("9 Sunday"), Some(9));
        assert_eq!(extract_day_number_strict("31"), Some(31));
        assert_eq!(extract_day_number_strict("1"), Some(1));
        assert_eq!(extract_day_number_strict(" 15 "), Some(15));
        assert_eq!(extract_day_number_strict("28"), Some(28));
    }

    #[test]
    fn test_extract_day_number_strict_invalid() {
        assert_eq!(extract_day_number_strict("0"), None);
        assert_eq!(extract_day_number_strict("32"), None);
        assert_eq!(extract_day_number_strict("abc"), None);
        assert_eq!(extract_day_number_strict(""), None);
        assert_eq!(extract_day_number_strict("  "), None);
        assert_eq!(extract_day_number_strict("100"), None);
    }
}
