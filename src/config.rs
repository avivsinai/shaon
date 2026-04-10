use anyhow::{Context, Result};
use secrecy::SecretString;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
#[cfg(test)]
use std::sync::{Mutex, OnceLock};

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
        migrate_config_if_needed()?;

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

        migrate_subdomain_state_if_needed(&config.subdomain)?;

        Ok(config)
    }

    /// Retrieve the password: keychain first, then legacy config file fallback.
    pub fn get_password(&self) -> Result<SecretString> {
        // 1. Try environment variable first (CI, scripts, testing)
        if let Ok(pw) = std::env::var("HILAN_PASSWORD") {
            if !pw.is_empty() {
                return Ok(SecretString::from(pw));
            }
            tracing::debug!("ignoring empty HILAN_PASSWORD");
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

/// Returns `~/.hilan/{subdomain}/` for per-org state (cookies, ontology cache).
#[allow(dead_code)] // used in later phases
pub fn subdomain_dir(subdomain: &str) -> PathBuf {
    config_dir().join(subdomain)
}

fn config_path() -> PathBuf {
    config_dir().join("config.toml")
}

fn legacy_config_roots() -> Vec<PathBuf> {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let mut roots = Vec::new();

    if let Ok(xdg_config_home) = std::env::var("XDG_CONFIG_HOME") {
        roots.push(PathBuf::from(xdg_config_home).join("hilan"));
    } else {
        roots.push(PathBuf::from(&home).join(".config").join("hilan"));
    }

    #[cfg(target_os = "macos")]
    roots.push(
        PathBuf::from(home)
            .join("Library")
            .join("Application Support")
            .join("com.hilan.hilan"),
    );

    roots
}

fn migrate_config_if_needed() -> Result<()> {
    let target = config_path();
    if target.exists() {
        return Ok(());
    }

    for legacy_root in legacy_config_roots() {
        let legacy_path = legacy_root.join("config.toml");
        if !legacy_path.exists() {
            continue;
        }

        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("create directory {}", parent.display()))?;
        }
        fs::copy(&legacy_path, &target).with_context(|| {
            format!(
                "migrate config from {} to {}",
                legacy_path.display(),
                target.display()
            )
        })?;
        set_file_permissions_600(&target);
        eprintln!(
            "Migrated config from {} to {}",
            legacy_path.display(),
            target.display()
        );
        return Ok(());
    }

    Ok(())
}

fn migrate_subdomain_state_if_needed(subdomain: &str) -> Result<()> {
    let target = subdomain_dir(subdomain);
    if target.exists() {
        return Ok(());
    }

    for legacy_root in legacy_config_roots() {
        let legacy_dir = legacy_root.join(subdomain);
        if !legacy_dir.is_dir() {
            continue;
        }

        copy_dir_recursively(&legacy_dir, &target)?;
        eprintln!(
            "Migrated state from {} to {}",
            legacy_dir.display(),
            target.display()
        );
        return Ok(());
    }

    Ok(())
}

fn copy_dir_recursively(source: &Path, target: &Path) -> Result<()> {
    fs::create_dir_all(target).with_context(|| format!("create directory {}", target.display()))?;

    for entry in fs::read_dir(source).with_context(|| format!("read {}", source.display()))? {
        let entry = entry.with_context(|| format!("read entry in {}", source.display()))?;
        let source_path = entry.path();
        let target_path = target.join(entry.file_name());

        if entry
            .file_type()
            .with_context(|| format!("read file type for {}", source_path.display()))?
            .is_dir()
        {
            copy_dir_recursively(&source_path, &target_path)?;
        } else {
            fs::copy(&source_path, &target_path).with_context(|| {
                format!(
                    "copy state file from {} to {}",
                    source_path.display(),
                    target_path.display()
                )
            })?;
            set_file_permissions_600(&target_path);
        }
    }

    Ok(())
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
    use secrecy::ExposeSecret;
    use std::ffi::OsString;

    struct EnvGuard {
        home: Option<OsString>,
        xdg_config_home: Option<OsString>,
        hilan_password: Option<OsString>,
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            restore_env("HOME", self.home.as_deref());
            restore_env("XDG_CONFIG_HOME", self.xdg_config_home.as_deref());
            restore_env("HILAN_PASSWORD", self.hilan_password.as_deref());
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
            hilan_password: std::env::var_os("HILAN_PASSWORD"),
        }
    }

    fn test_root(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "hilan-config-tests-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
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
    fn load_migrates_legacy_xdg_config_and_state() {
        let _env_guard = test_env_lock().lock().unwrap();
        let _saved_env = preserve_env();

        let root = test_root("migrate-xdg");
        let home = root.join("home");
        let xdg = root.join("xdg");
        let legacy_root = xdg.join("hilan");
        let legacy_state_dir = legacy_root.join("acme");

        fs::create_dir_all(&legacy_state_dir).unwrap();
        fs::write(
            legacy_root.join("config.toml"),
            r#"
subdomain = "acme"
username = "12345"
password = "legacy-password"
"#,
        )
        .unwrap();
        fs::write(legacy_state_dir.join("types.json"), r#"{"ok":true}"#).unwrap();
        fs::write(legacy_state_dir.join("cookies.json"), r#"{"cookies":[]}"#).unwrap();

        std::env::set_var("HOME", &home);
        std::env::set_var("XDG_CONFIG_HOME", &xdg);
        std::env::remove_var("HILAN_PASSWORD");

        let config = Config::load().unwrap();

        assert_eq!(config.subdomain, "acme");
        assert!(config_path().exists(), "config should be migrated to ~/.hilan");
        assert!(
            subdomain_dir("acme").join("types.json").exists(),
            "subdomain state should be migrated to ~/.hilan/acme"
        );
        assert!(
            subdomain_dir("acme").join("cookies.json").exists(),
            "cookie state should be migrated to ~/.hilan/acme"
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn get_password_ignores_empty_hilan_password() {
        let _env_guard = test_env_lock().lock().unwrap();
        let _saved_env = preserve_env();

        std::env::set_var("HILAN_PASSWORD", "");

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
}
