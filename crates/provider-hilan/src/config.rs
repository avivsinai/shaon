use anyhow::{bail, Context, Result};
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine as _;
use rand::RngCore;
use secrecy::SecretString;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering};
#[cfg(test)]
use std::sync::{Mutex, OnceLock};
use zeroize::Zeroize;

const KEYRING_SERVICE: &str = "shaon-cli";
const KEYRING_SCHEMA_VERSION: u32 = 1;
pub const LOCAL_MASTER_KEY_LEN: usize = 32;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
struct StoredCredentials {
    v: u32,
    password: String,
    local_master_key: String,
}

impl StoredCredentials {
    fn new(password: &str, local_master_key: &[u8; LOCAL_MASTER_KEY_LEN]) -> Self {
        Self {
            v: KEYRING_SCHEMA_VERSION,
            password: password.to_string(),
            local_master_key: encode_local_master_key(local_master_key),
        }
    }

    fn decode_local_master_key(&self) -> Result<[u8; LOCAL_MASTER_KEY_LEN]> {
        if self.v != KEYRING_SCHEMA_VERSION {
            bail!("unsupported shaon-cli keychain schema version {}", self.v);
        }
        decode_local_master_key(&self.local_master_key)
    }
}

#[derive(Clone, Deserialize, Serialize)]
pub struct Config {
    pub subdomain: String,
    pub username: String,

    #[serde(skip_serializing, default)]
    pub password: Option<String>,

    pub payslip_folder: Option<String>,
    pub payslip_format: Option<String>,
}

pub struct PendingStoredCredentials {
    config: Config,
    password: String,
    local_master_key: [u8; LOCAL_MASTER_KEY_LEN],
    clear_plaintext_password: bool,
}

impl PendingStoredCredentials {
    pub fn login_config(&self) -> Config {
        let mut config = self.config.clone();
        config.password = Some(self.password.clone());
        config
    }

    pub fn commit(mut self) -> Result<Config> {
        self.config
            .store_credentials(&self.password, &self.local_master_key)?;

        if self.clear_plaintext_password {
            self.config.password = None;
            self.config
                .save()
                .context("rewrite config without password")?;
            set_file_permissions_600(&config_path());
            eprintln!("Password migrated to OS keychain and removed from config file.");
        } else {
            eprintln!("Credentials stored in OS keychain.");
        }

        Ok(self.config.clone())
    }
}

