use anyhow::{Context, Result};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize, Serialize)]
pub struct Config {
    pub subdomain: String,
    pub username: String,
    pub password: String,
    pub payslip_folder: Option<String>,
    /// strftime format for payslip filenames; defaults to "%Y-%m.pdf"
    pub payslip_format: Option<String>,
}

impl Config {
    /// Load config from `~/.config/hilan/config.toml`.
    pub fn load() -> Result<Self> {
        let path = config_path();
        if !path.exists() {
            print_setup_instructions(&path);
            anyhow::bail!("Config file not found at {}", path.display());
        }
        let content =
            fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
        let config: Config =
            toml::from_str(&content).with_context(|| format!("parse {}", path.display()))?;
        Ok(config)
    }

    /// Returns the default payslip filename format.
    #[allow(dead_code)] // used in later phases
    pub fn payslip_fmt(&self) -> &str {
        self.payslip_format.as_deref().unwrap_or("%Y-%m.pdf")
    }
}

/// Returns the config directory: `~/.config/hilan/`
pub fn config_dir() -> PathBuf {
    ProjectDirs::from("com", "hilan", "hilan")
        .map(|p| p.config_dir().to_path_buf())
        .unwrap_or_else(|| dirs_fallback().join("hilan"))
}

/// Returns `~/.config/hilan/{subdomain}/` for per-org state (cookies, ontology cache).
#[allow(dead_code)] // used in later phases
pub fn subdomain_dir(subdomain: &str) -> PathBuf {
    config_dir().join(subdomain)
}

fn config_path() -> PathBuf {
    config_dir().join("config.toml")
}

fn dirs_fallback() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".config").join("hilan")
}

fn print_setup_instructions(path: &Path) {
    eprintln!("hilan: config file not found.");
    eprintln!();
    eprintln!("Create {} with:", path.display());
    eprintln!();
    eprintln!("  subdomain = \"YOUR_COMPANY\"");
    eprintln!("  username  = \"YOUR_ID_NUMBER\"");
    eprintln!("  password  = \"YOUR_PASSWORD\"");
    eprintln!();
    eprintln!("Optional fields:");
    eprintln!("  payslip_folder = \"/path/to/payslips\"");
    eprintln!("  payslip_format = \"%Y-%m.pdf\"");
    eprintln!();
    eprintln!("The subdomain is the part before .hilan.co.il in your company's Hilan URL.");
}
