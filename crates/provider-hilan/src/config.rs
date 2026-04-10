use anyhow::{Context, Result};
use rand::RngCore;
use secrecy::SecretString;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
#[cfg(test)]
use std::sync::{Mutex, OnceLock};

const KEYRING_SERVICE: &str = "shaon-cli";
const SESSION_KEYRING_SERVICE: &str = "shaon-session-key";
const SESSION_KEY_LEN: usize = 32;

/// Build a keyring entry with a stable target so macOS Keychain associates the
/// item with the binary identity rather than a generic account.  The binary
/// must be ad-hoc codesigned (`codesign -s - -f <binary>`) for silent keychain
/// access on macOS — otherwise the OS will prompt on every access.
fn keyring_entry(subdomain: &str, username: &str) -> Result<keyring::Entry> {
    let account = format!("{}/{}", subdomain, username);
    keyring::Entry::new(KEYRING_SERVICE, &account).context("create keyring entry")
}

fn session_keyring_entry(subdomain: &str, username: &str) -> Result<keyring::Entry> {
    let account = format!("{}/{}", subdomain, username);
    keyring::Entry::new(SESSION_KEYRING_SERVICE, &account).context("create session keyring entry")
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
    /// Load config from `~/.shaon/config.toml`.
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
                 Run `shaon auth --migrate` to move it to the OS keychain."
            );
        }

        Ok(config)
    }

    /// Retrieve the password: keychain first, then legacy config file fallback.
    pub fn get_password(&self) -> Result<SecretString> {
        // 1. Try environment variable first (CI, scripts, testing)
        if let Ok(pw) = std::env::var("SHAON_PASSWORD") {
            if !pw.is_empty() {
                return Ok(SecretString::from(pw));
            }
            tracing::debug!("ignoring empty SHAON_PASSWORD");
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
                     Run `shaon auth --migrate` to move it to the OS keychain."
                );
                Ok(SecretString::from(pw.clone()))
            }
            None => anyhow::bail!(
                "No password found. Set SHAON_PASSWORD env var, run `shaon auth`, \
                 or add password to ~/.shaon/config.toml"
            ),
        }
    }

    /// Migrate plaintext password to OS keychain, then rewrite config.toml without it.
    pub fn migrate_to_keychain(&mut self) -> Result<()> {
        let pw = match &self.password {
            Some(pw) => pw.clone(),
            None => anyhow::bail!("No plaintext password in config to migrate"),
        };

        self.store_password_in_keychain(&pw)?;

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
                 Try setting SHAON_PASSWORD env var or adding password to ~/.shaon/config.toml"
            ),
        }
    }

    /// Load the persisted cookie-encryption key from the OS keychain, if present.
    pub fn get_session_key(&self) -> Result<Option<[u8; SESSION_KEY_LEN]>> {
        let entry = session_keyring_entry(&self.subdomain, &self.username)?;
        match entry.get_password() {
            Ok(encoded) => decode_session_key(&encoded).map(Some),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(anyhow::anyhow!("read session key from OS keychain: {e}")),
        }
    }

    /// Load or create the persisted cookie-encryption key in the OS keychain.
    pub fn get_or_create_session_key(&self) -> Result<[u8; SESSION_KEY_LEN]> {
        if let Some(existing) = self.get_session_key()? {
            return Ok(existing);
        }

        let entry = session_keyring_entry(&self.subdomain, &self.username)?;
        let mut key = [0_u8; SESSION_KEY_LEN];
        let mut rng = rand::rngs::OsRng;
        rng.fill_bytes(&mut key);
        let encoded = encode_session_key(&key);

        entry
            .set_password(&encoded)
            .context("store session key in OS keychain")?;

        match entry.get_password() {
            Ok(stored) if stored == encoded => Ok(key),
            Ok(_) => anyhow::bail!("keychain stored a different session key than expected"),
            Err(e) => {
                anyhow::bail!("session key write appeared to succeed but read-back failed: {e}")
            }
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

/// Returns the config directory: `~/.shaon/`
pub fn config_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".shaon")
}

