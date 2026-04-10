use anyhow::{anyhow, Context, Result};
use chrono::{Datelike, NaiveDate};
use scraper::{ElementRef, Html, Selector};
use serde::Serialize;

use crate::client::{format_form_fields_for_display, HilanClient};

/// Keywords that indicate an attendance type in calendar cells.
///
/// Shared between `try_parse_day_row` and `parse_calendar_grid` to
/// avoid drift between the two code paths.
pub(crate) const TYPE_KEYWORDS: &[&str] = &[
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
    "עבודה",
    "חופשה",
    "מחלה",
];

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
    tracing::debug!(
        "Parsed {} calendar days for {} (employee {})",
        days.len(),
        month.format("%Y-%m"),
        employee_id
    );

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
            .children()
            .filter_map(ElementRef::wrap)
            .filter(|cell| cell.value().name() == "td")
            .map(|cell| cell_visible_text(&cell))
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
        for kw in TYPE_KEYWORDS {
            if trimmed.to_lowercase().contains(kw) && attendance_type.is_none() {
                attendance_type = Some(trimmed.to_string());
            }
        }

        // Hours pattern: H:MM or HH:MM (as total hours, usually after times)
        if i > 2 && is_time_pattern(trimmed) && entry_time.is_some() && total_hours.is_none() {
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
        let td_text = cell_visible_text(&td);
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
        let lower = td_text.to_lowercase();
        for kw in TYPE_KEYWORDS {
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

/// Extract visible text from a table cell, respecting `<select>` elements.
///
/// When a `<select>` is present, only the text of the selected `<option>`
/// (or the first `<option>` if none is marked selected) is included —
/// not the text of every option in the dropdown.
fn cell_visible_text(cell: &ElementRef<'_>) -> String {
    let select_sel = Selector::parse("select").unwrap();
    let option_sel = Selector::parse("option[selected]").unwrap();
    let option_any_sel = Selector::parse("option").unwrap();
    let option_value_sel = Selector::parse("option").unwrap();

    // If the cell contains no <select>, use the normal text extraction.
    let has_select = cell.select(&select_sel).next().is_some();
    if !has_select {
        return cell
            .text()
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
    }

    fn normalized_ov(value: Option<&str>) -> Option<&str> {
        value.filter(|ov| !ov.chars().all(char::is_whitespace))
    }

    fn collect_visible_parts(
        element: &ElementRef<'_>,
        inherited_ov: Option<&str>,
        option_sel: &Selector,
        option_any_sel: &Selector,
        option_value_sel: &Selector,
        parts: &mut Vec<String>,
    ) {
        let current_ov = normalized_ov(element.value().attr("ov")).or(inherited_ov);

        if element.value().name() == "select" {
            let selected_text = element
                .select(option_sel)
                .next()
                .or_else(|| {
                    current_ov.and_then(|ov| {
                        element
                            .select(option_value_sel)
                            .find(|opt| opt.value().attr("value") == Some(ov))
                    })
                })
                .or_else(|| element.select(option_any_sel).next())
                .map(|opt| opt.text().collect::<String>());

            if let Some(text) = selected_text {
                let text = text.trim();
                if !text.is_empty() {
                    parts.push(text.to_string());
                }
            }
            return;
        }

        for child in element.children() {
            if let Some(child_el) = ElementRef::wrap(child) {
                collect_visible_parts(
                    &child_el,
                    current_ov,
                    option_sel,
                    option_any_sel,
                    option_value_sel,
                    parts,
                );
            } else if let Some(text_node) = child.value().as_text() {
                let text = text_node.trim();
                if !text.is_empty() {
                    parts.push(text.to_string());
                }
            }
        }
    }

    let mut parts = Vec::new();
    collect_visible_parts(
        cell,
        None,
        &option_sel,
        &option_any_sel,
        &option_value_sel,
        &mut parts,
    );
    parts.join(" ")
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
pub fn is_time_pattern(s: &str) -> bool {
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
    client.ensure_authenticated().await?;

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

// ---------------------------------------------------------------------------
// Auto-fill types and logic
// ---------------------------------------------------------------------------

#[derive(Serialize)]
pub struct AutoFillResult {
    pub month: String,
    pub mode: String,
    pub attendance_type: String,
    pub filled: Vec<DayResult>,
    pub skipped: Vec<SkippedDay>,
    pub summary: AutoFillSummary,
}

#[derive(Serialize)]
pub struct DayResult {
    pub date: String,
    pub attendance_type: String,
    pub hours: Option<String>,
    pub status: String,
    pub error: Option<String>,
}

#[derive(Serialize)]
pub struct SkippedDay {
    pub date: String,
    pub reason: String,
}

#[derive(Serialize)]
pub struct AutoFillSummary {
    pub total_candidates: u32,
    pub filled: u32,
    pub skipped: u32,
    pub failed: u32,
}

/// Returns true if a calendar day is completely missing attendance data.
fn is_day_missing(day: &CalendarDay) -> bool {
    day.entry_time.is_none()
        && day.exit_time.is_none()
        && day.attendance_type.is_none()
        && day.total_hours.is_none()
}

/// Returns true if a day has partial data (e.g. entry but no exit, or type set but no times).
fn is_day_partial(day: &CalendarDay) -> bool {
    // Has some data but not fully missing
    if is_day_missing(day) {
        return false;
    }
    // Has entry but no exit (or vice versa) — partial
    let has_entry = day.entry_time.is_some();
    let has_exit = day.exit_time.is_some();
    if has_entry != has_exit {
        return true;
    }
    false
}

/// Returns true if the date falls on a weekend (Friday or Saturday in IL locale).
fn is_weekend(date: NaiveDate) -> bool {
    matches!(date.weekday(), chrono::Weekday::Fri | chrono::Weekday::Sat)
}

/// Configuration for an auto-fill run, bundled to avoid too-many-arguments.
pub struct AutoFillOpts<'a> {
    pub type_code: Option<String>,
    pub type_display: &'a str,
    pub hours: Option<&'a (String, String)>,
    pub include_weekends: bool,
    pub execute: bool,
    pub max_days: u32,
}

/// Auto-fill all missing days in a month.
///
/// Design constraints (per Codex review):
/// - Only fills days with NO data at all. Error days and partial days are skipped.
/// - Skips weekends (Fri/Sat) unless `include_weekends` is true.
/// - Safety cap: refuses to execute if candidate count > `max_days`.
pub async fn auto_fill(
    client: &mut HilanClient,
    cal: &MonthCalendar,
    opts: AutoFillOpts<'_>,
) -> anyhow::Result<AutoFillResult> {
    let AutoFillOpts {
        type_code,
        type_display,
        hours,
        include_weekends,
        execute,
        max_days,
    } = opts;
    let hours_display = hours.as_ref().map(|(e, x)| format!("{e}-{x}"));
    let mut filled = Vec::new();
    let mut skipped = Vec::new();
    let today = chrono::Local::now().date_naive();

    for day in &cal.days {
        let date_str = day.date.format("%Y-%m-%d").to_string();
        let day_label = day.date.format("%a").to_string();
        let date_display = format!("{date_str} {day_label}");

        // Skip future days
        if day.date > today {
            skipped.push(SkippedDay {
                date: date_display,
                reason: "future".to_string(),
            });
            continue;
        }

        // Skip error days — those need explicit fix, not auto-fill
        if day.has_error {
            skipped.push(SkippedDay {
                date: date_display,
                reason: "has_error".to_string(),
            });
            continue;
        }

        // Skip days that already have full data
        if day.is_reported() && !is_day_partial(day) && !is_day_missing(day) {
            skipped.push(SkippedDay {
                date: date_display,
                reason: "already_filled".to_string(),
            });
            continue;
        }

        // Skip partial days — entry but no exit, or type set with missing times
        if is_day_partial(day) {
            skipped.push(SkippedDay {
                date: date_display,
                reason: "partial_data".to_string(),
            });
            continue;
        }

        // At this point, the day is truly missing — it's a candidate for fill
        if !include_weekends && is_weekend(day.date) {
            skipped.push(SkippedDay {
                date: date_display,
                reason: "weekend".to_string(),
            });
            continue;
        }

        let (entry_time, exit_time, clear_entry, clear_exit) = match hours {
            Some((entry, exit)) => (Some(entry.clone()), Some(exit.clone()), false, false),
            None => (None, None, true, true),
        };

        let submit = AttendanceSubmit {
            date: day.date,
            attendance_type_code: type_code.clone(),
            entry_time,
            exit_time,
            comment: None,
            clear_entry,
            clear_exit,
            clear_comment: true,
            default_work_day: type_code.is_none(),
        };

        filled.push((date_display, submit));
    }

    // Safety cap check — BEFORE any execution
    let candidate_count = filled.len() as u32;
    if execute && candidate_count > max_days {
        anyhow::bail!(
            "{candidate_count} days exceeds safety limit of {max_days}. \
             Use --max-days {candidate_count} to override."
        );
    }

    // Now execute or preview each candidate
    let mut results = Vec::new();
    for (date_display, submit) in &filled {
        let (status, error) = if execute {
            match submit_day(client, submit, true).await {
                Ok(_) => ("success".to_string(), None),
                Err(e) => ("failed".to_string(), Some(e.to_string())),
            }
        } else {
            ("would_fill".to_string(), None)
        };

        results.push(DayResult {
            date: date_display.clone(),
            attendance_type: type_display.to_string(),
            hours: hours_display.clone(),
            status,
            error,
        });
    }

    let failed = results.iter().filter(|d| d.status == "failed").count() as u32;
    let filled_ok = results.iter().filter(|d| d.status != "failed").count() as u32;
    let skipped_count = skipped.len() as u32;

    let mode = if execute {
        "executed".to_string()
    } else {
        "dry_run".to_string()
    };

    Ok(AutoFillResult {
        month: cal.month.format("%Y-%m").to_string(),
        mode,
        attendance_type: type_display.to_string(),
        filled: results,
        skipped,
        summary: AutoFillSummary {
            total_candidates: candidate_count,
            filled: filled_ok,
            skipped: skipped_count,
            failed,
        },
    })
}

/// Resolve the attendance type code for auto-fill.
///
/// Per Codex review: does NOT infer from most-common type.
/// If `requested` is None, returns Ok(None) — caller must validate.
pub fn resolve_auto_fill_type(
    subdomain: &str,
    requested: Option<&str>,
) -> anyhow::Result<(Option<String>, String)> {
    let Some(name) = requested else {
        return Ok((None, String::new()));
    };

    let ont_path = crate::ontology::ontology_path(subdomain);

    if ont_path.exists() {
        let ontology = crate::ontology::OrgOntology::load(&ont_path)?;
        let at = ontology.validate_type(name)?;
        let display = at.name_en.as_deref().unwrap_or(&at.name_he).to_string();
        return Ok((Some(at.code.clone()), display));
    }

    if name.chars().all(|ch| ch.is_ascii_digit()) {
        return Ok((Some(name.to_string()), name.to_string()));
    }

    anyhow::bail!(
        "Attendance type '{name}' needs cached ontology. Run `hilan sync-types` first or pass a numeric code."
    );
}

/// Print human-readable auto-fill output.
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
            .map(|h| format!(" {h}"))
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

    if result.skipped.iter().any(|s| s.reason == "partial_data") {
        println!();
        println!("Skipped (partial data):");
        for day in result.skipped.iter().filter(|s| s.reason == "partial_data") {
            println!("  {} \u{2014} skipped (partial data)", day.date);
        }
    }

    println!();

    let s = &result.summary;
    if result.mode == "dry_run" {
        println!(
            "Summary: {} days to fill, {} skipped",
            s.total_candidates, s.skipped,
        );
        println!("Run with --execute to submit.");
    } else {
        println!(
            "Summary: {} filled, {} failed, {} skipped",
            s.filled, s.failed, s.skipped,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use scraper::{Html, Selector};

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

    #[test]
    fn live_employee_wrapper_cell_does_not_expand_all_dropdown_options() {
        let row = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/calendar/calendar-row0.html"
        ));
        let html = format!("<table><tbody>{row}</tbody></table>");
        let document = Html::parse_document(&html);
        let top_level_cell_sel = Selector::parse(r#"tr[id$="_row_0"] > td"#).unwrap();
        let employee_wrapper_cell = document
            .select(&top_level_cell_sel)
            .nth(2)
            .expect("employee wrapper cell");

        let text = cell_visible_text(&employee_wrapper_cell);

        assert_eq!(text, "work day");
    }

    #[test]
    fn live_calendar_row_parses_default_work_day_type() {
        let row = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/calendar/calendar-row0.html"
        ));
        let html = format!("<html><body><table><tbody>{row}</tbody></table></body></html>");
        let month = NaiveDate::from_ymd_opt(2026, 4, 1).unwrap();

        let days = parse_calendar_html(&html, month).expect("parse live calendar row fixture");
        let expected_date = NaiveDate::from_ymd_opt(2026, 4, 10).unwrap();
        let day = days
            .iter()
            .find(|day| day.date == expected_date)
            .expect("day 2026-04-10");

        assert_eq!(day.attendance_type.as_deref(), Some("work day"));
    }
}
