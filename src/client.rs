use anyhow::{anyhow, bail, Context, Result};
use chrono::{Datelike, Local, NaiveDate};
use regex::Regex;
use reqwest_cookie_store::CookieStoreMutex;
use scraper::{Html, Selector};
use secrecy::ExposeSecret;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};
use std::time::Duration;
use zeroize::Zeroize;

static ORG_ID_RE: LazyLock<Regex> = LazyLock::new(|| {
    // Match OrgId in both plain JSON ("OrgId":"4606") and escaped JSON (\"OrgId\":\"4606\")
    Regex::new(r#"\\?"OrgId\\?"[:\s]*\\?"(\d+)\\?""#).expect("invalid OrgId regex")
});

use crate::config::Config;

pub struct HilanClient {
    pub(crate) client: reqwest::Client,
    pub(crate) base_url: String,
    config: Config,
    org_id: Option<String>,
    logged_in: bool,
    cookie_store: Arc<CookieStoreMutex>,
}

const USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";

#[derive(Debug, Serialize)]
pub struct PayslipDownload {
    pub month: NaiveDate,
    pub path: PathBuf,
    pub size_bytes: usize,
}

#[derive(Debug, Serialize)]
pub struct SalaryEntry {
    pub month: NaiveDate,
    pub amount: u64,
}

#[derive(Debug, Serialize)]
pub struct SalarySummary {
    pub label: String,
    pub entries: Vec<SalaryEntry>,
    pub percent_diff: Option<f64>,
}

#[derive(Deserialize)]
struct LoginResponse {
    #[serde(rename = "IsFail")]
    is_fail: bool,
    #[serde(rename = "IsShowCaptcha")]
    is_show_captcha: Option<bool>,
    #[serde(rename = "Code")]
    code: Option<i32>,
    #[serde(rename = "ErrorMessage")]
    error_message: Option<String>,
}

impl HilanClient {
    pub fn new(config: Config) -> Result<Self> {
        // Load persisted cookies if available
        let cookie_path = crate::config::config_dir()
            .join(&config.subdomain)
            .join("cookies.json");
        let cookie_store = if cookie_path.exists() {
            let file = fs::File::open(&cookie_path)
                .with_context(|| format!("open {}", cookie_path.display()))?;
            let reader = std::io::BufReader::new(file);
            match cookie_store::serde::json::load(reader) {
                Ok(store) => {
                    tracing::debug!("loaded session cookies from {}", cookie_path.display());
                    store
                }
                Err(e) => {
                    tracing::debug!("stale cookie file, starting fresh: {e}");
                    cookie_store::CookieStore::default()
                }
            }
        } else {
            cookie_store::CookieStore::default()
        };
        let cookie_store = Arc::new(CookieStoreMutex::new(cookie_store));

        let client = reqwest::Client::builder()
            .cookie_provider(cookie_store.clone())
            .user_agent(USER_AGENT)
            .connect_timeout(Duration::from_secs(30))
            .timeout(Duration::from_secs(60))
            .use_rustls_tls()
            .build()
            .context("build HTTP client")?;

        // Validate subdomain to prevent URL manipulation via malicious config
        if !config
            .subdomain
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.')
        {
            anyhow::bail!(
                "subdomain '{}' contains invalid characters (only alphanumeric, hyphens, and dots allowed)",
                config.subdomain
            );
        }
        let base_url = format!("https://{}.hilan.co.il", config.subdomain);

        // Check if we have cached session cookies (candidate, not proven auth)
        let has_session_candidate = {
            let store = cookie_store.lock().unwrap();
            // Look for auth-related cookies, not just any cookie
            store.iter_any().count() > 1
        };

        Ok(Self {
            client,
            base_url,
            config,
            org_id: None,
            // Candidate session: will be validated on first request
            logged_in: has_session_candidate,
            cookie_store,
        })
    }

    /// Borrow the config.
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Mutably borrow the config (e.g. for migration).
    pub fn config_mut(&mut self) -> &mut Config {
        &mut self.config
    }

    /// Fetch the OrgId from the Hilanet homepage.
    pub async fn fetch_org_id(&mut self) -> Result<String> {
        tracing::debug!("GET {}", self.base_url);
        let resp = self
            .client
            .get(&self.base_url)
            .send()
            .await
            .context("GET Hilanet homepage")?;

        let text = resp.text().await.context("read homepage body")?;

        let org_id = ORG_ID_RE
            .captures(&text)
            .and_then(|cap| cap.get(1))
            .map(|m| m.as_str().to_string())
            .ok_or_else(|| {
                anyhow!("Could not find OrgId in Hilanet homepage. Is the subdomain correct?")
            })?;

        self.org_id = Some(org_id.clone());
        Ok(org_id)
    }

    /// Log in to Hilan. Fetches OrgId first if not already known.
    /// Subsequent calls are no-ops while the session is alive.
    pub async fn login(&mut self) -> Result<()> {
        if self.logged_in {
            return Ok(());
        }
        if self.org_id.is_none() {
            self.fetch_org_id().await?;
        }
        let org_id = self.org_id.as_ref().unwrap();

        let url = format!(
            "{}/HilanCenter/Public/api/LoginApi/LoginRequest",
            self.base_url
        );

        tracing::debug!("POST {}", url);
        let secret = self.config.get_password()?;
        let mut pw = secret.expose_secret().to_string();

        let form = [
            ("username", self.config.username.as_str()),
            ("password", pw.as_str()),
            ("orgId", org_id.as_str()),
        ];

        let resp = self
            .client
            .post(&url)
            .form(&form)
            .send()
            .await
            .context("POST login request")?;
        pw.zeroize();

        let login: LoginResponse = resp.json().await.context("parse login response")?;

        if login.is_show_captcha == Some(true) {
            bail!(
                "CAPTCHA required. Please log in via browser at {} and solve the captcha, then try again.",
                self.base_url
            );
        }

        if login.is_fail {
            match login.code {
                Some(18) => {
                    bail!("Temporary login error. Please try again in a few minutes.");
                }
                Some(6) => {
                    bail!(
                        "Password change required. Please update your password at {}.",
                        self.base_url
                    );
                }
                _ => {
                    let msg = login
                        .error_message
                        .unwrap_or_else(|| "Unknown login error".to_string());
                    bail!("Login failed: {}", msg);
                }
            }
        }

        self.logged_in = true;
        self.save_cookies();
        let masked_user = if self.config.username.len() > 4 {
            format!(
                "***{}",
                &self.config.username[self.config.username.len() - 4..]
            )
        } else {
            "***".to_string()
        };
        tracing::info!("Logged in as {} (org: {})", masked_user, org_id);
        Ok(())
    }

    /// Persist session cookies to disk for reuse across CLI invocations.
    fn save_cookies(&self) {
        let cookie_dir = crate::config::config_dir().join(&self.config.subdomain);
        if fs::create_dir_all(&cookie_dir).is_err() {
            return;
        }
        let cookie_path = cookie_dir.join("cookies.json");
        let store = self.cookie_store.lock().unwrap();
        if let Ok(mut file) = fs::File::create(&cookie_path) {
            // Include session cookies (no Expires) which are critical for Hilan auth.
            // Use the serde module free function (non-deprecated).
            if cookie_store::serde::json::save_incl_expired_and_nonpersistent(&store, &mut file)
                .is_ok()
            {
                drop(file);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = fs::set_permissions(&cookie_path, fs::Permissions::from_mode(0o600));
                }
                tracing::debug!("saved session cookies to {}", cookie_path.display());
            }
        }
    }

    pub async fn payslip(
        &mut self,
        month: NaiveDate,
        output: Option<&Path>,
    ) -> Result<PayslipDownload> {
        self.login().await?;

        let org_id = self.org_id.as_ref().context("missing org ID after login")?;

        let url = format!(
            "{}/Hilannetv2/PersonalFile/PdfPaySlip.aspx?Date=01/{:02}/{:04}&UserId={}{}",
            self.base_url,
            month.month(),
            month.year(),
            org_id,
            self.config.username
        );

        tracing::debug!("GET payslip {} (UserId=<redacted>)", month.format("%Y-%m"));
        let bytes = self
            .send_bytes_with_retry(&format!("GET payslip {}", month.format("%Y-%m")), &url)
            .await?;
        if !bytes.starts_with(b"%PDF") {
            bail!(
                "Payslip download did not return a PDF for {}. The session may be invalid or the payslip is unavailable.",
                month.format("%Y-%m")
            );
        }

        let path = self.resolve_payslip_path(month, output);
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            fs::create_dir_all(parent)
                .with_context(|| format!("create output directory {}", parent.display()))?;
        }

        fs::write(&path, &bytes).with_context(|| format!("write {}", path.display()))?;

        Ok(PayslipDownload {
            month,
            path,
            size_bytes: bytes.len(),
        })
    }

    pub async fn salary(&mut self, months: u32) -> Result<SalarySummary> {
        if months == 0 {
            bail!("months must be greater than 0");
        }

        self.login().await?;

        let latest_month = previous_month_start(Local::now().date_naive());
        let oldest_month = shift_month(latest_month, -((months - 1) as i32));
        let month_range = month_range(oldest_month, months);
        let date_picker_state = format!(
            "01/{:02}/{:04},0,30/{:02}/{:04},0",
            oldest_month.month(),
            oldest_month.year(),
            latest_month.month(),
            latest_month.year()
        );

        let url = format!(
            "{}/Hilannetv2/PersonalFile/SalaryAllSummary.aspx",
            self.base_url
        );

        let direct_html = self.post_salary_page(&url, &date_picker_state, &[]).await?;
        let html = if contains_salary_rows(&direct_html)? {
            direct_html
        } else {
            let hidden_fields = self.fetch_salary_hidden_fields(&url).await?;
            if hidden_fields.is_empty() {
                bail!("Salary summary page did not return a salary table or ASP.NET hidden fields");
            }
            self.post_salary_page(&url, &date_picker_state, &hidden_fields)
                .await?
        };

        self.parse_salary_summary(&html, month_range)
    }

    fn resolve_payslip_path(&self, month: NaiveDate, output: Option<&Path>) -> PathBuf {
        let default_name = month.format(self.config.payslip_fmt()).to_string();

        match output {
            Some(path) if path.is_dir() => path.join(default_name),
            Some(path) => path.to_path_buf(),
            None => self
                .config
                .payslip_folder
                .as_ref()
                .map(PathBuf::from)
                .unwrap_or_default()
                .join(default_name),
        }
    }

    async fn post_salary_page(
        &mut self,
        url: &str,
        date_picker_state: &str,
        hidden_fields: &[(String, String)],
    ) -> Result<String> {
        let mut form = Vec::with_capacity(hidden_fields.len() + 3);
        for (key, value) in hidden_fields {
            if key != "__DatePicker_State" {
                form.push((key.clone(), value.clone()));
            }
        }

        if !form.iter().any(|(key, _)| key == "__EVENTTARGET") {
            form.push(("__EVENTTARGET".to_string(), String::new()));
        }
        if !form.iter().any(|(key, _)| key == "__EVENTARGUMENT") {
            form.push(("__EVENTARGUMENT".to_string(), String::new()));
        }
        form.push((
            "__DatePicker_State".to_string(),
            date_picker_state.to_string(),
        ));

        tracing::debug!("POST {} ({} form fields)", url, form.len());
        let (status, text) = self
            .send_with_retry("POST salary summary", true, |c| c.post(url).form(&form))
            .await?;
        if !status.is_success() {
            bail!("Salary summary request failed with HTTP {}", status);
        }

        Ok(text)
    }

    async fn fetch_salary_hidden_fields(&mut self, url: &str) -> Result<Vec<(String, String)>> {
        tracing::debug!("GET (salary hidden fields) {}", url);
        let (status, text) = self
            .send_with_retry("GET salary summary page", true, |c| c.get(url))
            .await?;
        if !status.is_success() {
            bail!("Salary summary page request failed with HTTP {}", status);
        }

        let document = Html::parse_document(&text);
        let selector = Selector::parse(r#"input[type="hidden"]"#).unwrap();

        let hidden_fields = document
            .select(&selector)
            .filter_map(|input| {
                let name = input.value().attr("name")?;
                if !name.starts_with("__") {
                    return None;
                }
                Some((
                    name.to_string(),
                    input.value().attr("value").unwrap_or("").to_string(),
                ))
            })
            .collect();

        Ok(hidden_fields)
    }

    fn parse_salary_summary(&self, html: &str, months: Vec<NaiveDate>) -> Result<SalarySummary> {
        let rows = extract_salary_rows(html)?;

        let (label, amounts) = rows
            .into_iter()
            .find_map(|row| {
                let (label, cells) = row.split_first()?;
                let amounts: Vec<u64> = cells
                    .iter()
                    .filter_map(|cell| extract_amount(cell))
                    .collect();
                if amounts.len() >= months.len() {
                    Some((label.clone(), amounts))
                } else {
                    None
                }
            })
            .ok_or_else(|| anyhow!("Could not find a salary row in the summary table"))?;

        let amounts = amounts[amounts.len() - months.len()..].to_vec();
        let entries: Vec<SalaryEntry> = months
            .into_iter()
            .zip(amounts)
            .map(|(month, amount)| SalaryEntry { month, amount })
            .collect();

        let percent_diff = entries
            .windows(2)
            .last()
            .and_then(|pair| percent_diff(pair[0].amount, pair[1].amount));

        Ok(SalarySummary {
            label,
            entries,
            percent_diff,
        })
    }

    /// Send an HTTP request with optional retry on transient errors and
    /// automatic re-authentication on session expiry.
    ///
    /// `build_request` is called on each attempt to produce a fresh `RequestBuilder`
    /// (since `send()` consumes it). Returns `(StatusCode, response_body)`.
    ///
    /// When `retryable` is `false` the request is sent exactly once — no retries
    /// on transient errors and no re-login on session expiry. Use `false` for
    /// state-changing write submissions that must not be replayed.
    async fn send_with_retry(
        &mut self,
        label: &str,
        retryable: bool,
        build_request: impl Fn(&reqwest::Client) -> reqwest::RequestBuilder,
    ) -> Result<(reqwest::StatusCode, String)> {
        const MAX_RETRIES: u32 = 3;
        let mut attempt = 0u32;
        loop {
            let result = build_request(&self.client)
                .send()
                .await
                .with_context(|| label.to_string());

            match result {
                Err(e) if retryable && attempt < MAX_RETRIES && is_transient(&e) => {
                    attempt += 1;
                    retry_backoff(attempt, MAX_RETRIES).await;
                }
                Err(e) => return Err(e),
                Ok(resp) => {
                    if retryable && is_login_redirect(&resp) && attempt < MAX_RETRIES {
                        self.logged_in = false;
                        self.login().await?;
                        attempt += 1;
                        continue;
                    }
                    let status = resp.status();
                    if retryable && is_transient_status(status) && attempt < MAX_RETRIES {
                        attempt += 1;
                        retry_backoff(attempt, MAX_RETRIES).await;
                        continue;
                    }
                    let body = resp
                        .text()
                        .await
                        .with_context(|| format!("read body from {label}"))?;
                    return Ok((status, body));
                }
            }
        }
    }

    /// GET a URL with retry, returning the response as raw bytes.
    ///
    /// Used for binary downloads (e.g. payslip PDFs) that cannot go through
    /// `send_with_retry` (which reads the body as text).
    async fn send_bytes_with_retry(&mut self, label: &str, url: &str) -> Result<Vec<u8>> {
        const MAX_RETRIES: u32 = 3;
        let mut attempt = 0u32;
        loop {
            let result = self
                .client
                .get(url)
                .send()
                .await
                .with_context(|| label.to_string());

            match result {
                Err(e) if attempt < MAX_RETRIES && is_transient(&e) => {
                    attempt += 1;
                    retry_backoff(attempt, MAX_RETRIES).await;
                }
                Err(e) => return Err(e),
                Ok(resp) => {
                    if is_login_redirect(&resp) && attempt < MAX_RETRIES {
                        self.logged_in = false;
                        self.login().await?;
                        attempt += 1;
                        continue;
                    }
                    let status = resp.status();
                    if is_transient_status(status) && attempt < MAX_RETRIES {
                        attempt += 1;
                        retry_backoff(attempt, MAX_RETRIES).await;
                        continue;
                    }
                    if !status.is_success() {
                        bail!("{label} returned HTTP {status}");
                    }
                    let body = resp
                        .bytes()
                        .await
                        .with_context(|| format!("read bytes from {label}"))?;
                    return Ok(body.to_vec());
                }
            }
        }
    }

    /// GET a URL and return the response body as text.
    ///
    /// Retries on transient errors and re-authenticates on session expiry.
    pub async fn get_text(&mut self, url: &str) -> Result<String> {
        let (status, body) = self
            .send_with_retry(&format!("GET {url}"), true, |c| c.get(url))
            .await?;
        if !status.is_success() {
            bail!("GET {url} returned HTTP {status}");
        }
        Ok(body)
    }

    /// GET an .aspx page, parse ALL form fields from `<form id="aspnetForm">`, return (html, fields).
    ///
    /// Retries on transient errors and re-authenticates on session expiry.
    #[allow(dead_code)] // shared attendance/WebForms helper
    pub async fn get_aspx_form(&mut self, url: &str) -> Result<(String, BTreeMap<String, String>)> {
        tracing::debug!("GET (aspx form) {}", url);
        let (status, html) = self
            .send_with_retry(&format!("GET {url}"), true, |c| c.get(url))
            .await?;
        if !status.is_success() {
            bail!("GET {url} returned HTTP {status}");
        }
        let fields = parse_aspx_form_fields(&html);
        tracing::debug!("Parsed {} form fields from {}", fields.len(), url);
        Ok((html, fields))
    }

    /// POST an .aspx page with full form replay.
    ///
    /// Starts with `base_fields`, applies `overrides` (replacing existing keys or adding new ones),
    /// then adds the submit button field. POSTs as `application/x-www-form-urlencoded`.
    ///
    /// When `retryable` is `true`, retries on transient errors and re-authenticates
    /// on session expiry. Pass `false` for state-changing submissions (e.g. attendance
    /// writes) that must fire exactly once.
    #[allow(dead_code)] // shared attendance/WebForms helper
    pub async fn post_aspx_form(
        &mut self,
        url: &str,
        base_fields: &BTreeMap<String, String>,
        overrides: &[(&str, &str)],
        button_name: &str,
        button_value: &str,
        retryable: bool,
    ) -> Result<String> {
        tracing::debug!(
            "POST (aspx form) {} ({} base fields, {} overrides)",
            url,
            base_fields.len(),
            overrides.len()
        );
        let mut merged: BTreeMap<String, String> = base_fields.clone();
        for &(key, value) in overrides {
            merged.insert(key.to_string(), value.to_string());
        }
        merged.insert(button_name.to_string(), button_value.to_string());
        let form_pairs: Vec<(String, String)> = merged.into_iter().collect();

        let (status, text) = self
            .send_with_retry(&format!("POST {url}"), retryable, |c| {
                c.post(url).form(&form_pairs)
            })
            .await?;
        if !status.is_success() {
            bail!("POST {url} returned HTTP {status}");
        }
        Ok(text)
    }

    /// Call an ASMX JSON API endpoint.
    ///
    /// `POST /Hilannetv2/Services/Public/WS/{service}.asmx/{method}`
    /// with `Content-Type: application/json` and body `{}`.
    /// Retries on transient errors and re-authenticates on session expiry.
    /// These are read-only JSON-RPC calls, safe to retry.
    pub async fn asmx_call<T: serde::de::DeserializeOwned>(
        &mut self,
        service: &str,
        method: &str,
    ) -> Result<T> {
        let url = format!(
            "{}/Hilannetv2/Services/Public/WS/{}.asmx/{}",
            self.base_url, service, method
        );

        tracing::debug!("POST (asmx) {}/{}", service, method);
        let (status, text) = self
            .send_with_retry(&format!("POST {url}"), true, |c| {
                c.post(&url)
                    .header("Content-Type", "application/json")
                    .body("{}")
            })
            .await?;
        if !status.is_success() {
            bail!("{service}/{method} returned HTTP {status}: {text}");
        }
        let parsed: T = serde_json::from_str(&text)
            .with_context(|| format!("parse JSON from {service}/{method}"))?;
        Ok(parsed)
    }
}

