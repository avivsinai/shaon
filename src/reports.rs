use anyhow::{Context, Result};
use scraper::{Html, Selector};
use serde::Serialize;

use crate::client::HilanClient;

/// Known report names for `repAttendanceviewerGeneric.aspx`.
#[allow(dead_code)]
pub const ERRORS_REPORT: &str = "ErrorsReportNEW";
#[allow(dead_code)]
pub const MISSING_REPORT: &str = "MissingReportNEW";
#[allow(dead_code)]
pub const STATUS_REPORT: &str = "AttendanceStatusReportNew2";
#[allow(dead_code)]
pub const ABSENCE_REPORT: &str = "AbsenceReportNEW";
#[allow(dead_code)]
pub const ALL_REPORT: &str = "AllReportNEW";
#[allow(dead_code)]
pub const CORRECTIONS_REPORT: &str = "ManualReportingReportNEW";
#[allow(dead_code)]
pub const SHEET_URL_PATH: &str = "/Hilannetv2/Attendance/HoursAnalysis.aspx";
#[allow(dead_code)]
pub const CORRECTIONS_URL_PATH: &str = "/Hilannetv2/Attendance/HoursReportLog.aspx";

/// A parsed HTML table from a Hilan report page.
#[derive(Debug, Serialize)]
pub struct ReportTable {
    pub headers: Vec<String>,
    pub rows: Vec<Vec<String>>,
}

/// Fetch a named report and parse its HTML table.
pub async fn fetch_report(client: &mut HilanClient, report_name: &str) -> Result<ReportTable> {
    let url = format!(
        "{}/Hilannetv2/Reports/repAttendanceviewerGeneric.aspx?reportName={}",
        client.base_url, report_name
    );

    fetch_table_from_url(client, &url).await
}

/// Fetch a direct HTML table page by absolute or base-relative URL.
pub async fn fetch_table_from_url(client: &mut HilanClient, url: &str) -> Result<ReportTable> {
    let url = if url.starts_with("http://") || url.starts_with("https://") {
        url.to_string()
    } else {
        format!("{}{}", client.base_url, url)
    };

    let html = client
        .get_text(&url)
        .await
        .with_context(|| format!("fetch report from {url}"))?;

    parse_report_html(&html).with_context(|| format!("parse HTML table from {url}"))
}

/// Print a report table to the terminal.
pub fn print_report(table: &ReportTable) {
    if table.headers.is_empty() && table.rows.is_empty() {
        println!("(empty report — no data rows found)");
        return;
    }

    // Compute column count from headers or the widest row.
    let col_count = table
        .headers
        .len()
        .max(table.rows.iter().map(|r| r.len()).max().unwrap_or(0));

    if col_count == 0 {
        println!("(empty report — no columns found)");
        return;
    }

    // Compute max width per column (account for both headers and data).
    let mut widths = vec![0usize; col_count];
    for (i, h) in table.headers.iter().enumerate() {
        widths[i] = widths[i].max(display_width(h));
    }
    for row in &table.rows {
        for (i, cell) in row.iter().enumerate() {
            if i < col_count {
                widths[i] = widths[i].max(display_width(cell));
            }
        }
    }

    // Cap column widths to keep output readable.
    for w in &mut widths {
        *w = (*w).clamp(2, 40);
    }

    // Print header row.
    if !table.headers.is_empty() {
        let header_line: Vec<String> = (0..col_count)
            .map(|i| {
                let h = table.headers.get(i).map(String::as_str).unwrap_or("");
                pad_or_truncate(h, widths[i])
            })
            .collect();
        println!("{}", header_line.join("  "));

        let sep_line: Vec<String> = widths.iter().map(|&w| "-".repeat(w)).collect();
        println!("{}", sep_line.join("  "));
    }

    // Print data rows.
    for row in &table.rows {
        let line: Vec<String> = (0..col_count)
            .map(|i| {
                let cell = row.get(i).map(String::as_str).unwrap_or("");
                pad_or_truncate(cell, widths[i])
            })
            .collect();
        println!("{}", line.join("  "));
    }

    println!("\n({} rows)", table.rows.len());
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Parse the first meaningful table out of report HTML.
fn parse_report_html(html: &str) -> Result<ReportTable> {
    let document = Html::parse_document(html);

    // Try ASP.NET GridView-style tables first (common class patterns).
    let grid_selectors = [
        "table.GridView",
        "table.grid",
        "table.rgMasterTable",
        "table.DataGrid",
    ];

    for sel_str in &grid_selectors {
        if let Ok(sel) = Selector::parse(sel_str) {
            if let Some(table_el) = document.select(&sel).next() {
                let result = extract_table(table_el)?;
                if !result.headers.is_empty() || !result.rows.is_empty() {
                    return Ok(result);
                }
            }
        }
    }

    // Fallback: find the largest <table> by row count.
    let table_sel =
        Selector::parse("table").map_err(|_| anyhow::anyhow!("failed to build table selector"))?;

    let mut best: Option<ReportTable> = None;
    let mut best_size = 0usize;

    for table_el in document.select(&table_sel) {
        if let Ok(parsed) = extract_table(table_el) {
            let size = parsed.rows.len() + if parsed.headers.is_empty() { 0 } else { 1 };
            if size > best_size {
                best_size = size;
                best = Some(parsed);
            }
        }
    }

    best.ok_or_else(|| anyhow::anyhow!("no table found in report HTML"))
}

/// Extract headers and rows from a single `<table>` element.
fn extract_table(table_el: scraper::ElementRef<'_>) -> Result<ReportTable> {
    let tr_sel =
        Selector::parse("tr").map_err(|_| anyhow::anyhow!("failed to build tr selector"))?;
    let th_sel =
        Selector::parse("th").map_err(|_| anyhow::anyhow!("failed to build th selector"))?;
    let td_sel =
        Selector::parse("td").map_err(|_| anyhow::anyhow!("failed to build td selector"))?;

    let mut headers = Vec::new();
    let mut rows = Vec::new();

    for tr in table_el.select(&tr_sel) {
        let ths: Vec<String> = tr.select(&th_sel).map(|el| cell_text(el)).collect();

        if !ths.is_empty() {
            // Use the last header row encountered (some tables have multi-row headers).
            headers = ths;
            continue;
        }

        let tds: Vec<String> = tr.select(&td_sel).map(|el| cell_text(el)).collect();

        if !tds.is_empty() {
            rows.push(tds);
        }
    }

    Ok(ReportTable { headers, rows })
}

/// Collect visible text from a table cell, trimming whitespace.
fn cell_text(el: scraper::ElementRef<'_>) -> String {
    el.text()
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Approximate display width for a string.
///
/// Characters outside basic ASCII (e.g. Hebrew) are counted as-is since
/// terminal width of RTL text varies by terminal. This is good enough for
/// column alignment.
fn display_width(s: &str) -> usize {
    s.chars().count()
}

/// Pad (or truncate) a string to exactly `width` characters.
fn pad_or_truncate(s: &str, width: usize) -> String {
    let char_count = s.chars().count();
    if char_count <= width {
        format!("{s}{}", " ".repeat(width - char_count))
    } else {
        let truncated: String = s.chars().take(width.saturating_sub(1)).collect();
        format!("{truncated}…")
    }
}
