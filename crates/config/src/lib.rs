#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GittreeConfig {
    pub relay_bind: String,
}

const DEFAULT_RELAY_BIND: &str = "0.0.0.0:8080";
const DEFAULT_ADMISSION_BIND: &str = "127.0.0.1:8081";
const DEFAULT_STATE_BIND: &str = "127.0.0.1:8082";
const DEFAULT_COORDINATOR_BIND: &str = "127.0.0.1:8083";
const DEFAULT_SYNC_BIND: &str = "127.0.0.1:8084";
const DEFAULT_GIT_HTTP_BIND: &str = "127.0.0.1:8085";
const ENV_RELAY_BIND: &str = "GITTREE_RELAY_BIND";
const ENV_RELAY_URLS: &str = "GITTREE_RELAY_URLS";
const ENV_RELAY_COMPAT_MODE: &str = "GITTREE_RELAY_COMPAT_MODE";
const ENV_RELAY_PROBE_ACTIVE: &str = "GITTREE_RELAY_PROBE_ACTIVE";
const ENV_RELAY_PROBE_TIMEOUT_SECS: &str = "GITTREE_RELAY_PROBE_TIMEOUT_SECS";
const ENV_RELAY_PROBE_SECRET_KEY: &str = "GITTREE_RELAY_PROBE_SECRET_KEY";
const ENV_ADMISSION_BIND: &str = "GITTREE_ADMISSION_BIND";
const ENV_STATE_BIND: &str = "GITTREE_STATE_BIND";
const ENV_COORDINATOR_BIND: &str = "GITTREE_COORDINATOR_BIND";
const ENV_SYNC_BIND: &str = "GITTREE_SYNC_BIND";
const ENV_GIT_HTTP_BIND: &str = "GITTREE_GIT_HTTP_BIND";

const DEFAULT_RELAY_COMPAT_MODE: RelayCompatibilityMode = RelayCompatibilityMode::Strict;
const DEFAULT_RELAY_PROBE_TIMEOUT_SECS: u64 = 5;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceConfig {
    pub bind: String,
}

