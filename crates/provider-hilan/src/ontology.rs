use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Utc};
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

use crate::client::HilanClient;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttendanceType {
    pub code: String,
    pub name_he: String,
    pub name_en: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct OrgOntology {
    pub subdomain: String,
    pub types: Vec<AttendanceType>,
    pub fetched_at: DateTime<Utc>,
}

impl OrgOntology {
    /// Load from cache if fresh (< 24h), otherwise sync from server.
    pub async fn load_or_sync(client: &mut HilanClient, subdomain: &str) -> Result<Self> {
        let path = ontology_path(subdomain);
        if path.exists() {
            let ontology = Self::load(&path)?;
            let age = Utc::now() - ontology.fetched_at;
            if age < chrono::Duration::hours(24) {
                return Ok(ontology);
            }
            eprintln!(
                "Ontology cache is {} hours old, refreshing...",
                age.num_hours()
            );
        } else {
            eprintln!("No cached types found, syncing from server...");
        }
        sync_from_calendar(client, subdomain).await
    }

    /// Deserialize from a JSON file.
    pub fn load(path: &Path) -> Result<Self> {
        let content =
            fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
        let ontology: Self =
            serde_json::from_str(&content).with_context(|| format!("parse {}", path.display()))?;
        Ok(ontology)
    }

    /// Serialize to a JSON file, creating parent directories as needed.
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            fs::create_dir_all(parent)
                .with_context(|| format!("create directory {}", parent.display()))?;
        }
        let json = serde_json::to_string_pretty(self).context("serialize ontology")?;
        fs::write(path, json).with_context(|| format!("write {}", path.display()))?;
        Ok(())
    }

    /// Case-insensitive match against code, name_he, or name_en.
    /// Returns an error listing available types if not found.
    pub fn validate_type(&self, name: &str) -> Result<&AttendanceType> {
        let lower = name.to_lowercase();
        self.types
            .iter()
            .find(|t| {
                t.code.to_lowercase() == lower
                    || t.name_he.to_lowercase() == lower
                    || t.name_en
                        .as_deref()
                        .is_some_and(|en| en.to_lowercase() == lower)
            })
            .ok_or_else(|| {
                let mut available = String::from("Available attendance types:\n");
                for t in &self.types {
                    let en = t
                        .name_en
                        .as_deref()
                        .map(|s| format!(" ({s})"))
                        .unwrap_or_default();
                    available.push_str(&format!("  {} — {}{}\n", t.code, t.name_he, en));
                }
                anyhow::anyhow!("Unknown attendance type '{name}'.\n{available}")
            })
    }

    /// Print a formatted table of all attendance types.
    pub fn print_table(&self) {
        let code_w = self
            .types
            .iter()
            .map(|t| t.code.len())
            .max()
            .unwrap_or(4)
            .max(4);
        let he_w = self
            .types
            .iter()
            .map(|t| t.name_he.len())
            .max()
            .unwrap_or(6)
            .max(6);
        let en_w = self
            .types
            .iter()
            .map(|t| t.name_en.as_deref().map_or(0, str::len))
            .max()
            .unwrap_or(7)
            .max(7);

        println!(
            "{:<code_w$}  {:<he_w$}  {:<en_w$}",
            "Code",
            "Hebrew",
            "English",
            code_w = code_w,
            he_w = he_w,
            en_w = en_w,
        );
        println!(
            "{:-<code_w$}  {:-<he_w$}  {:-<en_w$}",
            "",
            "",
            "",
            code_w = code_w,
            he_w = he_w,
            en_w = en_w,
        );
        for t in &self.types {
            println!(
                "{:<code_w$}  {:<he_w$}  {:<en_w$}",
                t.code,
                t.name_he,
                t.name_en.as_deref().unwrap_or(""),
                code_w = code_w,
                he_w = he_w,
                en_w = en_w,
            );
        }
        println!(
            "\nCached from {} at {}",
            self.subdomain,
            self.fetched_at.format("%Y-%m-%d %H:%M UTC")
        );
    }
}

/// Returns the path to the cached ontology file for a given subdomain.
pub fn ontology_path(subdomain: &str) -> std::path::PathBuf {
    crate::config::subdomain_dir(subdomain).join("types.json")
}

/// Sync attendance types from the calendar page dropdown and the absences API.
///
/// 1. GET the calendar page and parse the Symbol.SymbolId `<select>` dropdown
/// 2. Fetch Hebrew names from the absences API
/// 3. Merge into an `OrgOntology` and save to cache
pub async fn sync_from_calendar(client: &mut HilanClient, subdomain: &str) -> Result<OrgOntology> {
    let url = format!(
        "{}/Hilannetv2/Attendance/calendarpage.aspx?isOnSelf=true",
        client.base_url
    );

    let (html, _fields) = client
        .get_aspx_form(&url)
        .await
        .context("sync_from_calendar: GET calendar page")?;

    // Parse the Symbol.SymbolId dropdown from the HTML
    let dropdown_types = parse_symbol_dropdown(&html)?;

    if dropdown_types.is_empty() {
        return Err(anyhow!(
            "No attendance types found in the calendar page dropdown"
        ));
    }

    // Try to get Hebrew names from the absences API
    let hebrew_map = match crate::api::get_absences_initial(client).await {
        Ok(data) => {
            let mut map = HashMap::new();
            for sym in &data.symbols {
                map.insert(sym.id.clone(), sym.name.clone());
            }
            map
        }
        Err(_) => {
            // Absences API may not return all types — proceed with what we have
            HashMap::new()
        }
    };

    // Build the ontology by merging dropdown (English) with absences (Hebrew)
    let types: Vec<AttendanceType> = dropdown_types
        .into_iter()
        .map(|(code, name_en)| {
            let name_he = hebrew_map.get(&code).cloned().unwrap_or_default();
            AttendanceType {
                code,
                name_he,
                name_en: if name_en.is_empty() {
                    None
                } else {
                    Some(name_en)
                },
            }
        })
        .collect();

    let ontology = OrgOntology {
        subdomain: subdomain.to_string(),
        types,
        fetched_at: Utc::now(),
    };

    let path = ontology_path(subdomain);
    ontology
        .save(&path)
        .with_context(|| format!("save ontology to {}", path.display()))?;

    tracing::info!(
        "Synced {} attendance types to {}",
        ontology.types.len(),
        path.display()
    );

    Ok(ontology)
}

/// Parse the `<select>` element whose name contains `Symbol.SymbolId` from the calendar HTML.
/// Returns a list of (code, english_name) tuples.
fn parse_symbol_dropdown(html: &str) -> Result<Vec<(String, String)>> {
    let document = Html::parse_document(html);

    // Find <select> elements — look for the one whose name contains "Symbol.SymbolId"
    let select_sel = Selector::parse("select").map_err(|e| anyhow!("selector parse error: {e}"))?;
    let option_sel = Selector::parse("option").map_err(|e| anyhow!("selector parse error: {e}"))?;

    let mut types = Vec::new();

    for select in document.select(&select_sel) {
        let name = select.value().attr("name").unwrap_or("");
        if !name.contains("Symbol.SymbolId") && !name.contains("SymbolId") {
            continue;
        }

        // Found the dropdown — extract all options
        for option in select.select(&option_sel) {
            let value = option.value().attr("value").unwrap_or("").to_string();
            let text: String = option
                .text()
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .collect::<Vec<_>>()
                .join(" ");

            // Skip empty or placeholder options
            if value.is_empty() && text.is_empty() {
                continue;
            }

            types.push((value, text));
        }

        // We found the relevant dropdown — no need to keep searching
        break;
    }

    Ok(types)
}
