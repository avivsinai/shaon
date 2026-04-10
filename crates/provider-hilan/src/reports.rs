use anyhow::{Context, Result};
use scraper::{Html, Selector};
use serde::Serialize;

use crate::client::HilanClient;

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
        client.base_url,
        urlencoding::encode(report_name)
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

    tracing::debug!("GET (report) {}", url);
    let html = client
        .get_text(&url)
        .await
        .with_context(|| format!("fetch report from {url}"))?;

    parse_report_html(&html).with_context(|| format!("parse HTML table from {url}"))
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
