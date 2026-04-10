use anyhow::{Context, Result};
use secrecy::SecretString;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

const KEYRING_SERVICE: &str = "hilan-cli";

/// Build a keyring entry with a stable target so macOS Keychain associates the
/// item with the binary identity rather than a generic account.  The binary
/// must be ad-hoc codesigned (`codesign -s - -f <binary>`) for silent keychain
/// access on macOS — otherwise the OS will prompt on every access.
fn keyring_entry(subdomain: &str, username: &str) -> Result<keyring::Entry> {
    let account = format!("{}/{}", subdomain, username);
    keyring::Entry::new(KEYRING_SERVICE, &account).context("create keyring entry")
}

#[derive(Clone, Deserialize, Serialize)]
pub struct Config {
    pub subdomain: String,
    pub username: String,

    /// Deprecated: only kept for migration from plaintext config files.
    /// New installs store the password exclusively in the OS keychain.
    #[serde(skip_serializing, default)]
    pub password: Option<String>,

    pub payslip_folder: Option<String>,
    /// strftime format for payslip filenames; defaults to "%Y-%m.pdf"
    pub payslip_format: Option<String>,
}

impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Config")
            .field("subdomain", &self.subdomain)
            .field("username", &self.username)
            .field("password", &"[REDACTED]")
            .field("payslip_folder", &self.payslip_folder)
            .field("payslip_format", &self.payslip_format)
            .finish()
    }
}

impl Config {
    /// Load config from `~/.hilan/config.toml`.
    pub fn load() -> Result<Self> {
        let path = config_path();
        if !path.exists() {
            print_setup_instructions(&path);
            anyhow::bail!("Config file not found at {}", path.display());
        }

        check_file_permissions(&path);

        let content =
            fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
        let config: Config =
            toml::from_str(&content).with_context(|| format!("parse {}", path.display()))?;

        if config.password.is_some() {
            eprintln!(
                "warning: password found in config file (plaintext). \
                 Run `hilan auth --migrate` to move it to the OS keychain."
            );
        }

        Ok(config)
    }

    /// Retrieve the password: keychain first, then legacy config file fallback.
    pub fn get_password(&self) -> Result<SecretString> {
        // 1. Try environment variable first (CI, scripts, testing)
        if let Ok(pw) = std::env::var("HILAN_PASSWORD") {
            return Ok(SecretString::from(pw));
        }

        // 2. Try OS keychain
        match keyring_entry(&self.subdomain, &self.username) {
            Ok(entry) => match entry.get_password() {
                Ok(pw) => return Ok(SecretString::from(pw)),
                Err(keyring::Error::NoEntry) => {}
                Err(e) => {
                    tracing::debug!("keychain lookup failed: {e}");
                }
            },
            Err(e) => {
                tracing::debug!("keychain entry creation failed: {e}");
            }
        }

        // 3. Fall back to config file password (legacy)
        match &self.password {
            Some(pw) => {
                eprintln!(
                    "warning: using plaintext password from config. \
                     Run `hilan auth --migrate` to move it to the OS keychain."
                );
                Ok(SecretString::from(pw.clone()))
            }
            None => anyhow::bail!(
                "No password found. Set HILAN_PASSWORD env var, run `hilan auth`, \
                 or add password to ~/.hilan/config.toml"
            ),
        }
    }

    /// Migrate plaintext password to OS keychain, then rewrite config.toml without it.
    pub fn migrate_to_keychain(&mut self) -> Result<()> {
        let pw = match &self.password {
            Some(pw) => pw.clone(),
            None => anyhow::bail!("No plaintext password in config to migrate"),
        };

        let entry = keyring_entry(&self.subdomain, &self.username)?;
        entry
            .set_password(&pw)
            .context("store password in OS keychain")?;

        // Clear the in-memory password and rewrite config without it
        self.password = None;
        self.save().context("rewrite config without password")?;

        set_file_permissions_600(&config_path());

        eprintln!("Password migrated to OS keychain and removed from config file.");
        Ok(())
    }

    /// Store a new password in the OS keychain. Verifies the write succeeded.
    pub fn store_password_in_keychain(&self, password: &str) -> Result<()> {
        let entry = keyring_entry(&self.subdomain, &self.username)?;
        entry
            .set_password(password)
            .context("store password in OS keychain")?;

        // Verify the password was actually persisted
        match entry.get_password() {
            Ok(stored) if stored == password => Ok(()),
            Ok(_) => anyhow::bail!("keychain stored a different value than expected"),
            Err(e) => anyhow::bail!(
                "keychain write appeared to succeed but read-back failed: {e}. \
                 Try setting HILAN_PASSWORD env var or adding password to ~/.hilan/config.toml"
            ),
        }
    }