impl ServiceConfig {
    pub fn new(bind: impl Into<String>) -> Self {
        Self { bind: bind.into() }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServicesConfig {
    pub relay: ServiceConfig,
    pub admission: ServiceConfig,
    pub state: ServiceConfig,
    pub coordinator: ServiceConfig,
    pub sync: ServiceConfig,
    pub git_http: ServiceConfig,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayTargetsConfig {
    pub relay_urls: Vec<String>,
}

impl RelayTargetsConfig {
    pub fn from_env() -> Self {
        let relay_urls = parse_relay_urls(std::env::var(ENV_RELAY_URLS).unwrap_or_default());
        Self { relay_urls }
    }

    pub fn from_env_validated() -> Result<Self, ConfigError> {
        let config = Self::from_env();
        config.validate()?;
        Ok(config)
    }

    pub fn from_toml_str(input: &str) -> Result<Self, ConfigError> {
        let parsed: TomlRelayTargets = toml::from_str(input)
            .map_err(|source| ConfigError::TomlParse { path: None, source })?;
        let relay_urls = parsed.relay_urls.unwrap_or_default();
        let config = Self { relay_urls };
        config.validate()?;
        Ok(config)
    }

    pub fn from_toml_file(path: impl AsRef<std::path::Path>) -> Result<Self, ConfigError> {
        let path = path.as_ref();
        let contents = std::fs::read_to_string(path).map_err(|source| ConfigError::ReadConfig {
            path: path.to_path_buf(),
            source,
        })?;
        Self::from_toml_str(&contents).map_err(|err| err.with_path(path))
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        for url in &self.relay_urls {
            validate_relay_url(url)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelayCompatibilityMode {
    Strict,
    Warn,
    Allow,
}

impl RelayCompatibilityMode {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "strict" => Some(Self::Strict),
            "warn" | "warning" => Some(Self::Warn),
            "allow" => Some(Self::Allow),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            RelayCompatibilityMode::Strict => "strict",
            RelayCompatibilityMode::Warn => "warn",
            RelayCompatibilityMode::Allow => "allow",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayCompatibilityConfig {
    pub mode: RelayCompatibilityMode,
}

impl Default for RelayCompatibilityConfig {
    fn default() -> Self {
        Self {
            mode: DEFAULT_RELAY_COMPAT_MODE,
        }
    }
}

impl RelayCompatibilityConfig {
    pub fn from_env() -> Result<Self, ConfigError> {
        let mode = env_or_default(ENV_RELAY_COMPAT_MODE, DEFAULT_RELAY_COMPAT_MODE.as_str());
        let mode = RelayCompatibilityMode::parse(&mode)
            .ok_or_else(|| ConfigError::InvalidRelayCompatibilityMode(mode))?;
        Ok(Self { mode })
    }

    pub fn from_toml_str(input: &str) -> Result<Self, ConfigError> {
        let parsed: TomlRelayCompatibilityRoot = toml::from_str(input)
            .map_err(|source| ConfigError::TomlParse { path: None, source })?;
        parsed.into_config()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayProbeConfig {
    pub active: bool,
    pub timeout_secs: u64,
    pub secret_key: Option<String>,
}

impl Default for RelayProbeConfig {
    fn default() -> Self {
        Self {
            active: false,
            timeout_secs: DEFAULT_RELAY_PROBE_TIMEOUT_SECS,
            secret_key: None,
        }
    }
}

impl RelayProbeConfig {
    pub fn from_env() -> Result<Self, ConfigError> {
        let active = env_bool(ENV_RELAY_PROBE_ACTIVE)?.unwrap_or(false);
        let timeout_secs =
            env_u64(ENV_RELAY_PROBE_TIMEOUT_SECS)?.unwrap_or(DEFAULT_RELAY_PROBE_TIMEOUT_SECS);
        let secret_key = env_optional_string(ENV_RELAY_PROBE_SECRET_KEY);
        let config = Self {
            active,
            timeout_secs,
            secret_key,
        };
        config.validate()?;
        Ok(config)
    }

    pub fn from_toml_str(input: &str) -> Result<Self, ConfigError> {
        let parsed: TomlRelayProbeRoot = toml::from_str(input)
            .map_err(|source| ConfigError::TomlParse { path: None, source })?;
        let config = parsed.into_config();
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.timeout_secs == 0 {
            return Err(ConfigError::InvalidRelayProbeConfig {
                field: "relay_probe.timeout_secs",
                value: "0".to_string(),
            });
        }
        if let Some(secret) = &self.secret_key {
            if !is_hex_len(secret, 64) {
                return Err(ConfigError::InvalidRelayProbeConfig {
                    field: "relay_probe.secret_key",
                    value: secret.clone(),
                });
            }
        }
        Ok(())
    }
}

impl Default for ServicesConfig {
    fn default() -> Self {
        Self {
            relay: ServiceConfig::new(DEFAULT_RELAY_BIND),
            admission: ServiceConfig::new(DEFAULT_ADMISSION_BIND),
            state: ServiceConfig::new(DEFAULT_STATE_BIND),
            coordinator: ServiceConfig::new(DEFAULT_COORDINATOR_BIND),
            sync: ServiceConfig::new(DEFAULT_SYNC_BIND),
            git_http: ServiceConfig::new(DEFAULT_GIT_HTTP_BIND),
        }
    }
}

impl ServicesConfig {
    pub fn from_env() -> Self {
        Self {
            relay: ServiceConfig::new(env_or_default(ENV_RELAY_BIND, DEFAULT_RELAY_BIND)),
            admission: ServiceConfig::new(env_or_default(
                ENV_ADMISSION_BIND,
                DEFAULT_ADMISSION_BIND,
            )),
            state: ServiceConfig::new(env_or_default(ENV_STATE_BIND, DEFAULT_STATE_BIND)),
            coordinator: ServiceConfig::new(env_or_default(
                ENV_COORDINATOR_BIND,
                DEFAULT_COORDINATOR_BIND,
            )),
            sync: ServiceConfig::new(env_or_default(ENV_SYNC_BIND, DEFAULT_SYNC_BIND)),
            git_http: ServiceConfig::new(env_or_default(ENV_GIT_HTTP_BIND, DEFAULT_GIT_HTTP_BIND)),
        }
    }

    pub fn from_toml_str(input: &str) -> Result<Self, ConfigError> {
        let parsed: TomlServicesRoot = toml::from_str(input)
            .map_err(|source| ConfigError::TomlParse { path: None, source })?;
        Ok(parsed.into_services())
    }

    pub fn from_toml_file(path: impl AsRef<std::path::Path>) -> Result<Self, ConfigError> {
        let path = path.as_ref();
        let contents = std::fs::read_to_string(path).map_err(|source| ConfigError::ReadConfig {
            path: path.to_path_buf(),
            source,
        })?;
        Self::from_toml_str(&contents).map_err(|err| err.with_path(path))
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        validate_service_bind("relay", &self.relay.bind)?;
        validate_service_bind("admission", &self.admission.bind)?;
        validate_service_bind("state", &self.state.bind)?;
        validate_service_bind("coordinator", &self.coordinator.bind)?;
        validate_service_bind("sync", &self.sync.bind)?;
        validate_service_bind("git_http", &self.git_http.bind)?;
        Ok(())
    }

    pub fn from_env_validated() -> Result<Self, ConfigError> {
        let config = Self::from_env();
        config.validate()?;
        Ok(config)
    }

    pub fn from_toml_file_validated(
        path: impl AsRef<std::path::Path>,
    ) -> Result<Self, ConfigError> {
        let config = Self::from_toml_file(path)?;
        config.validate()?;
        Ok(config)
    }
}

fn env_or_default(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

fn env_optional_string(key: &str) -> Option<String> {
    match std::env::var(key) {
        Ok(value) if value.trim().is_empty() => None,
        Ok(value) => Some(value),
        Err(_) => None,
    }
}

fn env_bool(key: &'static str) -> Result<Option<bool>, ConfigError> {
    match std::env::var(key) {
        Ok(value) => {
            if value.trim().is_empty() {
                return Ok(None);
            }
            parse_bool(&value)
                .map(Some)
                .ok_or_else(|| ConfigError::InvalidRelayProbeConfig { field: key, value })
        }
        Err(_) => Ok(None),
    }
}

fn env_u64(key: &'static str) -> Result<Option<u64>, ConfigError> {
    match std::env::var(key) {
        Ok(value) => {
            if value.trim().is_empty() {
                return Ok(None);
            }
            value.parse::<u64>().map(Some).map_err(|_| {
                ConfigError::InvalidRelayProbeConfig {
                    field: key,
                    value,
                }
            })
        }
        Err(_) => Ok(None),
    }
}

fn parse_bool(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" => Some(true),
        "false" | "0" | "no" => Some(false),
        _ => None,
    }
}

fn is_hex_len(value: &str, len: usize) -> bool {
    value.len() == len && value.chars().all(|c| c.is_ascii_hexdigit())
}

fn validate_service_bind(service: &'static str, value: &str) -> Result<(), ConfigError> {
    value
        .parse::<std::net::SocketAddr>()
        .map_err(|_| ConfigError::InvalidServiceBind {
            service,
            value: value.to_string(),
        })?;
    Ok(())
}

fn parse_relay_urls(raw: String) -> Vec<String> {
    raw.split(',')
        .map(|entry| entry.trim().to_string())
        .filter(|entry| !entry.is_empty())
        .collect()
}

fn validate_relay_url(value: &str) -> Result<(), ConfigError> {
    let parsed = url::Url::parse(value)
        .map_err(|_| ConfigError::InvalidRelayUrl(value.to_string()))?;
    match parsed.scheme() {
        "ws" | "wss" | "http" | "https" => Ok(()),
        _ => Err(ConfigError::InvalidRelayUrl(value.to_string())),
    }
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct TomlConfig {
    relay_bind: Option<String>,
}

impl TomlConfig {
    fn into_config(self) -> GittreeConfig {
        GittreeConfig {
            relay_bind: self
                .relay_bind
                .unwrap_or_else(|| DEFAULT_RELAY_BIND.to_string()),
        }
    }
}

impl TomlServicesRoot {
    fn into_services(self) -> ServicesConfig {
        let services = self.services.unwrap_or_default();
        ServicesConfig {
            relay: ServiceConfig::new(bind_or_default(services.relay, DEFAULT_RELAY_BIND)),
            admission: ServiceConfig::new(bind_or_default(
                services.admission,
                DEFAULT_ADMISSION_BIND,
            )),
            state: ServiceConfig::new(bind_or_default(services.state, DEFAULT_STATE_BIND)),
            coordinator: ServiceConfig::new(bind_or_default(
                services.coordinator,
                DEFAULT_COORDINATOR_BIND,
            )),
            sync: ServiceConfig::new(bind_or_default(services.sync, DEFAULT_SYNC_BIND)),
            git_http: ServiceConfig::new(bind_or_default(services.git_http, DEFAULT_GIT_HTTP_BIND)),
        }
    }
}

fn bind_or_default(config: Option<TomlServiceConfig>, default: &str) -> String {
    config
        .and_then(|entry| entry.bind)
        .unwrap_or_else(|| default.to_string())
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct TomlServicesRoot {
    services: Option<TomlServicesConfig>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct TomlRelayTargets {
    relay_urls: Option<Vec<String>>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct TomlRelayCompatibilityRoot {
    relay_compatibility: Option<TomlRelayCompatibility>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct TomlRelayCompatibility {
    mode: Option<String>,
}

impl TomlRelayCompatibilityRoot {
    fn into_config(self) -> Result<RelayCompatibilityConfig, ConfigError> {
        let mode = self
            .relay_compatibility
            .and_then(|value| value.mode)
            .unwrap_or_else(|| DEFAULT_RELAY_COMPAT_MODE.as_str().to_string());
        let mode = RelayCompatibilityMode::parse(&mode)
            .ok_or_else(|| ConfigError::InvalidRelayCompatibilityMode(mode))?;
        Ok(RelayCompatibilityConfig { mode })
    }
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct TomlRelayProbeRoot {
    relay_probe: Option<TomlRelayProbeConfig>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct TomlRelayProbeConfig {
    active: Option<bool>,
    timeout_secs: Option<u64>,
    secret_key: Option<String>,
}

impl TomlRelayProbeRoot {
    fn into_config(self) -> RelayProbeConfig {
        let config = self.relay_probe.unwrap_or(TomlRelayProbeConfig {
            active: None,
            timeout_secs: None,
            secret_key: None,
        });
        RelayProbeConfig {
            active: config.active.unwrap_or(false),
            timeout_secs: config
                .timeout_secs
                .unwrap_or(DEFAULT_RELAY_PROBE_TIMEOUT_SECS),
            secret_key: config.secret_key,
        }
    }
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct TomlServicesConfig {
    relay: Option<TomlServiceConfig>,
    admission: Option<TomlServiceConfig>,
    state: Option<TomlServiceConfig>,
    coordinator: Option<TomlServiceConfig>,
    sync: Option<TomlServiceConfig>,
    git_http: Option<TomlServiceConfig>,
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct TomlServiceConfig {
    bind: Option<String>,
}

#[derive(Debug)]
pub enum ConfigError {
    InvalidRelayBind(String),
    InvalidRelayUrl(String),
    InvalidRelayCompatibilityMode(String),
    InvalidRelayProbeConfig { field: &'static str, value: String },
    InvalidServiceBind {
        service: &'static str,
        value: String,
    },
    ReadConfig {
        path: std::path::PathBuf,
        source: std::io::Error,
    },
    TomlParse {
        path: Option<std::path::PathBuf>,
        source: toml::de::Error,
    },
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigError::InvalidRelayBind(value) => {
                write!(f, "invalid relay bind address: {value}")
            }
            ConfigError::InvalidRelayUrl(value) => {
                write!(f, "invalid relay url: {value}")
            }
            ConfigError::InvalidRelayCompatibilityMode(value) => {
                write!(f, "invalid relay compatibility mode: {value}")
            }
            ConfigError::InvalidRelayProbeConfig { field, value } => {
                write!(f, "invalid relay probe config {field}: {value}")
            }
            ConfigError::InvalidServiceBind { service, value } => {
                write!(f, "invalid {service} bind address: {value}")
            }
            ConfigError::ReadConfig { path, source } => {
                write!(f, "failed to read config file {}: {source}", path.display())
            }
            ConfigError::TomlParse {
                path: Some(path),
                source,
            } => write!(
                f,
                "failed to parse config file {}: {source}",
                path.display()
            ),
            ConfigError::TomlParse { path: None, source } => {
                write!(f, "failed to parse config: {source}")
            }
        }
    }
}

impl std::error::Error for ConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ConfigError::InvalidRelayBind(_) => None,
            ConfigError::InvalidRelayUrl(_) => None,
            ConfigError::InvalidRelayCompatibilityMode(_) => None,
            ConfigError::InvalidRelayProbeConfig { .. } => None,
            ConfigError::InvalidServiceBind { .. } => None,
            ConfigError::ReadConfig { source, .. } => Some(source),
            ConfigError::TomlParse { source, .. } => Some(source),
        }
    }
}

impl ConfigError {
    fn with_path(self, path: &std::path::Path) -> Self {
        match self {
            ConfigError::TomlParse { path: None, source } => ConfigError::TomlParse {
                path: Some(path.to_path_buf()),
                source,
            },
            other => other,
        }
    }
}

impl Default for GittreeConfig {
    fn default() -> Self {
        Self {
            relay_bind: DEFAULT_RELAY_BIND.to_string(),
        }
    }
}

impl GittreeConfig {
    pub fn from_env() -> Self {
        let relay_bind =
            std::env::var(ENV_RELAY_BIND).unwrap_or_else(|_| DEFAULT_RELAY_BIND.to_string());

        Self { relay_bind }
    }

    pub fn from_env_with_keys(relay_bind_key: &str) -> Self {
        let relay_bind =
            std::env::var(relay_bind_key).unwrap_or_else(|_| DEFAULT_RELAY_BIND.to_string());

        Self { relay_bind }
    }

    pub fn from_toml_str(input: &str) -> Result<Self, ConfigError> {
        let parsed: TomlConfig = toml::from_str(input)
            .map_err(|source| ConfigError::TomlParse { path: None, source })?;

        Ok(parsed.into_config())
    }

    pub fn from_toml_file(path: impl AsRef<std::path::Path>) -> Result<Self, ConfigError> {
        let path = path.as_ref();
        let contents = std::fs::read_to_string(path).map_err(|source| ConfigError::ReadConfig {
            path: path.to_path_buf(),
            source,
        })?;

        Self::from_toml_str(&contents).map_err(|err| err.with_path(path))
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        self.relay_bind
            .parse::<std::net::SocketAddr>()
            .map_err(|_| ConfigError::InvalidRelayBind(self.relay_bind.clone()))?;

        Ok(())
    }

    pub fn relay_bind_addr(&self) -> Result<std::net::SocketAddr, ConfigError> {
        self.relay_bind
            .parse::<std::net::SocketAddr>()
            .map_err(|_| ConfigError::InvalidRelayBind(self.relay_bind.clone()))
    }

    pub fn relay_bind_ip(&self) -> Result<std::net::IpAddr, ConfigError> {
        self.relay_bind_addr().map(|addr| addr.ip())
    }

    pub fn relay_bind_port(&self) -> Result<u16, ConfigError> {
        self.relay_bind_addr().map(|addr| addr.port())
    }

    pub fn from_env_validated() -> Result<Self, ConfigError> {
        let config = Self::from_env();
        config.validate()?;
        Ok(config)
    }

    pub fn from_env_validated_with_keys(relay_bind_key: &str) -> Result<Self, ConfigError> {
        let config = Self::from_env_with_keys(relay_bind_key);
        config.validate()?;
        Ok(config)
    }

    pub fn from_toml_file_validated(
        path: impl AsRef<std::path::Path>,
    ) -> Result<Self, ConfigError> {
        let config = Self::from_toml_file(path)?;
        config.validate()?;
        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::ConfigError;
    use super::GittreeConfig;
    use super::RelayCompatibilityConfig;
    use super::RelayCompatibilityMode;
    use super::RelayProbeConfig;
    use super::RelayTargetsConfig;
    use super::ServicesConfig;
    use crate::DEFAULT_ADMISSION_BIND;
    use crate::DEFAULT_COORDINATOR_BIND;
    use crate::DEFAULT_GIT_HTTP_BIND;
    use crate::DEFAULT_RELAY_BIND;
    use crate::DEFAULT_STATE_BIND;
    use crate::DEFAULT_SYNC_BIND;
    use crate::ENV_ADMISSION_BIND;
    use crate::ENV_COORDINATOR_BIND;
    use crate::ENV_GIT_HTTP_BIND;
    use crate::ENV_RELAY_BIND;
    use crate::ENV_RELAY_COMPAT_MODE;
    use crate::ENV_RELAY_PROBE_ACTIVE;
    use crate::ENV_RELAY_PROBE_SECRET_KEY;
    use crate::ENV_RELAY_PROBE_TIMEOUT_SECS;
    use crate::ENV_RELAY_URLS;
    use crate::ENV_STATE_BIND;
    use crate::ENV_SYNC_BIND;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());
    const ENV_RELAY_BIND_TEST1: &str = "GITTREE_RELAY_BIND_TEST1";
    const ENV_RELAY_BIND_TEST2: &str = "GITTREE_RELAY_BIND_TEST2";

    fn write_temp_config(contents: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        let mut path = std::env::temp_dir();
        path.push(format!(
            "gittree-config-{nanos}-{}.toml",
            std::process::id()
        ));
        std::fs::write(&path, contents).expect("write config file");
        path
    }

    fn with_env_var<F: FnOnce()>(key: &str, value: &str, f: F) {
        let previous = std::env::var_os(key);
        // SAFETY: tests run single-threaded in this crate; we restore the previous value after.
        unsafe {
            std::env::set_var(key, value);
        }
        f();
        match previous {
            Some(old) => unsafe {
                std::env::set_var(key, old);
            },
            None => unsafe {
                std::env::remove_var(key);
            },
        }
    }

    #[test]
    fn default_config_has_relay_bind() {
        let config = GittreeConfig::default();
        assert_eq!(config.relay_bind, DEFAULT_RELAY_BIND);
    }

    #[test]
    fn env_config_overrides_relay_bind() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var(ENV_RELAY_BIND, "127.0.0.1:9000", || {
            let config = GittreeConfig::from_env();
            assert_eq!(config.relay_bind, "127.0.0.1:9000");
        });
    }

    #[test]
    fn env_config_falls_back_to_default() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        // SAFETY: this test controls the env var for its duration only.
        unsafe {
            std::env::remove_var(ENV_RELAY_BIND);
        }
        let config = GittreeConfig::from_env();
        assert_eq!(config.relay_bind, DEFAULT_RELAY_BIND);
    }

    #[test]
    fn relay_targets_env_parses_list() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var(ENV_RELAY_URLS, "wss://relay.one, wss://relay.two ,", || {
            let config = RelayTargetsConfig::from_env_validated().expect("relay targets");
            assert_eq!(
                config.relay_urls,
                vec!["wss://relay.one".to_string(), "wss://relay.two".to_string()]
            );
        });
    }

    #[test]
    fn relay_targets_env_rejects_invalid_url() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var(ENV_RELAY_URLS, "ftp://relay.example", || {
            let err = RelayTargetsConfig::from_env_validated().unwrap_err();
            assert!(matches!(
                err,
                ConfigError::InvalidRelayUrl(value) if value == "ftp://relay.example"
            ));
        });
    }

    #[test]
    fn relay_targets_toml_parses_urls() {
        let config =
            RelayTargetsConfig::from_toml_str("relay_urls = [\"wss://relay.example\"]")
                .expect("relay targets");
        assert_eq!(config.relay_urls, vec!["wss://relay.example".to_string()]);
    }

    #[test]
    fn relay_compat_mode_env_parses() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var(ENV_RELAY_COMPAT_MODE, "warn", || {
            let config = RelayCompatibilityConfig::from_env().expect("compat config");
            assert_eq!(config.mode, RelayCompatibilityMode::Warn);
        });
    }

    #[test]
    fn relay_compat_mode_env_rejects_invalid() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var(ENV_RELAY_COMPAT_MODE, "nope", || {
            let err = RelayCompatibilityConfig::from_env().unwrap_err();
            assert!(matches!(
                err,
                ConfigError::InvalidRelayCompatibilityMode(value) if value == "nope"
            ));
        });
    }

    #[test]
    fn relay_probe_env_parses() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var(ENV_RELAY_PROBE_ACTIVE, "true", || {
            with_env_var(ENV_RELAY_PROBE_TIMEOUT_SECS, "7", || {
                with_env_var(ENV_RELAY_PROBE_SECRET_KEY, &"11".repeat(32), || {
                    let config = RelayProbeConfig::from_env().expect("probe config");
                    assert!(config.active);
                    assert_eq!(config.timeout_secs, 7);
                    assert_eq!(config.secret_key, Some("11".repeat(32)));
                });
            });
        });
    }

    #[test]
    fn relay_probe_env_rejects_bad_secret() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var(ENV_RELAY_PROBE_SECRET_KEY, "bad", || {
            let err = RelayProbeConfig::from_env().unwrap_err();
            assert!(matches!(
                err,
                ConfigError::InvalidRelayProbeConfig { field, .. } if field == "relay_probe.secret_key"
            ));
        });
    }

    #[test]
    fn relay_probe_toml_parses() {
        let config = RelayProbeConfig::from_toml_str(
            r#"[relay_probe]
active = true
timeout_secs = 9
secret_key = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
"#,
        )
        .expect("probe config");
        assert!(config.active);
        assert_eq!(config.timeout_secs, 9);
        assert_eq!(
            config.secret_key,
            Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string())
        );
    }

    #[test]
    fn relay_probe_toml_rejects_bad_secret() {
        let err = RelayProbeConfig::from_toml_str(
            r#"[relay_probe]
active = true
timeout_secs = 9
secret_key = "22"
"#,
        )
        .unwrap_err();
        assert!(matches!(
            err,
            ConfigError::InvalidRelayProbeConfig { field, .. } if field == "relay_probe.secret_key"
        ));
    }

    #[test]
    fn validate_accepts_valid_relay_bind() {
        let config = GittreeConfig {
            relay_bind: "127.0.0.1:9000".to_string(),
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn validate_rejects_invalid_relay_bind() {
        let config = GittreeConfig {
            relay_bind: "not-an-addr".to_string(),
        };
        assert!(matches!(
            config.validate(),
            Err(ConfigError::InvalidRelayBind(value)) if value == "not-an-addr"
        ));
    }

    #[test]
    fn relay_bind_addr_parses_socket_addr() {
        let config = GittreeConfig {
            relay_bind: "127.0.0.1:9000".to_string(),
        };
        let addr = config.relay_bind_addr().expect("valid socket addr");
        assert_eq!(addr, "127.0.0.1:9000".parse().expect("parse addr"));
    }

    #[test]
    fn relay_bind_addr_reports_invalid_bind() {
        let config = GittreeConfig {
            relay_bind: "bad".to_string(),
        };
        assert!(matches!(
            config.relay_bind_addr(),
            Err(ConfigError::InvalidRelayBind(value)) if value == "bad"
        ));
    }

    #[test]
    fn relay_bind_ip_returns_ip() {
        let config = GittreeConfig {
            relay_bind: "127.0.0.1:9100".to_string(),
        };
        let ip = config.relay_bind_ip().expect("valid ip");
        assert_eq!(
            ip,
            "127.0.0.1".parse::<std::net::IpAddr>().expect("parse ip")
        );
    }

    #[test]
    fn relay_bind_port_returns_port() {
        let config = GittreeConfig {
            relay_bind: "127.0.0.1:9100".to_string(),
        };
        let port = config.relay_bind_port().expect("valid port");
        assert_eq!(port, 9100);
    }

    #[test]
    fn from_env_validated_returns_error_for_invalid_bind() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var(ENV_RELAY_BIND, "bad:addr", || {
            let result = GittreeConfig::from_env_validated();
            assert!(matches!(
                result,
                Err(ConfigError::InvalidRelayBind(value)) if value == "bad:addr"
            ));
        });
    }

    #[test]
    fn from_env_validated_accepts_valid_bind() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var(ENV_RELAY_BIND, "0.0.0.0:7000", || {
            let config = GittreeConfig::from_env_validated();
            assert!(matches!(
                config,
                Ok(GittreeConfig {
                    relay_bind,
                }) if relay_bind == "0.0.0.0:7000"
            ));
        });
    }

    #[test]
    fn from_env_with_keys_reads_custom_key() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var(ENV_RELAY_BIND_TEST1, "127.0.0.1:8081", || {
            let config = GittreeConfig::from_env_with_keys(ENV_RELAY_BIND_TEST1);
            assert_eq!(config.relay_bind, "127.0.0.1:8081");
        });
    }

    #[test]
    fn from_env_validated_with_keys_accepts_valid_bind() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var(ENV_RELAY_BIND_TEST2, "127.0.0.1:8082", || {
            let config = GittreeConfig::from_env_validated_with_keys(ENV_RELAY_BIND_TEST2);
            assert!(matches!(
                config,
                Ok(GittreeConfig { relay_bind }) if relay_bind == "127.0.0.1:8082"
            ));
        });
    }

    #[test]
    fn toml_str_parses_valid_config() {
        let config =
            GittreeConfig::from_toml_str("relay_bind = \"127.0.0.1:9999\"").expect("parse config");
        assert_eq!(config.relay_bind, "127.0.0.1:9999");
    }

    #[test]
    fn toml_str_rejects_invalid_config() {
        let result = GittreeConfig::from_toml_str("relay_bind = [");
        assert!(matches!(result, Err(ConfigError::TomlParse { .. })));
    }

    #[test]
    fn toml_file_reads_valid_config() {
        let path = write_temp_config("relay_bind = \"127.0.0.1:9998\"");
        let config = GittreeConfig::from_toml_file(&path).expect("read config");
        assert_eq!(config.relay_bind, "127.0.0.1:9998");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn toml_file_reports_missing_file() {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "gittree-config-missing-{}.toml",
            std::process::id()
        ));
        let result = GittreeConfig::from_toml_file(&path);
        assert!(matches!(result, Err(ConfigError::ReadConfig { .. })));
    }

    #[test]
    fn toml_file_validated_rejects_invalid_bind() {
        let path = write_temp_config("relay_bind = \"invalid\"");
        let result = GittreeConfig::from_toml_file_validated(&path);
        assert!(matches!(
            result,
            Err(ConfigError::InvalidRelayBind(value)) if value == "invalid"
        ));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn services_config_has_expected_defaults() {
        let services = ServicesConfig::default();
        assert_eq!(services.relay.bind, DEFAULT_RELAY_BIND);
        assert_eq!(services.admission.bind, DEFAULT_ADMISSION_BIND);
        assert_eq!(services.state.bind, DEFAULT_STATE_BIND);
        assert_eq!(services.coordinator.bind, DEFAULT_COORDINATOR_BIND);
        assert_eq!(services.sync.bind, DEFAULT_SYNC_BIND);
        assert_eq!(services.git_http.bind, DEFAULT_GIT_HTTP_BIND);
    }

    #[test]
    fn services_config_from_env_uses_defaults_when_unset() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        unsafe {
            std::env::remove_var(ENV_RELAY_BIND);
            std::env::remove_var(ENV_ADMISSION_BIND);
            std::env::remove_var(ENV_STATE_BIND);
            std::env::remove_var(ENV_COORDINATOR_BIND);
            std::env::remove_var(ENV_SYNC_BIND);
            std::env::remove_var(ENV_GIT_HTTP_BIND);
        }

        let services = ServicesConfig::from_env();
        assert_eq!(services.relay.bind, DEFAULT_RELAY_BIND);
        assert_eq!(services.admission.bind, DEFAULT_ADMISSION_BIND);
        assert_eq!(services.state.bind, DEFAULT_STATE_BIND);
        assert_eq!(services.coordinator.bind, DEFAULT_COORDINATOR_BIND);
        assert_eq!(services.sync.bind, DEFAULT_SYNC_BIND);
        assert_eq!(services.git_http.bind, DEFAULT_GIT_HTTP_BIND);
    }

    #[test]
    fn services_config_from_env_overrides_bind() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var(ENV_ADMISSION_BIND, "127.0.0.1:9091", || {
            let services = ServicesConfig::from_env();
            assert_eq!(services.admission.bind, "127.0.0.1:9091");
        });
    }

    #[test]
    fn services_toml_parses_overrides() {
        let toml = r#"
[services.relay]
bind = "127.0.0.1:9010"

[services.admission]
bind = "127.0.0.1:9011"
"#;
        let services = ServicesConfig::from_toml_str(toml).expect("parse services");
        assert_eq!(services.relay.bind, "127.0.0.1:9010");
        assert_eq!(services.admission.bind, "127.0.0.1:9011");
        assert_eq!(services.state.bind, DEFAULT_STATE_BIND);
        assert_eq!(services.git_http.bind, DEFAULT_GIT_HTTP_BIND);
    }

    #[test]
    fn services_toml_rejects_invalid_config() {
        let result = ServicesConfig::from_toml_str("services = [");
        assert!(matches!(result, Err(ConfigError::TomlParse { .. })));
    }

    #[test]
    fn services_toml_file_reads_valid_config() {
        let toml = r#"
[services.state]
bind = "127.0.0.1:9101"
"#;
        let path = write_temp_config(toml);
        let services = ServicesConfig::from_toml_file(&path).expect("read services");
        assert_eq!(services.state.bind, "127.0.0.1:9101");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn services_config_validate_rejects_invalid_bind() {
        let mut services = ServicesConfig::default();
        services.state.bind = "bad".to_string();
        let err = services.validate().unwrap_err();
        assert!(matches!(
            err,
            ConfigError::InvalidServiceBind {
                service: "state",
                ..
            }
        ));
    }

    #[test]
    fn services_config_from_env_validated_reports_invalid_bind() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var(ENV_STATE_BIND, "bad", || {
            let err = ServicesConfig::from_env_validated().unwrap_err();
            assert!(matches!(
                err,
                ConfigError::InvalidServiceBind {
                    service: "state",
                    ..
                }
            ));
        });
    }

    #[test]
    fn services_toml_file_validated_rejects_invalid_bind() {
        let toml = r#"
[services.coordinator]
bind = "bad"
"#;
        let path = write_temp_config(toml);
        let result = ServicesConfig::from_toml_file_validated(&path);
        assert!(matches!(
            result,
            Err(ConfigError::InvalidServiceBind {
                service: "coordinator",
                ..
            })
        ));
        let _ = std::fs::remove_file(&path);
    }
}