/// Returns `~/.shaon/{subdomain}/` for per-org state (cookies, ontology cache).
pub fn subdomain_dir(subdomain: &str) -> PathBuf {
    config_dir().join(subdomain)
}

fn config_path() -> PathBuf {
    config_dir().join("config.toml")
}

#[cfg(test)]
pub(crate) fn test_env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
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
    eprintln!("shaon: config file not found.");
    eprintln!();
    eprintln!("Create {} with:", path.display());
    eprintln!();
    eprintln!("  subdomain = \"YOUR_COMPANY\"");
    eprintln!(
        "  username  = \"YOUR_EMPLOYEE_ID\"   # e.g. \"27\" — the ID you use to log in to Hilan"
    );
    eprintln!();
    eprintln!("Then run `shaon auth` to store your password in the OS keychain.");
    eprintln!();
    eprintln!("Optional fields:");
    eprintln!("  payslip_folder = \"/path/to/payslips\"");
    eprintln!("  payslip_format = \"%Y-%m.pdf\"");
    eprintln!();
    eprintln!("The subdomain is the part before .hilan.co.il in your company's Hilan URL.");
}

fn encode_session_key(key: &[u8; SESSION_KEY_LEN]) -> String {
    key.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn decode_session_key(encoded: &str) -> Result<[u8; SESSION_KEY_LEN]> {
    if encoded.len() != SESSION_KEY_LEN * 2 {
        anyhow::bail!(
            "session key has {} hex chars, expected {}",
            encoded.len(),
            SESSION_KEY_LEN * 2
        );
    }

    let mut key = [0_u8; SESSION_KEY_LEN];
    for (idx, chunk) in encoded.as_bytes().chunks_exact(2).enumerate() {
        let text = std::str::from_utf8(chunk).context("session key was not valid UTF-8")?;
        key[idx] = u8::from_str_radix(text, 16)
            .with_context(|| format!("invalid hex in session key at byte {idx}"))?;
    }
    Ok(key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use secrecy::ExposeSecret;
    use std::ffi::OsString;

    struct EnvGuard {
        home: Option<OsString>,
        xdg_config_home: Option<OsString>,
        shaon_password: Option<OsString>,
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            restore_env("HOME", self.home.as_deref());
            restore_env("XDG_CONFIG_HOME", self.xdg_config_home.as_deref());
            restore_env("SHAON_PASSWORD", self.shaon_password.as_deref());
        }
    }

    fn restore_env(key: &str, value: Option<&std::ffi::OsStr>) {
        match value {
            Some(value) => std::env::set_var(key, value),
            None => std::env::remove_var(key),
        }
    }

    fn preserve_env() -> EnvGuard {
        EnvGuard {
            home: std::env::var_os("HOME"),
            xdg_config_home: std::env::var_os("XDG_CONFIG_HOME"),
            shaon_password: std::env::var_os("SHAON_PASSWORD"),
        }
    }

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

    #[test]
    fn get_password_ignores_empty_shaon_password() {
        let _env_guard = test_env_lock().lock().unwrap();
        let _saved_env = preserve_env();

        std::env::set_var("SHAON_PASSWORD", "");

        let config = Config {
            subdomain: format!("acme-{}", std::process::id()),
            username: format!("user-{}", std::process::id()),
            password: Some("fallback-password".to_string()),
            payslip_folder: None,
            payslip_format: None,
        };

        let password = config.get_password().unwrap();
        assert_eq!(password.expose_secret(), "fallback-password");
    }

    #[test]
    fn session_key_hex_round_trip() {
        let original = [
            0x00, 0x01, 0x02, 0x03, 0x10, 0x11, 0x12, 0x13, 0x20, 0x21, 0x22, 0x23, 0x30, 0x31,
            0x32, 0x33, 0x40, 0x41, 0x42, 0x43, 0x50, 0x51, 0x52, 0x53, 0x60, 0x61, 0x62, 0x63,
            0x70, 0x71, 0x72, 0x73,
        ];

        let encoded = encode_session_key(&original);
        let decoded = decode_session_key(&encoded).unwrap();

        assert_eq!(decoded, original);
    }
}