impl Drop for PendingStoredCredentials {
    fn drop(&mut self) {
        self.password.zeroize();
        self.local_master_key.zeroize();
    }
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
    pub fn load() -> Result<Self> {
        let path = config_path();
        if !path.exists() {
            print_setup_instructions(&path);
            bail!("Config file not found at {}", path.display());
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

    pub fn get_password(&self) -> Result<SecretString> {
        if let Some(pw) = env_var_nonempty("SHAON_PASSWORD") {
            return Ok(SecretString::from(pw));
        }

        if let Some(credentials) = self.load_stored_credentials()? {
            return Ok(SecretString::from(credentials.password));
        }

        match &self.password {
            Some(pw) => {
                eprintln!(
                    "warning: using plaintext password from config. \
                     Run `shaon auth --migrate` to move it to the OS keychain."
                );
                Ok(SecretString::from(pw.clone()))
            }
            None => bail!(
                "No password found. Set SHAON_PASSWORD env var, run `shaon auth`, \
                 or add password to ~/.shaon/config.toml"
            ),
        }
    }

    pub fn get_local_master_key(&self) -> Result<[u8; LOCAL_MASTER_KEY_LEN]> {
        if let Some(encoded) = env_var_nonempty("SHAON_MASTER_KEY") {
            return decode_local_master_key(&encoded);
        }

        match self.load_stored_credentials()? {
            Some(credentials) => credentials.decode_local_master_key(),
            None => {
                bail!("No local master key found. Set SHAON_MASTER_KEY env var or run `shaon auth`")
            }
        }
    }

    pub fn should_skip_local_cache(&self) -> bool {
        env_var_nonempty("SHAON_PASSWORD").is_some()
            && env_var_nonempty("SHAON_MASTER_KEY").is_none()
    }

    pub fn prepare_stored_credentials(&self, password: String) -> PendingStoredCredentials {
        let mut local_master_key = [0_u8; LOCAL_MASTER_KEY_LEN];
        rand::rngs::OsRng.fill_bytes(&mut local_master_key);
        PendingStoredCredentials {
            config: self.clone(),
            password,
            local_master_key,
            clear_plaintext_password: false,
        }
    }

    pub fn prepare_migration(&self) -> Result<PendingStoredCredentials> {
        let pw = match &self.password {
            Some(pw) => pw.clone(),
            None => bail!("No plaintext password in config to migrate"),
        };

        let mut pending = self.prepare_stored_credentials(pw);
        pending.clear_plaintext_password = true;
        Ok(pending)
    }

    pub fn store_credentials(
        &self,
        password: &str,
        local_master_key: &[u8; LOCAL_MASTER_KEY_LEN],
    ) -> Result<()> {
        let account = keyring_account(&self.subdomain, &self.username);
        let credentials = StoredCredentials::new(password, local_master_key);
        let encoded =
            serde_json::to_string(&credentials).context("serialize bundled credentials")?;

        write_keychain_secret(KEYRING_SERVICE, &account, &encoded)?;

        match self.load_stored_credentials() {
            Ok(Some(stored)) if stored == credentials => Ok(()),
            Ok(Some(_)) => bail!("keychain stored different credentials than expected"),
            Ok(None) => bail!("keychain write appeared to succeed but credentials were missing"),
            Err(e) => bail!("keychain write appeared to succeed but read-back failed: {e}"),
        }
    }

    fn load_stored_credentials(&self) -> Result<Option<StoredCredentials>> {
        let account = keyring_account(&self.subdomain, &self.username);
        let Some(raw) = read_keychain_secret(KEYRING_SERVICE, &account)? else {
            return Ok(None);
        };
        parse_stored_credentials(&raw).map(Some)
    }

    fn save(&self) -> Result<()> {
        let path = config_path();
        let content = toml::to_string_pretty(self).context("serialize config")?;
        fs::write(&path, content).with_context(|| format!("write {}", path.display()))?;
        Ok(())
    }

    #[allow(dead_code)]
    pub fn payslip_fmt(&self) -> &str {
        self.payslip_format.as_deref().unwrap_or("%Y-%m.pdf")
    }
}

pub fn config_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".shaon")
}

pub fn subdomain_dir(subdomain: &str) -> PathBuf {
    config_dir().join(subdomain)
}

fn config_path() -> PathBuf {
    config_dir().join("config.toml")
}

fn keyring_account(subdomain: &str, username: &str) -> String {
    format!("{subdomain}/{username}")
}

fn env_var_nonempty(key: &str) -> Option<String> {
    match std::env::var(key) {
        Ok(value) if !value.is_empty() => Some(value),
        Ok(_) => {
            tracing::debug!("ignoring empty {key}");
            None
        }
        Err(_) => None,
    }
}

fn parse_stored_credentials(raw: &str) -> Result<StoredCredentials> {
    let credentials: StoredCredentials =
        serde_json::from_str(raw).context("parse shaon-cli keychain record")?;
    if credentials.v != KEYRING_SCHEMA_VERSION {
        bail!(
            "unsupported shaon-cli keychain schema version {}",
            credentials.v
        );
    }
    Ok(credentials)
}

fn encode_local_master_key(key: &[u8; LOCAL_MASTER_KEY_LEN]) -> String {
    BASE64_STANDARD.encode(key)
}

fn decode_local_master_key(encoded: &str) -> Result<[u8; LOCAL_MASTER_KEY_LEN]> {
    let decoded = BASE64_STANDARD
        .decode(encoded)
        .context("decode SHAON_MASTER_KEY as base64")?;
    if decoded.len() != LOCAL_MASTER_KEY_LEN {
        bail!(
            "local master key has {} bytes, expected {}",
            decoded.len(),
            LOCAL_MASTER_KEY_LEN
        );
    }

    let mut key = [0_u8; LOCAL_MASTER_KEY_LEN];
    key.copy_from_slice(&decoded);
    Ok(key)
}