/// Parse all form fields from an ASP.NET WebForms page.
///
/// Looks for `<form id="aspnetForm">` first; falls back to the first `<form>` if not found.
/// Extracts hidden inputs, text inputs, checkboxes, selects, and textareas.
/// Skips `input[type=submit]` (buttons are added explicitly by the caller).
pub fn parse_aspx_form_fields(html: &str) -> BTreeMap<String, String> {
    let document = Html::parse_document(html);
    let mut fields = BTreeMap::new();

    // Try aspnetForm first, fall back to first <form>
    let form_sel = Selector::parse(r#"form#aspnetForm"#).unwrap();
    let form_fallback_sel = Selector::parse("form").unwrap();
    let form_root = document
        .select(&form_sel)
        .next()
        .or_else(|| document.select(&form_fallback_sel).next());

    let Some(form) = form_root else {
        return fields;
    };

    // hidden + text inputs
    let input_sel = Selector::parse("input").unwrap();
    for input in form.select(&input_sel) {
        let el = input.value();
        let input_type = el.attr("type").unwrap_or("text").to_lowercase();
        let Some(name) = el.attr("name") else {
            continue;
        };
        if name.is_empty() {
            continue;
        }

        match input_type.as_str() {
            "submit" | "button" | "image" | "reset" => continue,
            "checkbox" => {
                if el.attr("checked").is_some() {
                    fields.insert(name.to_string(), "on".to_string());
                }
            }
            "radio" => {
                if el.attr("checked").is_some() {
                    let value = el.attr("value").unwrap_or("on");
                    fields.insert(name.to_string(), value.to_string());
                }
            }
            _ => {
                // hidden, text, password, etc.
                let value = el.attr("value").unwrap_or("");
                fields.insert(name.to_string(), value.to_string());
            }
        }
    }

    // select elements
    let select_sel = Selector::parse("select").unwrap();
    let option_sel = Selector::parse("option").unwrap();
    let selected_option_sel = Selector::parse("option[selected]").unwrap();

    for select in form.select(&select_sel) {
        let Some(name) = select.value().attr("name") else {
            continue;
        };
        if name.is_empty() {
            continue;
        }

        // Prefer the option with `selected` attribute, otherwise first option
        let chosen = select
            .select(&selected_option_sel)
            .next()
            .or_else(|| select.select(&option_sel).next());

        if let Some(opt) = chosen {
            let value = opt.value().attr("value").unwrap_or("");
            fields.insert(name.to_string(), value.to_string());
        }
    }

    // textarea elements
    let textarea_sel = Selector::parse("textarea").unwrap();
    for textarea in form.select(&textarea_sel) {
        let Some(name) = textarea.value().attr("name") else {
            continue;
        };
        if name.is_empty() {
            continue;
        }
        let text: String = textarea.text().collect();
        fields.insert(name.to_string(), text);
    }

    fields
}

/// Format form fields for dry-run display, masking sensitive values.
#[allow(dead_code)] // shared attendance/WebForms helper
pub fn format_form_fields_for_display(
    fields: &BTreeMap<String, String>,
    overrides: &[(&str, &str)],
) -> String {
    let override_keys: std::collections::HashSet<&str> =
        overrides.iter().map(|&(k, _)| k).collect();

    let mut lines = Vec::new();

    // Sensitive patterns to mask
    let sensitive_patterns = ["__VIEWSTATE", "password", "Password", "token", "Token"];

    for (key, value) in fields {
        let is_override = override_keys.contains(key.as_str());
        let marker = if is_override { " [OVERRIDE]" } else { "" };

        let display_value =
            if sensitive_patterns.iter().any(|pat| key.contains(pat)) && value.len() > 8 {
                format!("{}...({} chars)", &value[..4], value.len())
            } else if value.len() > 80 {
                format!("{}...({} chars)", &value[..40], value.len())
            } else {
                value.to_string()
            };

        lines.push(format!("  {key} = {display_value}{marker}"));
    }

    // Show overrides that aren't already in fields
    for &(key, value) in overrides {
        if !fields.contains_key(key) {
            lines.push(format!("  {key} = {value} [NEW]"));
        }
    }

    lines.join("\n")
}

fn contains_salary_rows(html: &str) -> Result<bool> {
    Ok(!extract_salary_rows(html)?.is_empty())
}

fn extract_salary_rows(html: &str) -> Result<Vec<Vec<String>>> {
    let document = Html::parse_document(html);
    let selector = Selector::parse("tr.RSGrid, tr.ARSGrid")
        .map_err(|_| anyhow!("failed to build salary row selector"))?;
    let cell_selector =
        Selector::parse("td").map_err(|_| anyhow!("failed to build salary cell selector"))?;

    let rows = document
        .select(&selector)
        .map(|row| {
            row.select(&cell_selector)
                .map(|cell| {
                    cell.text()
                        .map(str::trim)
                        .filter(|text| !text.is_empty())
                        .collect::<Vec<_>>()
                        .join(" ")
                })
                .filter(|text| !text.is_empty())
                .collect::<Vec<_>>()
        })
        .filter(|row| !row.is_empty())
        .collect();

    Ok(rows)
}

fn extract_amount(cell: &str) -> Option<u64> {
    let digits: String = cell.chars().filter(|ch| ch.is_ascii_digit()).collect();
    if digits.is_empty() {
        None
    } else {
        digits.parse().ok()
    }
}

pub fn previous_month_start(today: NaiveDate) -> NaiveDate {
    shift_month(today.with_day(1).unwrap(), -1)
}

pub(crate) fn month_range(start: NaiveDate, months: u32) -> Vec<NaiveDate> {
    (0..months)
        .map(|offset| shift_month(start, offset as i32))
        .collect()
}

pub(crate) fn shift_month(month_start: NaiveDate, delta_months: i32) -> NaiveDate {
    let total_months = month_start.year() * 12 + month_start.month0() as i32 + delta_months;
    let year = total_months.div_euclid(12);
    let month0 = total_months.rem_euclid(12) as u32;
    NaiveDate::from_ymd_opt(year, month0 + 1, 1).unwrap()
}

/// Signed percentage change from `previous` to `current`.
/// Returns `None` when `previous` is zero (division by zero).
fn percent_diff(previous: u64, current: u64) -> Option<f64> {
    if previous == 0 {
        None
    } else {
        Some((current as f64 - previous as f64) / previous as f64 * 100.0)
    }
}

/// Percent-encode a string for use in URL query parameters / form bodies.
///
/// Delegates to the `urlencoding` crate which handles non-ASCII correctly.
#[allow(dead_code)] // login now uses reqwest::form(); kept as utility
fn urlencode(s: &str) -> String {
    urlencoding::encode(s).into_owned()
}

/// Check whether an error is transient (connection reset, timeout, etc.)
/// and therefore worth retrying.
fn is_transient(err: &anyhow::Error) -> bool {
    for cause in err.chain() {
        if let Some(req_err) = cause.downcast_ref::<reqwest::Error>() {
            if req_err.is_timeout() || req_err.is_connect() || req_err.is_request() {
                return true;
            }
        }
        if let Some(io_err) = cause.downcast_ref::<std::io::Error>() {
            return matches!(
                io_err.kind(),
                std::io::ErrorKind::ConnectionReset
                    | std::io::ErrorKind::ConnectionRefused
                    | std::io::ErrorKind::BrokenPipe
                    | std::io::ErrorKind::TimedOut
            );
        }
    }
    false
}

/// Check whether an HTTP status code is a transient server error.
fn is_transient_status(status: reqwest::StatusCode) -> bool {
    matches!(status.as_u16(), 500 | 502 | 503)
}

/// Detect whether a response was redirected to the Hilan login page,
/// indicating session expiry. Checks the final URL after redirect following.
fn is_login_redirect(resp: &reqwest::Response) -> bool {
    let url = resp.url().as_str().to_lowercase();
    url.contains("/login") || url.contains("/signin") || url.contains("/logon")
}

/// Sleep with exponential backoff: 500ms, 1s, 2s, ...
async fn retry_backoff(attempt: u32, max_retries: u32) {
    let delay = Duration::from_millis(500 * 2u64.pow(attempt - 1));
    eprintln!("Retrying in {delay:?} (attempt {attempt}/{max_retries})");
    tokio::time::sleep(delay).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- percent_diff ----------------------------------------------------------

    #[test]
    fn test_percent_diff_positive() {
        let diff = percent_diff(10_000, 11_000).unwrap();
        assert!(
            (diff - 10.0).abs() < f64::EPSILON,
            "expected +10.0, got {diff}"
        );
    }

    #[test]
    fn test_percent_diff_negative() {
        let diff = percent_diff(10_000, 9_000).unwrap();
        assert!(
            (diff - (-10.0)).abs() < f64::EPSILON,
            "expected -10.0, got {diff}"
        );
    }

    #[test]
    fn test_percent_diff_zero_previous() {
        assert!(percent_diff(0, 5_000).is_none());
    }

    #[test]
    fn test_percent_diff_no_change() {
        let diff = percent_diff(10_000, 10_000).unwrap();
        assert!(diff.abs() < f64::EPSILON, "expected 0.0, got {diff}");
    }

    // -- urlencode -------------------------------------------------------------

    #[test]
    fn test_urlencode_ascii() {
        assert_eq!(urlencode("hello"), "hello");
    }

    #[test]
    fn test_urlencode_spaces() {
        // urlencoding::encode uses %20 for spaces (RFC 3986).
        assert_eq!(urlencode("a b"), "a%20b");
    }

    #[test]
    fn test_urlencode_special_chars() {
        let encoded = urlencode("p@ss&w=rd!");
        assert!(!encoded.contains('@'));
        assert!(!encoded.contains('&'));
        assert!(!encoded.contains('='));
    }

    #[test]
    fn test_urlencode_non_ascii() {
        // Hebrew letter Alef (U+05D0) should be percent-encoded.
        let encoded = urlencode("א");
        assert!(
            encoded.starts_with('%'),
            "non-ASCII should be encoded: {encoded}"
        );
    }
}
