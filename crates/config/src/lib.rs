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
const ENV_ADMISSION_BIND: &str = "GITTREE_ADMISSION_BIND";
const ENV_STATE_BIND: &str = "GITTREE_STATE_BIND";
const ENV_COORDINATOR_BIND: &str = "GITTREE_COORDINATOR_BIND";
const ENV_SYNC_BIND: &str = "GITTREE_SYNC_BIND";
const ENV_GIT_HTTP_BIND: &str = "GITTREE_GIT_HTTP_BIND";

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

fn validate_service_bind(service: &'static str, value: &str) -> Result<(), ConfigError> {
    value
        .parse::<std::net::SocketAddr>()
        .map_err(|_| ConfigError::InvalidServiceBind {
            service,
            value: value.to_string(),
        })?;
    Ok(())
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