#[cfg(not(test))]
fn read_keychain_secret(service: &str, account: &str) -> Result<Option<String>> {
    let entry = keyring::Entry::new(service, account)
        .with_context(|| format!("create keyring entry for service {service}"))?;
    match entry.get_password() {
        Ok(secret) => Ok(Some(secret)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(anyhow::anyhow!("read {service} from OS keychain: {e}")),
    }
}

#[cfg(not(test))]
fn write_keychain_secret(service: &str, account: &str, secret: &str) -> Result<()> {
    let entry = keyring::Entry::new(service, account)
        .with_context(|| format!("create keyring entry for service {service}"))?;
    entry
        .set_password(secret)
        .with_context(|| format!("store {service} in OS keychain"))
}

#[cfg(test)]
type TestKey = (String, String);

#[cfg(test)]
fn test_keyring_store() -> &'static Mutex<std::collections::HashMap<TestKey, String>> {
    static STORE: OnceLock<Mutex<std::collections::HashMap<TestKey, String>>> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

#[cfg(test)]
fn read_keychain_secret(service: &str, account: &str) -> Result<Option<String>> {
    let store = test_keyring_store().lock().unwrap();
    Ok(store
        .get(&(service.to_string(), account.to_string()))
        .cloned())
}

#[cfg(test)]
fn write_keychain_secret(service: &str, account: &str, secret: &str) -> Result<()> {
    let mut store = test_keyring_store().lock().unwrap();
    store.insert(
        (service.to_string(), account.to_string()),
        secret.to_string(),
    );
    Ok(())
}

#[cfg(test)]
pub(crate) fn test_env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

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
fn check_file_permissions(_path: &Path) {}

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
fn set_file_permissions_600(_path: &Path) {}

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
    eprintln!("Then run `shaon auth` to store your credentials in the OS keychain.");
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
    use std::time::{SystemTime, UNIX_EPOCH};

    struct EnvGuard {
        home: Option<OsString>,
        xdg_config_home: Option<OsString>,
        shaon_password: Option<OsString>,
        shaon_master_key: Option<OsString>,
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            restore_env("HOME", self.home.as_deref());
            restore_env("XDG_CONFIG_HOME", self.xdg_config_home.as_deref());
            restore_env("SHAON_PASSWORD", self.shaon_password.as_deref());
            restore_env("SHAON_MASTER_KEY", self.shaon_master_key.as_deref());
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
            shaon_master_key: std::env::var_os("SHAON_MASTER_KEY"),
        }
    }

    fn unique_config(tag: &str) -> Config {
        static NEXT_ID: AtomicUsize = AtomicUsize::new(1);
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);

        Config {
            subdomain: format!("{tag}-{}-{id}", std::process::id()),
            username: format!("user-{id}"),
            password: None,
            payslip_folder: None,
            payslip_format: None,
        }
    }

    fn clear_test_keyring_store() {
        test_keyring_store().lock().unwrap().clear();
    }

    fn temp_home_dir(tag: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "shaon-config-test-{tag}-{}-{unique}",
            std::process::id()
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
    fn config_deserializes_with_plaintext_password() {
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
        clear_test_keyring_store();

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
    fn store_credentials_write_then_read_roundtrip() {
        let _env_guard = test_env_lock().lock().unwrap();
        let _saved_env = preserve_env();
        clear_test_keyring_store();
        let config = unique_config("roundtrip");
        let local_master_key = [0x5a; LOCAL_MASTER_KEY_LEN];

        config
            .store_credentials("correct horse battery staple", &local_master_key)
            .unwrap();

        let password = config.get_password().unwrap();
        assert_eq!(password.expose_secret(), "correct horse battery staple");
        assert_eq!(config.get_local_master_key().unwrap(), local_master_key);

        let stored = read_keychain_secret(
            KEYRING_SERVICE,
            &keyring_account(&config.subdomain, &config.username),
        )
        .unwrap()
        .unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&stored).unwrap();
        assert_eq!(parsed["v"], KEYRING_SCHEMA_VERSION);
        assert_eq!(
            parsed["local_master_key"],
            encode_local_master_key(&local_master_key)
        );
    }

    #[test]
    fn env_var_master_key_overrides_keychain() {
        let _env_guard = test_env_lock().lock().unwrap();
        let _saved_env = preserve_env();
        clear_test_keyring_store();
        let config = unique_config("env-master-key");

        config
            .store_credentials("stored-password", &[0x11; LOCAL_MASTER_KEY_LEN])
            .unwrap();

        let override_key = [0x22; LOCAL_MASTER_KEY_LEN];
        std::env::set_var("SHAON_MASTER_KEY", encode_local_master_key(&override_key));

        assert_eq!(config.get_local_master_key().unwrap(), override_key);
    }

    #[test]
    fn fresh_auth_wrong_password_does_not_persist_before_commit() {
        let _env_guard = test_env_lock().lock().unwrap();
        let _saved_env = preserve_env();
        clear_test_keyring_store();

        let config = unique_config("fresh-auth");
        let pending = config.prepare_stored_credentials("wrong-password".to_string());

        assert_eq!(
            pending
                .login_config()
                .get_password()
                .unwrap()
                .expose_secret(),
            "wrong-password"
        );

        drop(pending);

        assert!(config.load_stored_credentials().unwrap().is_none());
        assert!(config.get_password().is_err());
    }

    #[test]
    fn migrate_wrong_password_keeps_plaintext_fallback_until_commit() {
        let _env_guard = test_env_lock().lock().unwrap();
        let _saved_env = preserve_env();
        clear_test_keyring_store();

        let config = Config {
            subdomain: format!("acme-{}", std::process::id()),
            username: "12345".to_string(),
            password: Some("wrong-password".to_string()),
            payslip_folder: None,
            payslip_format: None,
        };
        let pending = config.prepare_migration().expect("prepare migration");

        assert_eq!(
            pending
                .login_config()
                .get_password()
                .unwrap()
                .expose_secret(),
            "wrong-password"
        );

        drop(pending);

        assert_eq!(config.password.as_deref(), Some("wrong-password"));
        assert!(config.load_stored_credentials().unwrap().is_none());
    }

    #[test]
    fn migration_commit_rewrites_config_without_password() {
        let _env_guard = test_env_lock().lock().unwrap();
        let _saved_env = preserve_env();
        clear_test_keyring_store();

        let home = temp_home_dir("migration-commit");
        let shaon_dir = home.join(".shaon");
        fs::create_dir_all(&shaon_dir).expect("create config dir");
        std::env::set_var("HOME", &home);

        let config = Config {
            subdomain: "acme".to_string(),
            username: "12345".to_string(),
            password: Some("plaintext-password".to_string()),
            payslip_folder: None,
            payslip_format: None,
        };

        let persisted = config
            .prepare_migration()
            .expect("prepare migration")
            .commit()
            .expect("commit migration");

        assert!(persisted.password.is_none());
        assert_eq!(
            persisted.get_password().unwrap().expose_secret(),
            "plaintext-password"
        );

        let saved = fs::read_to_string(config_path()).expect("read saved config");
        assert!(!saved.contains("password"));
        assert!(!saved.contains("plaintext-password"));
    }

    #[test]
    fn malformed_bundled_record_missing_local_master_key_fails_cleanly() {
        let _env_guard = test_env_lock().lock().unwrap();
        let _saved_env = preserve_env();
        clear_test_keyring_store();

        let config = unique_config("malformed-bundle");
        write_keychain_secret(
            KEYRING_SERVICE,
            &keyring_account(&config.subdomain, &config.username),
            r#"{"v":1,"password":"x"}"#,
        )
        .expect("write malformed keychain record");

        let err = config
            .get_local_master_key()
            .expect_err("missing local_master_key should fail");

        assert!(err.to_string().contains("parse shaon-cli keychain record"));
    }
}