    /// Write the current config back to disk (without the password field).
    fn save(&self) -> Result<()> {
        let path = config_path();
        let content = toml::to_string_pretty(self).context("serialize config")?;
        fs::write(&path, content).with_context(|| format!("write {}", path.display()))?;
        Ok(())
    }

    /// Returns the default payslip filename format.
    #[allow(dead_code)] // used in later phases
    pub fn payslip_fmt(&self) -> &str {
        self.payslip_format.as_deref().unwrap_or("%Y-%m.pdf")
    }
}

/// Returns the config directory: `~/.hilan/`
pub fn config_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".hilan")
}

/// Returns `~/.config/hilan/{subdomain}/` for per-org state (cookies, ontology cache).
#[allow(dead_code)] // used in later phases
pub fn subdomain_dir(subdomain: &str) -> PathBuf {
    config_dir().join(subdomain)
}

fn config_path() -> PathBuf {
    config_dir().join("config.toml")
}

/// Warn if config file is group/world readable (Unix only).
#[cfg(unix)]
fn check_file_permissions(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(meta) = fs::metadata(path) {
        let mode = meta.permissions().mode();
        if mode & 0o077 != 0 {
            eprintln!(
                "warning: {} has mode {:#o} — group/world readable. \
                 Run: chmod 600 {}",
                path.display(),
                mode,
                path.display()
            );
        }
    }
}

#[cfg(not(unix))]
fn check_file_permissions(_path: &Path) {
    // No-op on non-Unix platforms
}

/// Set config file to 0600 (Unix only).
#[cfg(unix)]
fn set_file_permissions_600(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let perms = std::fs::Permissions::from_mode(0o600);
    if let Err(e) = fs::set_permissions(path, perms) {
        eprintln!(
            "warning: could not set permissions on {}: {e}",
            path.display()
        );
    }
}

#[cfg(not(unix))]
fn set_file_permissions_600(_path: &Path) {
    // No-op on non-Unix platforms
}

fn print_setup_instructions(path: &Path) {
    eprintln!("hilan: config file not found.");
    eprintln!();
    eprintln!("Create {} with:", path.display());
    eprintln!();
    eprintln!("  subdomain = \"YOUR_COMPANY\"");
    eprintln!(
        "  username  = \"YOUR_EMPLOYEE_ID\"   # e.g. \"27\" — the ID you use to log in to Hilan"
    );
    eprintln!();
    eprintln!("Then run `hilan auth` to store your password in the OS keychain.");
    eprintln!();
    eprintln!("Optional fields:");
    eprintln!("  payslip_folder = \"/path/to/payslips\"");
    eprintln!("  payslip_format = \"%Y-%m.pdf\"");
    eprintln!();
    eprintln!("The subdomain is the part before .hilan.co.il in your company's Hilan URL.");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_redacts_password() {
        let config = Config {
            subdomain: "acme".to_string(),
            username: "12345".to_string(),
            password: Some("supersecret".to_string()),
            payslip_folder: None,
            payslip_format: None,
        };
        let debug_output = format!("{config:?}");
        assert!(
            debug_output.contains("[REDACTED]"),
            "Debug output should redact password"
        );
        assert!(
            !debug_output.contains("supersecret"),
            "Debug output must not contain actual password"
        );
    }

    #[test]
    fn config_serialization_omits_password() {
        let config = Config {
            subdomain: "acme".to_string(),
            username: "12345".to_string(),
            password: Some("supersecret".to_string()),
            payslip_folder: None,
            payslip_format: None,
        };
        let serialized = toml::to_string_pretty(&config).unwrap();
        assert!(
            !serialized.contains("password"),
            "Serialized config must not contain password field"
        );
        assert!(
            !serialized.contains("supersecret"),
            "Serialized config must not contain password value"
        );
    }

    #[test]
    fn config_deserializes_without_password() {
        let toml_str = r#"
            subdomain = "acme"
            username = "12345"
        "#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.subdomain, "acme");
        assert_eq!(config.username, "12345");
        assert!(config.password.is_none());
    }

    #[test]
    fn config_deserializes_with_legacy_password() {
        let toml_str = r#"
            subdomain = "acme"
            username = "12345"
            password = "oldpass"
        "#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.password.as_deref(), Some("oldpass"));
    }
}
