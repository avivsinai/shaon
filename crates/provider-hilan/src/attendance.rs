use std::collections::{btree_map::Entry, BTreeMap};
use std::sync::LazyLock;

use anyhow::{anyhow, bail, Context, Result};
use chrono::{DateTime, Datelike, NaiveDate, Utc};
use scraper::{ElementRef, Html, Selector};
use serde::{Deserialize, Serialize};

use crate::client::{format_form_fields_for_display, HilanClient};

// Pre-compiled scraper selectors. Building a Selector allocates, so cache the
// constant-string ones at module scope to keep hot parsing paths allocation-free.
static DAY_CELL_SEL: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("td[Days]").expect("valid selector"));
static DAY_NUM_SEL: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("tr.dayImageNumberContainer td.dTS").expect("valid selector"));
static CAL_MESSAGE_SEL: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("td.calendarMessageCell .cDM").expect("valid selector"));
static CAL_ICON_SEL: LazyLock<Selector> = LazyLock::new(|| {
    Selector::parse("td.imageContainerStyle span.calendarIcon").expect("valid selector")
});
static REPORT_DAY_ROW_SEL: LazyLock<Selector> = LazyLock::new(|| {
    Selector::parse(r#"tr[id*="_RG_Days_"][id$="_row_0"]"#).expect("valid selector")
});
static REPORT_DATE_CELL_SEL: LazyLock<Selector> = LazyLock::new(|| {
    Selector::parse(r#"td[id*="cellOf_ReportDate_row_"]"#).expect("valid selector")
});
static TABLE_CELL_SEL: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("td").expect("valid selector"));
static INPUT_SEL: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("input").expect("valid selector"));
static SELECT_SEL: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("select").expect("valid selector"));
static OPTION_SELECTED_SEL: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("option[selected]").expect("valid selector"));
static OPTION_ANY_SEL: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("option").expect("valid selector"));
#[cfg(test)]
static ROW_0_TOP_CELL_SEL: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse(r#"tr[id$="_row_0"] > td"#).expect("valid selector"));

/// Default attendance type labels synthesized by the parser.
const LABEL_WORK_DAY: &str = "work day";
const LABEL_VACATION: &str = "vacation";
const EMPTY_OBJECT_ID: &str = "00000000-0000-0000-0000-000000000000";
const CONFLICTING_REPORT_MESSAGE_FRAGMENT: &str = "קיים דיווח";
const CALENDAR_BROWSER_FIELDS: &[&str] = &[
    "DisableTimeout",
    "H-XSRF-Token",
    "Time",
    "__EVENTARGUMENT",
    "__EVENTTARGET",
    "__LASTFOCUS",
    "__VIEWSTATE",
    "__VIEWSTATEGENERATOR",
    "__calendarSelectedDays",
    "ctl00$DummyAutoCompleteText",
    "ctl00$DummyAutoComplete_Value",
    "ctl00$mp$Strip$ACESearch_Value",
    "ctl00$mp$Strip$blSaveList",
    "ctl00$mp$Strip$hCurrentItemId",
    "ctl00$mp$Strip$hSelectedIds",
    "ctl00$mp$Strip$txtSearch",
    "ctl00$mp$currentMonth",
    "ctl00$mp$scriptBox",
    "ctl00$datePickerTmp$jdatePicker",
    "ctl00_datePickerTmp_State",
];
const ERROR_WIZARD_BROWSER_FIELDS: &[&str] = &[
    "DisableTimeout",
    "H-XSRF-Token",
    "Time",
    "__EVENTARGUMENT",
    "__EVENTTARGET",
    "__VIEWSTATE",
    "__VIEWSTATEGENERATOR",
    "ctl00$DummyAutoCompleteText",
    "ctl00$DummyAutoComplete_Value",
    "ctl00$mp$ScriptBox",
    "ctl00$datePickerTmp$jdatePicker",
    "ctl00_datePickerTmp_State",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SubmitPage {
    Calendar,
    ErrorWizard,
}

#[derive(Debug, Clone)]
pub(crate) struct CalendarSubmitContext {
    pub url: String,
    pub date: NaiveDate,
    pub employee_id: String,
    pub fields: BTreeMap<String, String>,
    pub reports: Vec<CalendarExistingReport>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CalendarExistingReport {
    pub object_id: String,
    pub employee_id: u32,
    pub row_name: String,
    pub symbol_code: Option<String>,
    pub symbol_name: Option<String>,
    pub is_absence: bool,
    pub report_date_iso_utc: String,
}

#[derive(Debug, Clone)]
struct ParsedCalendarReportRow {
    row_name: String,
    symbol_code: Option<String>,
    symbol_name: Option<String>,
    is_absence: bool,
}

#[derive(Debug, Clone, Deserialize)]
struct RowDataSymbol {
    #[serde(rename = "First")]
    first: Option<String>,
    #[serde(rename = "Second")]
    second: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct RowDataEntry {
    #[serde(rename = "ID")]
    id: String,
    #[serde(rename = "EmployeeId")]
    employee_id: u32,
    #[serde(rename = "ReportDate")]
    report_date: String,
    #[serde(rename = "FromDate")]
    from_date: Option<String>,
    #[serde(rename = "IsRange")]
    is_range: bool,
    #[serde(rename = "IsReportDeleted")]
    is_report_deleted: bool,
    #[serde(rename = "Symbol")]
    symbol: Option<RowDataSymbol>,
}

fn browser_field_allowlist(page: SubmitPage) -> &'static [&'static str] {
    match page {
        SubmitPage::Calendar => CALENDAR_BROWSER_FIELDS,
        SubmitPage::ErrorWizard => ERROR_WIZARD_BROWSER_FIELDS,
    }
}

fn retain_browser_fields(replay_fields: &mut BTreeMap<String, String>, allowlist: &[&str]) {
    replay_fields.retain(|key, _| allowlist.contains(&key.as_str()));
}

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
    mut fields: BTreeMap<String, String>,
    requested_month: NaiveDate,
) -> Result<(String, BTreeMap<String, String>)> {
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
    fields: &BTreeMap<String, String>,
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
    fields: &BTreeMap<String, String>,
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
            dump_debug_html(
                &format!("probe-{}-{:02}", month.format("%Y-%m"), day_num),
                &html,
            );
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
    days.extend(parse_calendar_detail_rows_from_dom(&document, month));

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

    let days = merge_calendar_days(days);

    Ok(days)
}

fn merge_calendar_days(days: Vec<CalendarDay>) -> Vec<CalendarDay> {
    let mut merged = BTreeMap::new();
    for day in days {
        match merged.entry(day.date) {
            Entry::Vacant(entry) => {
                entry.insert(day);
            }
            Entry::Occupied(mut entry) => {
                merge_calendar_day_into(entry.get_mut(), day);
            }
        }
    }
    merged.into_values().collect()
}

fn merge_calendar_day_into(existing: &mut CalendarDay, candidate: CalendarDay) {
    let candidate_source = candidate.source;

    if !candidate.day_name.is_empty() {
        existing.day_name = candidate.day_name;
    }
    existing.has_error |= candidate.has_error;
    if candidate.error_message.is_some() {
        existing.error_message = candidate.error_message;
    }
    if candidate.entry_time.is_some() {
        existing.entry_time = candidate.entry_time;
    }
    if candidate.exit_time.is_some() {
        existing.exit_time = candidate.exit_time;
    }
    if candidate.attendance_type.is_some() {
        existing.attendance_type = candidate.attendance_type;
    }
    if candidate.total_hours.is_some() {
        existing.total_hours = candidate.total_hours;
    }
    existing.source = preferred_source(existing.source, candidate_source);
}

fn parse_calendar_detail_rows_from_dom(document: &Html, month: NaiveDate) -> Vec<CalendarDay> {
    let mut days = Vec::new();

    for row in document.select(&REPORT_DAY_ROW_SEL) {
        let Some(date) = parse_detail_row_date(&row, month) else {
            continue;
        };

        let entry_time = detail_input_value(&row, "ManualEntry_EmployeeReports")
            .or_else(|| detail_cell_time(&row, "cellOf_ManualEntry_EmployeeReports"));
        let exit_time = detail_input_value(&row, "ManualExit_EmployeeReports")
            .or_else(|| detail_cell_time(&row, "cellOf_ManualExit_EmployeeReports"));
        let total_hours = detail_cell_time(&row, "cellOf_ManualTotal_EmployeeReports");
        let attendance_type = detail_cell_text(&row, "cellOf_Symbol.SymbolId_EmployeeReports")
            .or_else(|| detail_select_text(&row, "Symbol.SymbolId_EmployeeReports"));

        if entry_time.is_none()
            && exit_time.is_none()
            && total_hours.is_none()
            && attendance_type.is_none()
        {
            continue;
        }

        let source = if attendance_type.is_some() || entry_time.is_some() || exit_time.is_some() {
            source_from_calendar_state("", None, attendance_type.as_deref())
        } else {
            hr_core::AttendanceSource::Unreported
        };

        days.push(CalendarDay {
            date,
            day_name: date.format("%a").to_string(),
            has_error: false,
            error_message: None,
            entry_time,
            exit_time,
            attendance_type,
            total_hours,
            source,
        });
    }

    days
}

fn parse_detail_row_date(row: &ElementRef<'_>, month: NaiveDate) -> Option<NaiveDate> {
    let cell = row.select(&REPORT_DATE_CELL_SEL).next()?;
    let day_num = cell
        .value()
        .attr("ov")
        .and_then(extract_day_number)
        .or_else(|| cell.value().attr("title").and_then(extract_day_number))
        .or_else(|| extract_day_number(&cell_visible_text(&cell)))?;

    NaiveDate::from_ymd_opt(month.year(), month.month(), day_num)
}

fn detail_input_value(row: &ElementRef<'_>, field_fragment: &str) -> Option<String> {
    row.select(&INPUT_SEL)
        .find(|input| element_attr_contains(input, "name", field_fragment))
        .or_else(|| {
            row.select(&INPUT_SEL)
                .find(|input| element_attr_contains(input, "id", field_fragment))
        })
        .and_then(|input| normalized_cell_value(input.value().attr("value")))
}

fn detail_cell_time(row: &ElementRef<'_>, cell_fragment: &str) -> Option<String> {
    detail_cell_text(row, cell_fragment).filter(|text| is_time_pattern(text))
}

fn detail_cell_text(row: &ElementRef<'_>, cell_fragment: &str) -> Option<String> {
    row.select(&TABLE_CELL_SEL)
        .find(|cell| element_attr_contains(cell, "id", cell_fragment))
        .and_then(|cell| {
            let text = cell_visible_text(&cell);
            normalized_cell_value(Some(text.as_str()))
                .or_else(|| normalized_cell_value(cell.value().attr("ov")))
        })
}

fn detail_select_text(row: &ElementRef<'_>, field_fragment: &str) -> Option<String> {
    let select = row
        .select(&SELECT_SEL)
        .find(|select| element_attr_contains(select, "name", field_fragment))
        .or_else(|| {
            row.select(&SELECT_SEL)
                .find(|select| element_attr_contains(select, "id", field_fragment))
        })?;

    select
        .select(&OPTION_SELECTED_SEL)
        .next()
        .or_else(|| {
            select
                .select(&OPTION_ANY_SEL)
                .find(|option| option.value().attr("value").is_some_and(|v| !v.is_empty()))
        })
        .or_else(|| select.select(&OPTION_ANY_SEL).next())
        .and_then(|option| normalized_cell_value(Some(&option.text().collect::<String>())))
}

fn element_attr_contains(element: &ElementRef<'_>, attr: &str, needle: &str) -> bool {
    element
        .value()
        .attr(attr)
        .is_some_and(|value| value.contains(needle))
}

fn normalized_cell_value(value: Option<&str>) -> Option<String> {
    let value = value?.replace('\u{a0}', " ");
    let value = value.trim();
    if value.is_empty() || value == "--:--" || value == "&nbsp;" {
        None
    } else {
        Some(value.to_string())
    }
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
    for td in document.select(&DAY_CELL_SEL) {
        // Prefer the visible day-number cell over outer attributes.
        let day_num = td
            .select(&DAY_NUM_SEL)
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
            .select(&CAL_MESSAGE_SEL)
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

        for icon in td.select(&CAL_ICON_SEL) {
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
    // If the cell contains no <select>, use the normal text extraction.
    let has_select = cell.select(&SELECT_SEL).next().is_some();
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
        parts: &mut Vec<String>,
    ) {
        let current_ov = normalized_ov(element.value().attr("ov")).or(inherited_ov);

        if element.value().name() == "select" {
            let selected_text = element
                .select(&OPTION_SELECTED_SEL)
                .next()
                .or_else(|| {
                    current_ov.and_then(|ov| {
                        element
                            .select(&OPTION_ANY_SEL)
                            .find(|opt| opt.value().attr("value") == Some(ov))
                    })
                })
                .or_else(|| element.select(&OPTION_ANY_SEL).next())
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
                collect_visible_parts(&child_el, current_ov, parts);
            } else if let Some(text_node) = child.value().as_text() {
                let text = text_node.trim();
                if !text.is_empty() {
                    parts.push(text.to_string());
                }
            }
        }
    }

    let mut parts = Vec::new();
    collect_visible_parts(cell, None, &mut parts);
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
    let context = load_calendar_submit_context(client, submit.date).await?;
    submit_day_with_context(client, submit, execute, context).await
}

pub(crate) async fn submit_day_with_context(
    client: &mut HilanClient,
    submit: &AttendanceSubmit,
    execute: bool,
    mut context: CalendarSubmitContext,
) -> Result<SubmitPreview> {
    let mut steps = Vec::new();
    let mut deleted_conflict = false;

    if let Some(desired_type_code) = submit.attendance_type_code.as_deref() {
        let (delete_previews, refreshed_context) =
            delete_conflicting_absence_reports(client, context, desired_type_code, execute).await?;
        deleted_conflict = !delete_previews.is_empty();
        steps.extend(delete_previews.into_iter().map(|preview| {
            (
                "delete the conflicting calendar row before applying the requested attendance",
                preview,
            )
        }));
        context = refreshed_context;
    }

    let object_id = calendar_submit_object_id(&context).to_string();
    let submit_result = replay_submit_with_fields(
        client,
        &context.url,
        submit,
        &object_id,
        "שמירה",
        execute,
        context.fields.clone(),
        SubmitPage::Calendar,
    )
    .await;

    match submit_result {
        Ok(preview) => Ok(compose_submit_preview_steps(
            preview,
            &steps,
            "apply the requested attendance via the calendar page",
        )),
        Err(err)
            if execute
                && !deleted_conflict
                && is_conflicting_report_error(&err)
                && submit.attendance_type_code.is_some() =>
        {
            let desired_type_code = submit
                .attendance_type_code
                .as_deref()
                .expect("guard ensures desired attendance type exists");
            let refreshed_context = load_calendar_submit_context(client, submit.date)
                .await
                .with_context(|| {
                    format!(
                        "reload calendar submit context after conflict for {}",
                        submit.date.format("%Y-%m-%d")
                    )
                })?;
            let (delete_previews, refreshed_context) = delete_conflicting_absence_reports(
                client,
                refreshed_context,
                desired_type_code,
                execute,
            )
            .await?;
            if !delete_previews.is_empty() {
                steps.extend(delete_previews.into_iter().map(|preview| {
                    (
                        "delete the conflicting calendar row after Hilan rejected the first submit",
                        preview,
                    )
                }));
                let object_id = calendar_submit_object_id(&refreshed_context).to_string();
                let preview = replay_submit_with_fields(
                    client,
                    &refreshed_context.url,
                    submit,
                    &object_id,
                    "שמירה",
                    execute,
                    refreshed_context.fields,
                    SubmitPage::Calendar,
                )
                .await?;
                Ok(compose_submit_preview_steps(
                    preview,
                    &steps,
                    "apply the requested attendance via the calendar page",
                ))
            } else {
                Err(err)
            }
        }
        Err(err) => Err(err),
    }
}

fn calendar_submit_object_id(context: &CalendarSubmitContext) -> &str {
    // Hilan submits against the first populated row ObjectId in the selected-day grid.
    context
        .reports
        .iter()
        .find(|report| report.object_id != EMPTY_OBJECT_ID)
        .map(|report| report.object_id.as_str())
        .unwrap_or(EMPTY_OBJECT_ID)
}

pub(crate) async fn load_calendar_submit_context(
    client: &mut HilanClient,
    date: NaiveDate,
) -> Result<CalendarSubmitContext> {
    let url = calendar_page_url(client);
    let (html, fields) = load_calendar_submit_form(client, &url, date).await?;
    let employee_id = match fields.get("ctl00$mp$Strip$hCurrentItemId") {
        Some(id) if !id.is_empty() => id.clone(),
        _ => {
            crate::api::bootstrap(client)
                .await
                .context("bootstrap employee info for calendar submit context")?
                .user_id
        }
    };
    let reports = parse_calendar_existing_reports(&html, &employee_id, date)?;

    Ok(CalendarSubmitContext {
        url,
        date,
        employee_id,
        fields,
        reports,
    })
}

fn conflicting_absence_reports(
    context: &CalendarSubmitContext,
    desired_type_code: &str,
) -> Vec<CalendarExistingReport> {
    context
        .reports
        .iter()
        .filter(|report| {
            report.is_absence && report.symbol_code.as_deref() != Some(desired_type_code)
        })
        .cloned()
        .collect()
}

pub(crate) fn has_matching_report(
    context: &CalendarSubmitContext,
    desired_type_code: &str,
) -> bool {
    context
        .reports
        .iter()
        .any(|report| report.symbol_code.as_deref() == Some(desired_type_code))
}

pub(crate) async fn delete_calendar_report(
    client: &mut HilanClient,
    context: &CalendarSubmitContext,
    report: &CalendarExistingReport,
    execute: bool,
) -> Result<(SubmitPreview, CalendarSubmitContext)> {
    let grid_event_target = format!(
        "{}$reportsGrid",
        day_field_prefix(&context.employee_id, context.date)
    );
    let grid_dom_id = calendar_grid_dom_id(&context.employee_id, context.date);
    let action_key = format!("{}_action", grid_dom_id.replace('_', "$"));
    let mut replay_fields = context.fields.clone();
    retain_browser_fields(&mut replay_fields, CALENDAR_BROWSER_FIELDS);

    let sm_value = format!("{grid_event_target}_updatePanel|{grid_event_target}");
    let event_argument = serde_json::json!({
        "ObjectId": report.object_id,
        "RowName": report.row_name,
        "EmployeeId": report.employee_id,
        "ReportDate": report.report_date_iso_utc,
    })
    .to_string();

    let overrides = [
        (action_key.clone(), "DELETE_ROW".to_string()),
        ("ctl00$ms".to_string(), sm_value.clone()),
        ("__EVENTTARGET".to_string(), grid_event_target.clone()),
        ("__EVENTARGUMENT".to_string(), event_argument.clone()),
        ("__ASYNCPOST".to_string(), "true".to_string()),
        (
            "H-XSRF-Token".to_string(),
            replay_fields
                .get("H-XSRF-Token")
                .cloned()
                .unwrap_or_default(),
        ),
    ];
    let override_refs: Vec<(&str, &str)> = overrides
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    let payload_display = format_form_fields_for_display(&replay_fields, &override_refs);

    let preview = SubmitPreview {
        url: context.url.clone(),
        button_name: "__EVENTTARGET".to_string(),
        button_value: grid_event_target.clone(),
        employee_id: context.employee_id.clone(),
        payload_display,
        executed: execute,
    };

    if !execute {
        return Ok((preview, context.clone()));
    }

    let response = client
        .post_aspx_async_event_write(
            &context.url,
            &replay_fields,
            &override_refs,
            "ctl00$ms",
            &sm_value,
            &grid_event_target,
            &event_argument,
            false,
        )
        .await
        .with_context(|| format!("delete calendar attendance row for {}", context.date))?;
    dump_debug_html(
        &format!("delete-response-{}", context.date.format("%Y-%m-%d")),
        &response,
    );
    if let Some(message) = extract_submit_error_message(&response) {
        bail!("Hilan rejected attendance delete: {message}");
    }

    let fields = if response.contains("<html") || response.contains("<!DOCTYPE") {
        crate::client::parse_aspx_form_fields(&response)
    } else {
        let entries = crate::client::parse_aspx_delta(&response);
        merge_delta_fields(&replay_fields, &entries)
    };
    let reports = parse_calendar_existing_reports(&response, &context.employee_id, context.date)
        .unwrap_or_default();
    Ok((
        preview,
        CalendarSubmitContext {
            url: context.url.clone(),
            date: context.date,
            employee_id: context.employee_id.clone(),
            fields,
            reports,
        },
    ))
}

pub(crate) async fn delete_conflicting_absence_reports(
    client: &mut HilanClient,
    mut context: CalendarSubmitContext,
    desired_type_code: &str,
    execute: bool,
) -> Result<(Vec<SubmitPreview>, CalendarSubmitContext)> {
    let mut previews = Vec::new();

    loop {
        let Some(report) = conflicting_absence_reports(&context, desired_type_code)
            .into_iter()
            .next()
        else {
            return Ok((previews, context));
        };

        let deleted_object_id = report.object_id.clone();
        let previous_context = context.clone();
        let (preview, refreshed_context) =
            delete_calendar_report(client, &context, &report, execute).await?;
        previews.push(preview);
        context =
            merge_deleted_report_context(previous_context, refreshed_context, &deleted_object_id);
    }
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

    client.ensure_authenticated().await?;
    let (_html, fields) = client
        .get_aspx_form(&url)
        .await
        .with_context(|| format!("load form for {}", submit.date.format("%Y-%m-%d")))?;

    replay_submit_with_fields(
        client,
        &url,
        submit,
        report_id,
        "שמור וסגור",
        execute,
        fields,
        SubmitPage::ErrorWizard,
    )
    .await
}

async fn load_calendar_submit_form(
    client: &mut HilanClient,
    url: &str,
    date: NaiveDate,
) -> Result<(String, BTreeMap<String, String>)> {
    client.ensure_authenticated().await?;
    let (html, fields) = client
        .get_aspx_form(url)
        .await
        .with_context(|| format!("load calendar form for {}", date.format("%Y-%m-%d")))?;
    let (_html, fields) = load_month_page(client, url, html, fields, date).await?;
    let selected_day = calendar_selected_day_value(date);
    // RefreshSelectedDays expects the browser's leading-comma encoding; save posts the bare day.
    let refresh_selected_days = format!(",{selected_day}");
    let month_value = month_field_value(date);
    let response = client
        .post_aspx_async_event_write(
            url,
            &fields,
            &[
                ("__calendarSelectedDays", refresh_selected_days.as_str()),
                ("ctl00$mp$currentMonth", month_value.as_str()),
                ("ctl00$mp$RefreshSelectedDays", "ימים נבחרים"),
            ],
            "ctl00$ms",
            "ctl00$mp$upBtns|ctl00$mp$RefreshSelectedDays",
            "",
            "",
            true,
        )
        .await
        .with_context(|| format!("select calendar day {}", date.format("%Y-%m-%d")))?;
    dump_debug_html(
        &format!("select-day-{}", date.format("%Y-%m-%d")),
        &response,
    );

    let mut fields = if response.contains("<html") || response.contains("<!DOCTYPE") {
        crate::client::parse_aspx_form_fields(&response)
    } else {
        let entries = crate::client::parse_aspx_delta(&response);
        merge_delta_fields(&fields, &entries)
    };

    let selected_row_date = selected_row_date(&response).ok_or_else(|| {
        anyhow!(
            "calendar selection response for {} did not expose a selected row date",
            date.format("%Y-%m-%d")
        )
    })?;
    if selected_row_date != date {
        bail!(
            "calendar selection stayed on {} instead of {}",
            selected_row_date.format("%Y-%m-%d"),
            date.format("%Y-%m-%d")
        );
    }
    // Delta refreshes omit these browser-maintained fields, but the final save depends on them.
    fields.insert("__calendarSelectedDays".to_string(), selected_day);
    fields.insert("ctl00$mp$currentMonth".to_string(), month_value);

    Ok((response, fields))
}

async fn replay_submit_with_fields(
    client: &mut HilanClient,
    url: &str,
    submit: &AttendanceSubmit,
    object_id: &str,
    button_value: &str,
    execute: bool,
    base_fields: BTreeMap<String, String>,
    page: SubmitPage,
) -> Result<SubmitPreview> {
    // `hCurrentItemId` on the form is the long UserId (e.g., 460627).
    // The DirtyFields JSON needs the short EmployeeId (e.g., 27) from the bootstrap API.
    let bootstrap = crate::api::bootstrap(client)
        .await
        .context("bootstrap employee info for form replay")?;
    let employee_id = match base_fields.get("ctl00$mp$Strip$hCurrentItemId") {
        Some(id) if !id.is_empty() => id.clone(),
        _ => bootstrap.user_id.clone(),
    };
    let short_employee_id = bootstrap.employee_id;

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

    let needs_completion_dirty = submit.entry_time.is_none() && submit.exit_time.is_none();
    // Calendar writes still need CompletionToStandard in DirtyFields for browser fidelity,
    // but the checkbox form field itself must stay off there or Hilan narrows the type list.
    let needs_completion_checkbox = page == SubmitPage::ErrorWizard && needs_completion_dirty;

    // The browser only posts ~25 specific fields on async save. Our scraper finds
    // additional hidden fields (employee strip, calendar state) that aren't submitted
    // by the browser. Drop those to match browser fidelity.
    let allowlist = browser_field_allowlist(page);
    replay_fields.retain(|key, _| allowlist.contains(&key.as_str()) || key.starts_with(&prefix));
    if page == SubmitPage::Calendar {
        replay_fields.insert("ctl00$mp$Strip$ACESearch_Value".to_string(), String::new());
    }

    let mut overrides: Vec<(String, String)> = Vec::new();

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

    if needs_completion_checkbox {
        overrides.push((completion_key.clone(), "on".to_string()));
    }

    let script_box_key = match page {
        SubmitPage::Calendar => "ctl00$mp$scriptBox",
        SubmitPage::ErrorWizard => "ctl00$mp$ScriptBox",
    };
    overrides.push((
        script_box_key.to_string(),
        replay_fields
            .get(script_box_key)
            .cloned()
            .unwrap_or_default(),
    ));
    overrides.push((
        "H-XSRF-Token".to_string(),
        replay_fields
            .get("H-XSRF-Token")
            .cloned()
            .unwrap_or_default(),
    ));

    // Build the DirtyFields JSON that Hilan requires to confirm which fields changed.
    // Without this, the server silently ignores all form field updates.
    let include_completion_dirty = page == SubmitPage::Calendar || needs_completion_dirty;
    let dirty_fields_json = build_dirty_fields_json(
        submit,
        short_employee_id,
        object_id,
        include_completion_dirty,
    );
    let reports_grid_key = format!(
        "ctl00_mp_RG_Days_{}_{:04}_{:02}_reportsGrid_data",
        employee_id,
        submit.date.year(),
        submit.date.month()
    );
    overrides.push((reports_grid_key.clone(), dirty_fields_json));
    overrides.push((
        "ReportPageMode".to_string(),
        match page {
            SubmitPage::Calendar => "2",
            SubmitPage::ErrorWizard => "Days",
        }
        .to_string(),
    ));
    overrides.push((
        "hiddenInputToUpdateATBuffer_CommonToolkitScripts".to_string(),
        "1".to_string(),
    ));
    if page == SubmitPage::Calendar {
        overrides.push(("__NextBtnState".to_string(), "false".to_string()));
        overrides.push(("__PrevBtnState".to_string(), "false".to_string()));
    }
    // The button name/value pair (browser sends this even on async postback)
    overrides.push((button_name.clone(), button_value.to_string()));

    let override_refs: Vec<(&str, &str)> = overrides
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();

    let payload_display = format_form_fields_for_display(&replay_fields, &override_refs);

    if execute {
        let response = client
            .post_aspx_async_write(
                url,
                &replay_fields,
                &override_refs,
                "ctl00$ms",
                &button_name,
                false, // state-changing write — must NOT be retried
            )
            .await
            .with_context(|| format!("submit attendance form for {}", submit.date))?;
        dump_debug_html(
            &format!("submit-response-{}", submit.date.format("%Y-%m-%d")),
            &response,
        );
        if let Some(message) = extract_submit_error_message(&response) {
            bail!("Hilan rejected attendance submit: {message}");
        }
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

/// Build the `DirtyFields` JSON that Hilan requires to confirm field changes.
/// Without this, the server silently discards the POST.
fn build_dirty_fields_json(
    submit: &AttendanceSubmit,
    short_employee_id: u32,
    object_id: &str,
    completion_to_standard: bool,
) -> String {
    // The browser marks the full editable row dirty even when the values are blank.
    let mut dirty = vec![
        "\"ManualEntry\":true",
        "\"ManualExit\":true",
        "\"Comment\":true",
        "\"Symbol.SymbolId\":true",
    ];
    if completion_to_standard {
        dirty.push("\"CompletionToStandard\":true");
    }
    format!(
        "[{{\"DirtyFields\":{{{}}},\"EmployeeId\":{},\"IsRangeObject\":false,\"ReportDate\":\"{}-{}-{}\",\"RequestRowName\":\"row_0_0\",\"ReportTypeName\":\"RG\",\"ObjectId\":\"{}\"}}]",
        dirty.join(","),
        short_employee_id,
        submit.date.year(),
        submit.date.month(),
        submit.date.day(),
        object_id,
    )
}

fn compose_submit_preview_steps(
    mut final_preview: SubmitPreview,
    earlier_steps: &[(&'static str, SubmitPreview)],
    final_label: &'static str,
) -> SubmitPreview {
    if earlier_steps.is_empty() {
        return final_preview;
    }

    let mut steps: Vec<(&str, &SubmitPreview)> = earlier_steps
        .iter()
        .map(|(label, preview)| (*label, preview))
        .collect();
    steps.push((final_label, &final_preview));
    let rendered = render_step_list(&steps);
    final_preview.payload_display = rendered;
    final_preview
}

pub(crate) fn render_submit_preview_step(
    step_number: usize,
    label: &str,
    preview: &SubmitPreview,
) -> String {
    format!(
        "Step {step_number}: {label}\nTarget URL: {}\nButton: {} = {}\n{}",
        preview.url, preview.button_name, preview.button_value, preview.payload_display
    )
}

/// Render a list of labeled submit steps as a single display string joined by
/// blank lines. Returned verbatim by both the attendance composer and the
/// provider preview helper so the two render identical output.
pub(crate) fn render_step_list(steps: &[(&str, &SubmitPreview)]) -> String {
    steps
        .iter()
        .enumerate()
        .map(|(index, (label, preview))| render_submit_preview_step(index + 1, label, preview))
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn is_conflicting_report_error(err: &anyhow::Error) -> bool {
    err.to_string()
        .contains(CONFLICTING_REPORT_MESSAGE_FRAGMENT)
}

fn calendar_page_url(client: &HilanClient) -> String {
    format!(
        "{}/Hilannetv2/Attendance/calendarpage.aspx?isOnSelf=true",
        client.base_url
    )
}

fn calendar_grid_dom_id(employee_id: &str, date: NaiveDate) -> String {
    format!(
        "ctl00_mp_RG_Days_{}_{:04}_{:02}_reportsGrid",
        employee_id,
        date.year(),
        date.month()
    )
}

fn parse_calendar_existing_reports(
    html: &str,
    employee_id: &str,
    date: NaiveDate,
) -> Result<Vec<CalendarExistingReport>> {
    let dom_rows = parse_calendar_report_rows_from_dom(html, employee_id, date)?;
    if dom_rows.is_empty() {
        return Ok(Vec::new());
    }

    let data_rows = parse_row_data_entries(html, date)?;
    let mut used = vec![false; data_rows.len()];
    let mut reports = Vec::with_capacity(dom_rows.len());

    for dom_row in dom_rows {
        let match_idx = data_rows
            .iter()
            .enumerate()
            .find(|(idx, row)| {
                !used[*idx]
                    && dom_row.symbol_code.as_deref()
                        == row.symbol.as_ref().and_then(|s| s.first.as_deref())
            })
            .map(|(idx, _)| idx)
            .or_else(|| used.iter().position(|is_used| !is_used));

        let Some(idx) = match_idx else {
            continue;
        };
        used[idx] = true;
        let row = &data_rows[idx];
        let report_date_iso_utc = row_data_report_date_to_iso_utc(row)
            .ok_or_else(|| anyhow!("parse row-data report date for {}", date.format("%Y-%m-%d")))?;
        reports.push(CalendarExistingReport {
            object_id: row.id.clone(),
            employee_id: row.employee_id,
            row_name: dom_row.row_name,
            symbol_code: dom_row
                .symbol_code
                .or_else(|| row.symbol.as_ref().and_then(|symbol| symbol.first.clone())),
            symbol_name: dom_row
                .symbol_name
                .or_else(|| row.symbol.as_ref().and_then(|symbol| symbol.second.clone())),
            is_absence: dom_row.is_absence,
            report_date_iso_utc,
        });
    }

    Ok(reports)
}

fn parse_calendar_report_rows_from_dom(
    html: &str,
    employee_id: &str,
    date: NaiveDate,
) -> Result<Vec<ParsedCalendarReportRow>> {
    let document = Html::parse_document(html);
    let row_selector = Selector::parse(r#"tr[id*="_EmployeeReports_row_"]"#)
        .map_err(|e| anyhow!("selector parse error: {e}"))?;
    let select_selector =
        Selector::parse("select").map_err(|e| anyhow!("selector parse error: {e}"))?;
    let option_selector =
        Selector::parse("option").map_err(|e| anyhow!("selector parse error: {e}"))?;

    let mut rows = Vec::new();
    for row in document.select(&row_selector) {
        let Some(row_id) = row.value().attr("id") else {
            continue;
        };
        let Some(suffix) = row_id.split("_EmployeeReports_row_").nth(1) else {
            continue;
        };
        let Some(select) = row.select(&select_selector).next() else {
            continue;
        };
        let selected = select
            .select(&option_selector)
            .find(|option| option.value().attr("selected").is_some())
            .or_else(|| {
                select
                    .select(&option_selector)
                    .find(|option| option.value().attr("value").is_some_and(|v| !v.is_empty()))
            });
        let symbol_code = selected
            .as_ref()
            .and_then(|option| option.value().attr("value"))
            .filter(|value| !value.is_empty())
            .map(ToString::to_string);
        let symbol_name = selected
            .as_ref()
            .map(|option| option.text().collect::<String>().trim().to_string());
        let is_absence = selected.as_ref().and_then(|option| {
            option
                .value()
                .attr("isAbsenceSymbol")
                .or_else(|| option.value().attr("isabsencesymbol"))
        }) == Some("true");

        rows.push(ParsedCalendarReportRow {
            row_name: delete_row_name(employee_id, date, suffix),
            symbol_code,
            symbol_name,
            is_absence,
        });
    }

    Ok(rows)
}

fn parse_row_data_entries(html: &str, date: NaiveDate) -> Result<Vec<RowDataEntry>> {
    let row_date = format!("{}-{}-{}", date.year(), date.month(), date.day());
    let raw = extract_row_data_json_for_date(html, &row_date)
        .ok_or_else(|| anyhow!("missing RowData JSON for {}", date.format("%Y-%m-%d")))?;
    let decoded: String = serde_json::from_str(&format!("\"{raw}\""))
        .with_context(|| format!("decode RowData JSON string for {}", date.format("%Y-%m-%d")))?;
    let rows: Vec<RowDataEntry> = serde_json::from_str(&decoded)
        .with_context(|| format!("parse RowData array for {}", date.format("%Y-%m-%d")))?;
    let mut unique = Vec::new();
    for row in rows {
        if row.is_report_deleted {
            continue;
        }
        if unique
            .iter()
            .any(|existing: &RowDataEntry| existing.id == row.id)
        {
            continue;
        }
        unique.push(row);
    }
    Ok(unique)
}

fn extract_row_data_json_for_date<'a>(html: &'a str, row_date: &str) -> Option<&'a str> {
    let needle = "RowData\":\"";
    let row_date_needle = format!("\",\"RowDate\":\"{row_date}\"");
    let row_date_start = html.find(&row_date_needle)?;
    let start = html[..row_date_start].rfind(needle)?;
    let content_start = start + needle.len();
    Some(&html[content_start..row_date_start])
}

fn row_data_report_date_to_iso_utc(row: &RowDataEntry) -> Option<String> {
    let raw = if row.is_range {
        row.from_date.as_deref().unwrap_or(&row.report_date)
    } else {
        &row.report_date
    };
    aspx_json_date_to_iso_utc(raw)
}

fn aspx_json_date_to_iso_utc(raw: &str) -> Option<String> {
    let millis = raw
        .strip_prefix("/Date(")?
        .strip_suffix(")/")?
        .parse::<i64>()
        .ok()?;
    let dt: DateTime<Utc> = DateTime::from_timestamp_millis(millis)?;
    Some(dt.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string())
}

fn delete_row_name(employee_id: &str, date: NaiveDate, suffix: &str) -> String {
    format!(
        "{}$cellOf_SysColumn_Delete_EmployeeReports_row_{}$sysCol_EmployeeReports_row_{}",
        day_field_prefix(employee_id, date),
        suffix,
        suffix
    )
}

fn merge_deleted_report_context(
    previous: CalendarSubmitContext,
    mut refreshed: CalendarSubmitContext,
    deleted_object_id: &str,
) -> CalendarSubmitContext {
    if refreshed.reports.is_empty() {
        refreshed.reports = previous.reports;
    }
    refreshed
        .reports
        .retain(|existing| existing.object_id != deleted_object_id);
    refreshed
}

fn merge_delta_fields(
    base_fields: &BTreeMap<String, String>,
    entries: &BTreeMap<(String, String), String>,
) -> BTreeMap<String, String> {
    let mut fields = base_fields.clone();

    for ((entry_type, entry_id), content) in entries {
        if entry_type == "hiddenField" {
            fields.insert(entry_id.clone(), content.clone());
        }
    }

    for ((entry_type, _), content) in entries {
        if entry_type != "updatePanel" {
            continue;
        }
        for (name, value) in parse_aspx_fragment_fields(content) {
            fields.insert(name, value);
        }
    }

    fields
}

fn parse_aspx_fragment_fields(fragment: &str) -> BTreeMap<String, String> {
    let wrapped = format!(r#"<html><body><form id="aspnetForm">{fragment}</form></body></html>"#);
    crate::client::parse_aspx_form_fields(&wrapped)
}

fn extract_submit_error_message(response: &str) -> Option<String> {
    let entries = crate::client::parse_aspx_delta(response);
    for ((entry_type, _), content) in &entries {
        if entry_type == "scriptStartupBlock" || entry_type == "scriptBlock" {
            if let Some(message) = extract_js_message(content) {
                return Some(message);
            }
        }
    }
    extract_js_message(response)
}

fn extract_js_message(text: &str) -> Option<String> {
    for prefix in ["alert('", "HWarning('", "HError('"] {
        if let Some(start) = text.find(prefix) {
            let rest = &text[start + prefix.len()..];
            if let Some(end) = rest.find("')") {
                return Some(rest[..end].to_string());
            }
        }
    }
    None
}

fn selected_row_date(html: &str) -> Option<NaiveDate> {
    // Hilan serializes RowDate without zero-padding on month/day,
    // e.g. `"RowDate":"2026-4-9"` rather than `"2026-04-09"`. Parse manually.
    // Use escaped quotes (not raw strings — `"#` would terminate r#"..."# early).
    let needle = "\"RowDate\":\"";
    let start = html.find(needle)?;
    let rest = &html[start + needle.len()..];
    let end = rest.find('"')?;
    let raw = &rest[..end];
    let parts: Vec<&str> = raw.split('-').collect();
    if parts.len() != 3 {
        return None;
    }
    let (y, m, d) = (
        parts[0].parse::<i32>().ok()?,
        parts[1].parse::<u32>().ok()?,
        parts[2].parse::<u32>().ok()?,
    );
    NaiveDate::from_ymd_opt(y, m, d)
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

fn displayed_month(fields: &BTreeMap<String, String>) -> Result<NaiveDate> {
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
    Ok(crate::client::shift_month(date, months))
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
        "Attendance type '{name}' needs cached ontology. Run `shaon cache refresh attendance-types` first or pass a numeric code."
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use scraper::Html;

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
        let employee_wrapper_cell = document
            .select(&ROW_0_TOP_CELL_SEL)
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
    fn selected_day_detail_row_enriches_visual_calendar_status() {
        let html = r#"
            <html><body>
                <table><tbody><tr>
                    <td class="cDIES CSD" Days="9612" tabindex="0" aria-label="26" title="9:00">
                        <table class="iDSIE">
                            <tr class="dayImageNumberContainer">
                                <td class="dTS">26</td>
                                <td class="imageContainerStyle"></td>
                            </tr>
                            <tr>
                                <td class="calendarMessageCell" colspan="2"><div class="cDM">9:00</div></td>
                            </tr>
                        </table>
                    </td>
                </tr></tbody></table>
                <table><tbody>
                    <tr id="ctl00_mp_RG_Days_460627_2026_04_row_0">
                        <td id="ctl00_mp_RG_Days_460627_2026_04_cellOf_ReportDate_row_0" ov="26/04 יום&nbsp;א">
                            <span>26/04 יום&nbsp;א</span>
                        </td>
                        <td>
                            <table><tbody>
                                <tr id="ctl00_mp_RG_Days_460627_2026_04_EmployeeReports_row_0_0">
                                    <td>
                                        <input name="ctl00$mp$RG_Days_460627_2026_04$cellOf_ManualEntry_EmployeeReports_row_0_0$ManualEntry_EmployeeReports_row_0_0" value="09:00" />
                                    </td>
                                    <td>
                                        <input name="ctl00$mp$RG_Days_460627_2026_04$cellOf_ManualExit_EmployeeReports_row_0_0$ManualExit_EmployeeReports_row_0_0" value="18:00" />
                                    </td>
                                    <td>
                                        <select name="ctl00$mp$RG_Days_460627_2026_04$cellOf_Symbol.SymbolId_EmployeeReports_row_0_0$Symbol.SymbolId_EmployeeReports_row_0_0">
                                            <option value="0">work day</option>
                                            <option selected="selected" value="120">work from home</option>
                                        </select>
                                    </td>
                                </tr>
                            </tbody></table>
                        </td>
                    </tr>
                </tbody></table>
            </body></html>
        "#;
        let month = NaiveDate::from_ymd_opt(2026, 4, 1).unwrap();

        let days = parse_calendar_html(html, month).expect("parse selected day detail");
        let day = days
            .iter()
            .find(|day| day.date == NaiveDate::from_ymd_opt(2026, 4, 26).unwrap())
            .expect("day 2026-04-26");

        assert_eq!(day.entry_time.as_deref(), Some("09:00"));
        assert_eq!(day.exit_time.as_deref(), Some("18:00"));
        assert_eq!(day.attendance_type.as_deref(), Some("work from home"));
        assert_eq!(day.source, hr_core::AttendanceSource::UserReported);
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

    #[test]
    fn dirty_fields_json_matches_browser_shape_for_completion_submit() {
        let submit = AttendanceSubmit {
            date: NaiveDate::from_ymd_opt(2026, 4, 9).unwrap(),
            attendance_type_code: Some("120".to_string()),
            entry_time: None,
            exit_time: None,
            comment: None,
            clear_entry: false,
            clear_exit: false,
            clear_comment: false,
            default_work_day: false,
        };

        let json = build_dirty_fields_json(&submit, 27, EMPTY_OBJECT_ID, true);

        assert!(json.contains("\"ManualEntry\":true"));
        assert!(json.contains("\"ManualExit\":true"));
        assert!(json.contains("\"Comment\":true"));
        assert!(json.contains("\"Symbol.SymbolId\":true"));
        assert!(json.contains("\"CompletionToStandard\":true"));
        assert!(json.contains("\"EmployeeId\":27"));
        assert!(json.contains("\"ReportDate\":\"2026-4-9\""));
        assert!(json.contains(&format!("\"ObjectId\":\"{}\"", EMPTY_OBJECT_ID)));
    }

    #[test]
    fn dirty_fields_json_uses_passed_object_id() {
        let submit = AttendanceSubmit {
            date: NaiveDate::from_ymd_opt(2026, 4, 9).unwrap(),
            attendance_type_code: Some("120".to_string()),
            entry_time: Some("09:00".to_string()),
            exit_time: Some("18:00".to_string()),
            comment: Some("office".to_string()),
            clear_entry: false,
            clear_exit: false,
            clear_comment: false,
            default_work_day: false,
        };

        let json = build_dirty_fields_json(&submit, 27, "report-1", false);

        assert!(json.contains("\"ObjectId\":\"report-1\""));
        assert!(!json.contains("\"CompletionToStandard\":true"));
    }

    #[test]
    fn merge_delta_fields_applies_hidden_and_panel_updates() {
        let base = BTreeMap::from([
            ("__VIEWSTATE".to_string(), "old-state".to_string()),
            ("__calendarSelectedDays".to_string(), "9596".to_string()),
        ]);
        let entries = BTreeMap::from([
            (
                ("hiddenField".to_string(), "__VIEWSTATE".to_string()),
                "new-state".to_string(),
            ),
            (
                (
                    "updatePanel".to_string(),
                    "ctl00_mp_calendarUpdator".to_string(),
                ),
                r#"<input type="hidden" name="__calendarSelectedDays" value="9598,9595" />
                   <input type="text" name="ctl00$mp$scriptBox" value="" />"#
                    .to_string(),
            ),
        ]);

        let merged = merge_delta_fields(&base, &entries);

        assert_eq!(
            merged.get("__VIEWSTATE").map(String::as_str),
            Some("new-state")
        );
        assert_eq!(
            merged.get("__calendarSelectedDays").map(String::as_str),
            Some("9598,9595")
        );
        assert_eq!(
            merged.get("ctl00$mp$scriptBox").map(String::as_str),
            Some("")
        );
    }

    #[test]
    fn extract_submit_error_message_reads_alert_from_delta() {
        let delta =
            "40|scriptStartupBlock|ScriptContentNoTags|alert('09/04 - קיים דיווח בזמן המדווח');|";

        assert_eq!(
            extract_submit_error_message(delta).as_deref(),
            Some("09/04 - קיים דיווח בזמן המדווח")
        );
    }

    #[test]
    fn extract_submit_error_message_reads_hwarning_from_delta() {
        let delta =
            "35|scriptStartupBlock|ScriptContentNoTags|HWarning('יש לבחור סוג דיווח אחר');|";

        assert_eq!(
            extract_submit_error_message(delta).as_deref(),
            Some("יש לבחור סוג דיווח אחר")
        );
    }

    #[test]
    fn parse_calendar_existing_reports_extracts_delete_target_metadata() {
        let html = r#"
            <html><body>
                <table>
                    <tr id="ctl00_mp_RG_Days_460627_2026_04_EmployeeReports_row_0_1">
                        <td>
                            <select>
                                <option value="">בחר</option>
                                <option selected="selected" value="481" isAbsenceSymbol="true">vacation</option>
                            </select>
                        </td>
                    </tr>
                </table>
                <script>
                    $create(Hilan.HilanNet.Web.Controls.HAttendanceGrid.HReportsGridRow.HReportsGridRowBehavior, {"RowData":"[{\"ID\":\"628cce48-84b5-4a3d-b507-c40169cdfefe\",\"IsRange\":false,\"EmployeeId\":27,\"ReportDate\":\"\\/Date(1775682000000)\\/\",\"IsReportDeleted\":false,\"Symbol\":{\"First\":\"481\",\"Second\":\"vacation\"}}]","RowDate":"2026-4-9"});
                </script>
            </body></html>
        "#;

        let reports = parse_calendar_existing_reports(
            html,
            "460627",
            NaiveDate::from_ymd_opt(2026, 4, 9).unwrap(),
        )
        .unwrap();

        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].object_id, "628cce48-84b5-4a3d-b507-c40169cdfefe");
        assert_eq!(reports[0].employee_id, 27);
        assert_eq!(reports[0].symbol_code.as_deref(), Some("481"));
        assert_eq!(reports[0].symbol_name.as_deref(), Some("vacation"));
        assert!(reports[0].is_absence);
        assert_eq!(
            reports[0].row_name,
            "ctl00$mp$RG_Days_460627_2026_04$cellOf_SysColumn_Delete_EmployeeReports_row_0_1$sysCol_EmployeeReports_row_0_1"
        );
        assert_eq!(reports[0].report_date_iso_utc, "2026-04-08T21:00:00.000Z");
    }

    #[test]
    fn conflicting_absence_reports_collects_all_non_matching_absence_rows() {
        let date = NaiveDate::from_ymd_opt(2026, 4, 9).unwrap();
        let context = CalendarSubmitContext {
            url: "https://example.test/calendar".to_string(),
            date,
            employee_id: "460627".to_string(),
            fields: BTreeMap::new(),
            reports: vec![
                CalendarExistingReport {
                    object_id: "keep-match".to_string(),
                    employee_id: 27,
                    row_name: "row-0".to_string(),
                    symbol_code: Some("120".to_string()),
                    symbol_name: Some("work from home".to_string()),
                    is_absence: true,
                    report_date_iso_utc: "2026-04-08T21:00:00.000Z".to_string(),
                },
                CalendarExistingReport {
                    object_id: "delete-vacation".to_string(),
                    employee_id: 27,
                    row_name: "row-1".to_string(),
                    symbol_code: Some("481".to_string()),
                    symbol_name: Some("vacation".to_string()),
                    is_absence: true,
                    report_date_iso_utc: "2026-04-08T21:00:00.000Z".to_string(),
                },
                CalendarExistingReport {
                    object_id: "keep-non-absence".to_string(),
                    employee_id: 27,
                    row_name: "row-2".to_string(),
                    symbol_code: Some("0".to_string()),
                    symbol_name: Some("work day".to_string()),
                    is_absence: false,
                    report_date_iso_utc: "2026-04-08T21:00:00.000Z".to_string(),
                },
                CalendarExistingReport {
                    object_id: "delete-sick".to_string(),
                    employee_id: 27,
                    row_name: "row-3".to_string(),
                    symbol_code: Some("999".to_string()),
                    symbol_name: Some("sick".to_string()),
                    is_absence: true,
                    report_date_iso_utc: "2026-04-08T21:00:00.000Z".to_string(),
                },
            ],
        };

        let reports = conflicting_absence_reports(&context, "120");
        let object_ids = reports
            .iter()
            .map(|report| report.object_id.as_str())
            .collect::<Vec<_>>();

        assert_eq!(object_ids, vec!["delete-vacation", "delete-sick"]);
    }

    #[test]
    fn parse_row_data_entries_uses_row_data_for_requested_date() {
        let html = r#"
            <script>
                $create(Hilan.HilanNet.Web.Controls.HAttendanceGrid.HReportsGridRow.HReportsGridRowBehavior, {"RowData":"[{\"ID\":\"first-row\",\"IsRange\":false,\"EmployeeId\":27,\"ReportDate\":\"\\/Date(1775595600000)\\/\",\"IsReportDeleted\":false,\"Symbol\":{\"First\":\"481\",\"Second\":\"vacation\"}}]","RowDate":"2026-4-8"});
                $create(Hilan.HilanNet.Web.Controls.HAttendanceGrid.HReportsGridRow.HReportsGridRowBehavior, {"RowData":"[{\"ID\":\"second-row\",\"IsRange\":false,\"EmployeeId\":27,\"ReportDate\":\"\\/Date(1775682000000)\\/\",\"IsReportDeleted\":false,\"Symbol\":{\"First\":\"120\",\"Second\":\"work from home\"}}]","RowDate":"2026-4-9"});
            </script>
        "#;

        let rows = parse_row_data_entries(html, NaiveDate::from_ymd_opt(2026, 4, 9).unwrap())
            .expect("parse requested row data");

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "second-row");
        assert_eq!(
            rows[0]
                .symbol
                .as_ref()
                .and_then(|symbol| symbol.first.as_deref()),
            Some("120")
        );
    }

    #[test]
    fn delete_action_key_matches_browser_shape() {
        let date = NaiveDate::from_ymd_opt(2026, 4, 9).unwrap();
        let action_key = format!(
            "{}_action",
            calendar_grid_dom_id("460627", date).replace('_', "$")
        );

        assert_eq!(
            action_key,
            "ctl00$mp$RG$Days$460627$2026$04$reportsGrid_action"
        );
    }
}
