use anyhow::{anyhow, Context, Result};
use chrono::{Datelike, NaiveDate};
use scraper::{ElementRef, Html, Selector};
use serde::Serialize;

use crate::client::{format_form_fields_for_display, HilanClient};

/// Default attendance type labels synthesized by the parser.
const LABEL_WORK_DAY: &str = "work day";
const LABEL_VACATION: &str = "vacation";

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
    pub source: hr_core::AttendanceSource,
}

impl CalendarDay {
    /// Whether this day counts as reported by the user or organization.
    pub fn is_reported(&self) -> bool {
        self.source == hr_core::AttendanceSource::UserReported
            || self.source == hr_core::AttendanceSource::Holiday
    }

    pub fn is_auto_filled(&self) -> bool {
        self.source == hr_core::AttendanceSource::SystemAutoFill
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

    let (html, fields) = client
        .get_aspx_form(&url)
        .await
        .context("read attendance calendar page")?;
    let (html, fields) = load_month_page(client, &url, html, fields, month).await?;

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

    let mut days = parse_calendar_html(&html, month)?;
    // Check if the parsed days have actual attendance data (not just day numbers
    // from the mini calendar). The mini calendar returns 30 days with no types,
    // times, or errors — just blank CalendarDay entries.
    let has_real_data = days.iter().any(|d| {
        d.entry_time.is_some()
            || d.exit_time.is_some()
            || d.attendance_type.is_some()
            || d.has_error
    });
    if !has_real_data {
        tracing::debug!(
            "Calendar page has {} day(s) but no attendance data for {}; trying async postback for full grid",
            days.len(),
            month.format("%Y-%m")
        );
        // The calendar grid lazy-loads via ASP.NET UpdatePanel. The RefreshPeriod
        // button is inside an UpdatePanel with ChildrenAsTriggers=true, so clicking
        // it triggers an async postback. The delta response contains the full grid.
        match load_full_grid_async(client, &url, &fields, month).await {
            Ok(async_days) if !async_days.is_empty() => {
                days = async_days;
            }
            Ok(_) => {
                tracing::debug!(
                    "async postback returned no days, falling back to day-by-day probe"
                );
                days = probe_month_days(client, &url, &fields, month).await?;
            }
            Err(e) => {
                tracing::debug!("async postback failed: {e}, falling back to day-by-day probe");
                days = probe_month_days(client, &url, &fields, month).await?;
            }
        }
    }
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

async fn load_month_page(
    client: &mut HilanClient,
    url: &str,
    mut html: String,
    mut fields: std::collections::BTreeMap<String, String>,
    requested_month: NaiveDate,
) -> Result<(String, std::collections::BTreeMap<String, String>)> {
    let requested_month = month_start(requested_month)?;
    let mut current_month = displayed_month(&fields)?;

    if current_month == requested_month {
        return Ok((html, fields));
    }

    let month_value = month_field_value(requested_month);
    let selected_day = calendar_selected_day_value(requested_month);

    // First try the most direct path: set the hidden month field and submit the
    // standard period refresh. This is not constrained by the visible month list
    // window, so it can jump to months beyond the +/-12 range shown in the UI.
    match client
        .post_aspx_form(
            url,
            &fields,
            &[
                ("__calendarSelectedDays", selected_day.as_str()),
                ("ctl00$mp$currentMonth", month_value.as_str()),
            ],
            "ctl00$mp$RefreshPeriod",
            "דיווח תקופתי",
            true,
        )
        .await
    {
        Ok(new_html) => {
            let new_fields = crate::client::parse_aspx_form_fields(&new_html);
            if let Ok(displayed) = displayed_month(&new_fields) {
                if displayed == requested_month {
                    tracing::debug!(
                        "direct month refresh to {} succeeded",
                        requested_month.format("%Y-%m")
                    );
                    return Ok((new_html, new_fields));
                }
                tracing::debug!(
                    "direct month refresh landed on {} instead of {}; trying other navigation paths",
                    displayed.format("%Y-%m"),
                    requested_month.format("%Y-%m")
                );
                html = new_html;
                fields = new_fields;
                current_month = displayed;
            }
        }
        Err(err) => {
            tracing::debug!(
                "direct month refresh to {} failed: {err}",
                requested_month.format("%Y-%m")
            );
        }
    }

    // Next try the dropdown month-change event. This works when the target month
    // is present in Hilan's visible month-list window and is still a single POST.
    match client
        .post_aspx_event(
            url,
            &fields,
            &[
                ("__calendarSelectedDays", selected_day.as_str()),
                ("ctl00$mp$currentMonth", month_value.as_str()),
            ],
            "ctl00$mp$calendar_monthChanged",
            &month_value,
            true,
        )
        .await
    {
        Ok(new_html) => {
            let new_fields = crate::client::parse_aspx_form_fields(&new_html);
            if let Ok(displayed) = displayed_month(&new_fields) {
                if displayed == requested_month {
                    tracing::debug!(
                        "direct dropdown jump to {} succeeded",
                        requested_month.format("%Y-%m")
                    );
                    return Ok((new_html, new_fields));
                }
                tracing::debug!(
                    "direct dropdown jump landed on {} instead of {}; falling back to step-by-step",
                    displayed.format("%Y-%m"),
                    requested_month.format("%Y-%m")
                );
                // Use the jumped page as starting point if it moved closer.
                html = new_html;
                fields = new_fields;
                current_month = displayed;
            }
        }
        Err(err) => {
            tracing::debug!(
                "direct dropdown jump to {} failed: {err}",
                requested_month.format("%Y-%m")
            );
        }
    }

    // Step-by-step fallback (or finish remaining distance after partial jump)
    const MAX_NAV_STEPS: u32 = 24;
    let mut nav_step = 0u32;
    while current_month != requested_month {
        nav_step += 1;
        if nav_step > MAX_NAV_STEPS {
            return Err(anyhow!(
                "calendar navigation exceeded {MAX_NAV_STEPS} steps (stuck at {})",
                current_month.format("%Y-%m")
            ));
        }
        let go_forward = current_month < requested_month;
        let next_month = shift_month(current_month, if go_forward { 1 } else { -1 })?;
        let selected_day = calendar_selected_day_value(next_month);
        let month_value = month_field_value(next_month);
        let event_target = if go_forward {
            "ctl00_mp_calendar_next"
        } else {
            "ctl00_mp_calendar_prev"
        };

        html = client
            .post_aspx_event(
                url,
                &fields,
                &[
                    ("__calendarSelectedDays", selected_day.as_str()),
                    ("ctl00$mp$currentMonth", month_value.as_str()),
                ],
                event_target,
                "",
                true,
            )
            .await
            .with_context(|| {
                format!(
                    "navigate attendance calendar from {} to {}",
                    current_month.format("%Y-%m"),
                    next_month.format("%Y-%m")
                )
            })?;
        fields = crate::client::parse_aspx_form_fields(&html);
        dump_debug_html(&format!("nav-{}", next_month.format("%Y-%m")), &html);
        let displayed = displayed_month(&fields)?;
        if displayed == current_month {
            return Err(anyhow!(
                "calendar navigation via {} did not change displayed month from {}",
                event_target,
                current_month.format("%Y-%m")
            ));
        }
        current_month = displayed;
    }

    Ok((html, fields))
}

/// Try to load the full calendar grid via ASP.NET async postback.
///
/// The calendar page uses UpdatePanel for the grid. RefreshPeriod (period view)
/// is inside an UpdatePanel with ChildrenAsTriggers=true, meaning the browser
/// sends it as an async postback. The response is in ASP.NET delta format with
/// the full grid HTML embedded in the updatePanel entries.
async fn load_full_grid_async(
    client: &mut HilanClient,
    url: &str,
    fields: &std::collections::BTreeMap<String, String>,
    month: NaiveDate,
) -> Result<Vec<CalendarDay>> {
    let month_value = month_field_value(month);

    let delta = client
        .post_aspx_async(
            url,
            fields,
            &[
                ("ctl00$mp$currentMonth", month_value.as_str()),
                ("ctl00$mp$RefreshPeriod", "דיווח תקופתי"),
            ],
            "ctl00$ms",
            "ctl00$mp$RefreshPeriod",
        )
        .await
        .context("async postback for RefreshPeriod")?;

    dump_debug_html(&format!("async-{}", month.format("%Y-%m")), &delta);

    // If the response looks like full HTML (not delta), try parsing it directly
    if delta.contains("<html") || delta.contains("<!DOCTYPE") {
        return parse_calendar_html(&delta, month);
    }

    // Parse delta once and search for grid content
    let entries = crate::client::parse_aspx_delta(&delta);
    tracing::debug!(
        "async postback delta has {} entries: {:?}",
        entries.len(),
        entries.keys().collect::<Vec<_>>()
    );

    // First pass: check panels with known grid IDs
    for ((entry_type, entry_id), content) in &entries {
        if entry_type == "updatePanel"
            && (entry_id.contains("upGrid") || entry_id.contains("reportsGrid_bodyUpdate"))
        {
            let panel_days = parse_calendar_html(content, month)?;
            if !panel_days.is_empty() {
                return Ok(panel_days);
            }
        }
    }
    // Fallback: scan all updatePanel content for row data
    for ((entry_type, _), content) in &entries {
        if entry_type == "updatePanel" && content.contains("row_") {
            let panel_days = parse_calendar_html(content, month)?;
            if !panel_days.is_empty() {
                return Ok(panel_days);
            }
        }
    }

    Ok(Vec::new())
}

async fn probe_month_days(
    client: &mut HilanClient,
    url: &str,
    fields: &std::collections::BTreeMap<String, String>,
    month: NaiveDate,
) -> Result<Vec<CalendarDay>> {
    let month = month_start(month)?;
    let mut fields = fields.clone();
    let mut days = Vec::with_capacity(days_in_month(month) as usize);

    for day_num in 1..=days_in_month(month) {
        let date = NaiveDate::from_ymd_opt(month.year(), month.month(), day_num)
            .ok_or_else(|| anyhow!("invalid date while probing month: {month} + day {day_num}"))?;
        let selected_day = calendar_selected_day_value(date);
        let month_value = month_field_value(date);
        let html = client
            .post_aspx_form(
                url,
                &fields,
                &[
                    ("__calendarSelectedDays", selected_day.as_str()),
                    ("ctl00$mp$currentMonth", month_value.as_str()),
                ],
                "ctl00$mp$RefreshSelectedDays",
                "ימים נבחרים",
                true, // read-only navigation, safe to retry
            )
            .await
            .with_context(|| format!("load attendance details for {}", date.format("%Y-%m-%d")))?;
        fields = crate::client::parse_aspx_form_fields(&html);

        if day_num <= 3 {
            dump_debug_html(&format!("probe-{}-{:02}", month.format("%Y-%m"), day_num), &html);
        }
        let parsed_day = parse_calendar_html(&html, month)?
            .into_iter()
            .find(|candidate| candidate.date == date)
            .unwrap_or_else(|| blank_calendar_day(date));
        days.push(parsed_day);
    }

    Ok(days)
}

/// Parse the calendar HTML grid and extract day information.
fn parse_calendar_html(html: &str, month: NaiveDate) -> Result<Vec<CalendarDay>> {
    let document = Html::parse_document(html);
    let mut days = parse_calendar_grid(&document, month);

    if days.is_empty() {
        // Fallback for the tabular attendance grid, which appears in some
        // postback/update-panel responses instead of the visual month calendar.
        let row_sel = Selector::parse("tr").map_err(|e| anyhow!("selector parse error: {e}"))?;
        for row in document.select(&row_sel) {
            let cells: Vec<String> = row
                .children()
                .filter_map(ElementRef::wrap)
                .filter(|cell| cell.value().name() == "td")
                .map(|cell| cell_visible_text(&cell))
                .collect();

            if cells.len() < 3 {
                continue;
            }

            if let Some(day) = try_parse_day_row(&cells, month) {
                days.push(day);
            }
        }
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
                    source: preferred_source(b.source, a.source),
                };
            } else {
                a.source = preferred_source(a.source, b.source);
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

    let source = if let Some(attendance_type) = attendance_type.as_deref() {
        source_from_calendar_state("", None, Some(attendance_type))
    } else if entry_time.is_some() {
        hr_core::AttendanceSource::UserReported
    } else {
        hr_core::AttendanceSource::Unreported
    };

    Some(CalendarDay {
        date,
        day_name,
        has_error,
        error_message,
        entry_time,
        exit_time,
        attendance_type,
        total_hours,
        source,
    })
}

/// Parse the Hilan visual calendar grid (month view with clickable day cells).
///
/// The calendar renders each day as a `<td>` with:
/// - `aria-label="N"` — the day number
/// - `title="H:MM"` — clock-in time (if present)
/// - `class` containing `cDIES` or similar calendar day class
/// - `Days="NNNNN"` — day index (epoch-based)
/// - Nested: `<td class="dTS">N</td>` — displayed day number
/// - Nested: `<span class="calendarIcon mx-1 fh-x">` — absence/vacation icon
/// - Nested: `<span class="calendarIcon mx-1 fh-error">` — error icon
/// - Nested: `<div class="cDM">H:MM</div>` — clock-in time display
fn parse_calendar_grid(document: &Html, month: NaiveDate) -> Vec<CalendarDay> {
    let mut days = Vec::new();

    // Target calendar day cells: both regular (cDIES) and absence (calendarAbcenseDay).
    // Both have the `Days` attribute.
    let day_cell_sel = Selector::parse("td[Days]").unwrap();
    let day_num_sel = Selector::parse("tr.dayImageNumberContainer td.dTS").unwrap();
    let message_sel = Selector::parse("td.calendarMessageCell .cDM").unwrap();
    let icon_sel = Selector::parse("td.imageContainerStyle span.calendarIcon").unwrap();

    for td in document.select(&day_cell_sel) {
        // Prefer the visible day-number cell over outer attributes.
        let day_num = td
            .select(&day_num_sel)
            .next()
            .map(|cell| cell_visible_text(&cell))
            .as_deref()
            .and_then(extract_day_number_strict)
            .or_else(|| {
                td.value()
                    .attr("aria-label")
                    .and_then(|label| label.parse::<u32>().ok())
                    .filter(|&n| (1..=31).contains(&n))
            });

        let day_num = match day_num {
            Some(n) => n,
            None => continue,
        };

        let date = match NaiveDate::from_ymd_opt(month.year(), month.month(), day_num) {
            Some(d) => d,
            None => continue,
        };

        // Determine attendance source from CSS class:
        // - "calendarAbcenseDay" = user-reported absence (vacation, sickness, etc.)
        // - "cED" = system auto-filled (user didn't report)
        // - neither = user-reported work day or unreported
        let td_class = td.value().attr("class").unwrap_or("");
        let is_absence_day = td_class.contains("calendarAbcenseDay");
        let is_auto_filled = td_class.contains("cED");

        let mut has_error = false;
        let mut error_message = None;
        let mut attendance_type = None;

        let message_text = td
            .select(&message_sel)
            .map(|cell| cell_visible_text(&cell))
            .map(|text| text.trim().to_string())
            .find(|text| !text.is_empty());

        // The title attribute contains either:
        // - A clock-in time like "9:07" for days with physical clock-in
        // - An attendance type name like "work from home" for manually reported days
        // - Empty/missing for truly unreported days
        let title_value = td
            .value()
            .attr("title")
            .map(str::trim)
            .filter(|t| !t.is_empty());
        let mut entry_time = title_value
            .filter(|title| is_time_pattern(title))
            .map(ToOwned::to_owned);

        // If title is not a time, it's an attendance type name
        if entry_time.is_none() {
            if let Some(title) = title_value {
                if !is_time_pattern(title) {
                    attendance_type = Some(title.to_string());
                }
            }
        }

        if entry_time.is_none() {
            entry_time = message_text
                .as_deref()
                .filter(|text| is_time_pattern(text))
                .map(ToOwned::to_owned);
        }

        for icon in td.select(&icon_sel) {
            let icon_class = icon.value().attr("class").unwrap_or("");
            if icon_class.contains("fh-x") {
                // "x" icon = system auto-filled as vacation
                if attendance_type.is_none() {
                    attendance_type = Some(LABEL_VACATION.to_string());
                }
            } else if icon_class.contains("fh-error") || icon_class.contains("fh-warning") {
                has_error = true;
            }
        }

        if let Some(message) = &message_text {
            let lower = message.to_lowercase();
            if message.contains("שגיאה") || lower.contains("error") || lower.contains("missing")
            {
                has_error = true;
                error_message = Some(message.clone());
            } else if attendance_type.is_none() {
                for kw in TYPE_KEYWORDS {
                    if lower.contains(kw) {
                        attendance_type = Some(message.clone());
                        break;
                    }
                }
            }
        }

        // If we have a clock-in time but no attendance type, it's a work day
        let exit_time = None;
        if entry_time.is_some() && attendance_type.is_none() {
            attendance_type = Some(LABEL_WORK_DAY.to_string());
        }

        let source = if is_auto_filled {
            hr_core::AttendanceSource::SystemAutoFill
        } else if is_absence_day {
            hr_core::AttendanceSource::UserReported
        } else {
            source_from_calendar_state(td_class, title_value, attendance_type.as_deref())
        };

        days.push(CalendarDay {
            date,
            day_name: date.format("%a").to_string(),
            has_error,
            error_message,
            entry_time,
            exit_time,
            attendance_type,
            total_hours: None,
            source,
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

fn preferred_source(
    left: hr_core::AttendanceSource,
    right: hr_core::AttendanceSource,
) -> hr_core::AttendanceSource {
    fn priority(source: hr_core::AttendanceSource) -> u8 {
        match source {
            hr_core::AttendanceSource::Unreported => 0,
            hr_core::AttendanceSource::SystemAutoFill => 1,
            hr_core::AttendanceSource::Holiday => 2,
            hr_core::AttendanceSource::UserReported => 3,
        }
    }

    if priority(left) >= priority(right) {
        left
    } else {
        right
    }
}

fn source_from_calendar_state(
    td_class: &str,
    title_value: Option<&str>,
    attendance_type: Option<&str>,
) -> hr_core::AttendanceSource {
    if title_value.is_some() || attendance_type.is_some() {
        if td_class.contains("cHD")
            || td_class.contains("holiday")
            || is_holiday_text(title_value.or(attendance_type).unwrap_or_default())
        {
            hr_core::AttendanceSource::Holiday
        } else {
            hr_core::AttendanceSource::UserReported
        }
    } else {
        hr_core::AttendanceSource::Unreported
    }
}

fn is_holiday_text(value: &str) -> bool {
    let lower = value.to_lowercase();
    lower.contains("חג")
        || lower.contains("d.off")
        || lower.contains("day off")
        || lower
            .split_whitespace()
            .any(|part| part == "ho" || part == "h.o")
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

fn displayed_month(fields: &std::collections::BTreeMap<String, String>) -> Result<NaiveDate> {
    let raw = fields
        .get("ctl00$mp$currentMonth")
        .ok_or_else(|| anyhow!("calendar page is missing ctl00$mp$currentMonth"))?;
    let parsed = NaiveDate::parse_from_str(raw, "%d/%m/%Y")
        .with_context(|| format!("parse displayed month from '{raw}'"))?;
    month_start(parsed)
}

fn month_start(date: NaiveDate) -> Result<NaiveDate> {
    NaiveDate::from_ymd_opt(date.year(), date.month(), 1)
        .ok_or_else(|| anyhow!("invalid month start for {}", date.format("%Y-%m-%d")))
}

fn shift_month(date: NaiveDate, months: i32) -> Result<NaiveDate> {
    let month_index = date.year() * 12 + date.month0() as i32 + months;
    let year = month_index.div_euclid(12);
    let month0 = month_index.rem_euclid(12);
    NaiveDate::from_ymd_opt(year, month0 as u32 + 1, 1).ok_or_else(|| {
        anyhow!(
            "failed to shift month {} by {}",
            date.format("%Y-%m-%d"),
            months
        )
    })
}

fn days_in_month(date: NaiveDate) -> u32 {
    let current = month_start(date).expect("valid month start");
    let next = shift_month(current, 1).expect("next month");
    next.signed_duration_since(current).num_days() as u32
}

fn calendar_selected_day_value(date: NaiveDate) -> String {
    // Inferred from the captured Hilanet payload where 2026-04-10 maps to 9596.
    let epoch = NaiveDate::from_ymd_opt(2000, 1, 1).unwrap();
    date.signed_duration_since(epoch).num_days().to_string()
}

fn blank_calendar_day(date: NaiveDate) -> CalendarDay {
    CalendarDay {
        date,
        day_name: date.format("%a").to_string(),
        has_error: false,
        error_message: None,
        entry_time: None,
        exit_time: None,
        attendance_type: None,
        total_hours: None,
        source: hr_core::AttendanceSource::Unreported,
    }
}

fn dump_debug_html(label: &str, content: &str) {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if !ENABLED.get_or_init(|| std::env::var("SHAON_DUMP_HTML").is_ok()) {
        return;
    }
    let dump_path = format!("/tmp/shaon-{label}.html");
    let _ = std::fs::write(&dump_path, content);
    tracing::debug!("{label}: {} bytes → {dump_path}", content.len());
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
        "Attendance type '{name}' needs cached ontology. Run `shaon sync-types` first or pass a numeric code."
    );
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

    #[test]
    fn visual_calendar_cell_parses_work_day_from_message_text() {
        let html = r#"
            <html><body><table><tbody><tr>
                <td class="cDIES" Days="9437" tabindex="0" aria-label="2">
                    <table class="iDSIE">
                        <tr class="dayImageNumberContainer">
                            <td class="dTS">2</td>
                            <td class="imageContainerStyle"></td>
                        </tr>
                        <tr>
                            <td class="calendarMessageCell" colspan="2"><div class="cDM">6:55</div></td>
                        </tr>
                    </table>
                </td>
            </tr></tbody></table></body></html>
        "#;
        let month = NaiveDate::from_ymd_opt(2025, 11, 1).unwrap();

        let days = parse_calendar_html(html, month).expect("parse visual calendar work day");
        let day = days
            .iter()
            .find(|day| day.date == NaiveDate::from_ymd_opt(2025, 11, 2).unwrap())
            .expect("day 2025-11-02");

        assert_eq!(day.entry_time.as_deref(), Some("6:55"));
        assert_eq!(day.attendance_type.as_deref(), Some("work day"));
        assert_eq!(day.source, hr_core::AttendanceSource::UserReported);
        assert!(day.is_reported());
        assert!(!day.has_error);
    }

    #[test]
    fn visual_calendar_cell_parses_fh_x_as_system_auto_fill() {
        let html = r#"
            <html><body><table><tbody><tr>
                <td class="cDIES cED" Days="9441" tabindex="0" aria-label="6">
                    <table class="iDSIE">
                        <tr class="dayImageNumberContainer">
                            <td class="dTS">6</td>
                            <td class="imageContainerStyle">
                                <span class="calendarIcon mx-1 fh-x"></span>
                            </td>
                        </tr>
                        <tr>
                            <td class="calendarMessageCell" colspan="2"><div class="cDM">&nbsp;</div></td>
                        </tr>
                    </table>
                </td>
            </tr></tbody></table></body></html>
        "#;
        let month = NaiveDate::from_ymd_opt(2025, 11, 1).unwrap();

        let days = parse_calendar_html(html, month).expect("parse visual calendar absence");
        let day = days
            .iter()
            .find(|day| day.date == NaiveDate::from_ymd_opt(2025, 11, 6).unwrap())
            .expect("day 2025-11-06");

        assert_eq!(day.attendance_type.as_deref(), Some("vacation"));
        assert_eq!(day.source, hr_core::AttendanceSource::SystemAutoFill);
        assert!(day.is_auto_filled());
        assert!(!day.is_reported());
        assert!(!day.has_error);
        assert!(day.entry_time.is_none());
    }

    #[test]
    fn visual_calendar_cell_parses_calendar_absence_day_as_user_reported() {
        let html = r#"
            <html><body><table><tbody><tr>
                <td class="calendarAbcenseDay" Days="9255" tabindex="0" aria-label="4" title="vacation">
                    <table class="iDSIE">
                        <tr class="dayImageNumberContainer">
                            <td class="dTS">4</td>
                            <td class="imageContainerStyle"></td>
                        </tr>
                        <tr>
                            <td class="calendarMessageCell" colspan="2"><div class="cDM">&nbsp;</div></td>
                        </tr>
                    </table>
                </td>
            </tr></tbody></table></body></html>
        "#;
        let month = NaiveDate::from_ymd_opt(2025, 4, 1).unwrap();

        let days = parse_calendar_html(html, month).expect("parse user-reported vacation");
        let day = days
            .iter()
            .find(|day| day.date == NaiveDate::from_ymd_opt(2025, 4, 4).unwrap())
            .expect("day 2025-04-04");

        assert_eq!(day.attendance_type.as_deref(), Some("vacation"));
        assert_eq!(day.source, hr_core::AttendanceSource::UserReported);
        assert!(day.is_reported());
        assert!(!day.is_auto_filled());
    }

    #[test]
    fn visual_calendar_cell_parses_holiday_as_holiday_source() {
        let html = r#"
            <html><body><table><tbody><tr>
                <td class="cHD" Days="9521" tabindex="0" aria-label="25" title="חג">
                    <table class="iDSIE">
                        <tr class="dayImageNumberContainer">
                            <td class="dTS">25</td>
                            <td class="imageContainerStyle"></td>
                        </tr>
                        <tr>
                            <td class="calendarMessageCell" colspan="2"><div class="cDM">חג</div></td>
                        </tr>
                    </table>
                </td>
            </tr></tbody></table></body></html>
        "#;
        let month = NaiveDate::from_ymd_opt(2025, 9, 1).unwrap();

        let days = parse_calendar_html(html, month).expect("parse holiday cell");
        let day = days
            .iter()
            .find(|day| day.date == NaiveDate::from_ymd_opt(2025, 9, 25).unwrap())
            .expect("day 2025-09-25");

        assert_eq!(day.attendance_type.as_deref(), Some("חג"));
        assert_eq!(day.source, hr_core::AttendanceSource::Holiday);
        assert!(day.is_reported());
        assert!(!day.is_auto_filled());
    }

    #[test]
    fn displayed_month_reads_hidden_calendar_month() {
        let fields = std::collections::BTreeMap::from([(
            "ctl00$mp$currentMonth".to_string(),
            "01/04/2026".to_string(),
        )]);

        assert_eq!(
            displayed_month(&fields).unwrap(),
            NaiveDate::from_ymd_opt(2026, 4, 1).unwrap()
        );
    }

    #[test]
    fn shift_month_moves_across_year_boundaries() {
        assert_eq!(
            shift_month(NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(), -1).unwrap(),
            NaiveDate::from_ymd_opt(2025, 12, 1).unwrap()
        );
        assert_eq!(
            shift_month(NaiveDate::from_ymd_opt(2026, 12, 1).unwrap(), 1).unwrap(),
            NaiveDate::from_ymd_opt(2027, 1, 1).unwrap()
        );
    }

    #[test]
    fn days_in_month_handles_leap_and_non_leap_february() {
        assert_eq!(
            days_in_month(NaiveDate::from_ymd_opt(2024, 2, 1).unwrap()),
            29
        );
        assert_eq!(
            days_in_month(NaiveDate::from_ymd_opt(2025, 2, 1).unwrap()),
            28
        );
    }
}
