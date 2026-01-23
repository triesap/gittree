use gittree_config::{ConfigError, ServicesConfig};
use gittree_observability::{ObservabilityError, ObservabilityHandle};
use gittree_storage::StorageConfig;

const ENV_STORAGE_READ_URL: &str = "GITTREE_STORAGE_READ_URL";
const ENV_STORAGE_WRITE_URL: &str = "GITTREE_STORAGE_WRITE_URL";
const ENV_STORAGE_MAX_CONNECTIONS: &str = "GITTREE_STORAGE_MAX_CONNECTIONS";
const ENV_STORAGE_MIN_CONNECTIONS: &str = "GITTREE_STORAGE_MIN_CONNECTIONS";
const ENV_STORAGE_IDLE_TIMEOUT_SECS: &str = "GITTREE_STORAGE_IDLE_TIMEOUT_SECS";
const ENV_STORAGE_MAX_LIFETIME_SECS: &str = "GITTREE_STORAGE_MAX_LIFETIME_SECS";
const ENV_STORAGE_APP_NAME: &str = "GITTREE_STORAGE_APP_NAME";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateConfig {
    pub bind: String,
    pub storage: StorageConfig,
}

impl StateConfig {
    pub fn from_env() -> Result<Self, StateConfigError> {
        let services = ServicesConfig::from_env_validated().map_err(StateConfigError::Config)?;
        let storage = storage_from_env()?;
        Ok(Self {
            bind: services.state.bind,
            storage,
        })
    }
}

#[derive(Debug)]
pub enum StateConfigError {
    Config(ConfigError),
    Storage(StorageConfigError),
}

impl std::fmt::Display for StateConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StateConfigError::Config(err) => write!(f, "state config error: {err}"),
            StateConfigError::Storage(err) => write!(f, "state storage config error: {err}"),
        }
    }
}

impl std::error::Error for StateConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            StateConfigError::Config(err) => Some(err),
            StateConfigError::Storage(err) => Some(err),
        }
    }
}

#[derive(Debug)]
pub enum StorageConfigError {
    MissingEnv(&'static str),
    InvalidEnv { key: &'static str, value: String },
    InvalidConfig(String),
}

impl std::fmt::Display for StorageConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StorageConfigError::MissingEnv(key) => write!(f, "missing env {key}"),
            StorageConfigError::InvalidEnv { key, value } => {
                write!(f, "invalid env {key}: {value}")
            }
            StorageConfigError::InvalidConfig(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for StorageConfigError {}

fn storage_from_env() -> Result<StorageConfig, StateConfigError> {
    let read_connection = std::env::var(ENV_STORAGE_READ_URL).map_err(|_| {
        StateConfigError::Storage(StorageConfigError::MissingEnv(ENV_STORAGE_READ_URL))
    })?;
    let write_connection = std::env::var(ENV_STORAGE_WRITE_URL).ok();
    let max_connections = env_u32(ENV_STORAGE_MAX_CONNECTIONS)?.unwrap_or(10);
    let min_connections = env_u32(ENV_STORAGE_MIN_CONNECTIONS)?.unwrap_or(2);
    let idle_timeout_secs = env_u64(ENV_STORAGE_IDLE_TIMEOUT_SECS)?;
    let max_lifetime_secs = env_u64(ENV_STORAGE_MAX_LIFETIME_SECS)?;
    let application_name = std::env::var(ENV_STORAGE_APP_NAME).ok();

    let config = StorageConfig {
        read_connection,
        write_connection,
        max_connections,
        min_connections,
        idle_timeout_secs,
        max_lifetime_secs,
        application_name,
    };

    config.validate().map_err(|err| {
        StateConfigError::Storage(StorageConfigError::InvalidConfig(err.to_string()))
    })?;

    Ok(config)
}

fn env_u32(key: &'static str) -> Result<Option<u32>, StateConfigError> {
    match std::env::var(key) {
        Ok(value) => value
            .parse::<u32>()
            .map(Some)
            .map_err(|_| StateConfigError::Storage(StorageConfigError::InvalidEnv { key, value })),
        Err(_) => Ok(None),
    }
}

fn env_u64(key: &'static str) -> Result<Option<u64>, StateConfigError> {
    match std::env::var(key) {
        Ok(value) => value
            .parse::<u64>()
            .map(Some)
            .map_err(|_| StateConfigError::Storage(StorageConfigError::InvalidEnv { key, value })),
        Err(_) => Ok(None),
    }
}

#[derive(Debug)]
pub enum StateError {
    Config(StateConfigError),
    Observability(ObservabilityError),
}

impl std::fmt::Display for StateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StateError::Config(err) => write!(f, "state config error: {err}"),
            StateError::Observability(err) => write!(f, "state observability error: {err}"),
        }
    }
}

impl std::error::Error for StateError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            StateError::Config(err) => Some(err),
            StateError::Observability(err) => Some(err),
        }
    }
}

pub fn init_observability() -> Result<ObservabilityHandle, StateError> {
    let config = gittree_observability::ObservabilityConfig {
        service_name: "gittree-state".to_string(),
        ..gittree_observability::ObservabilityConfig::default()
    };
    let handle = gittree_observability::init(&config).map_err(StateError::Observability)?;
    Ok(handle)
}

#[cfg(test)]
mod tests {
    use super::ENV_STORAGE_READ_URL;
    use super::StateConfig;
    use super::StorageConfigError;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

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

    fn without_env_var<F: FnOnce()>(key: &str, f: F) {
        let previous = std::env::var_os(key);
        // SAFETY: tests run single-threaded in this crate; we restore the previous value after.
        unsafe {
            std::env::remove_var(key);
        }
        f();
        match previous {
            Some(old) => unsafe {
                std::env::set_var(key, old);
            },
            None => {}
        }
    }

    #[test]
    fn config_loads_from_env() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var(
            ENV_STORAGE_READ_URL,
            "postgres://user:pass@localhost:5432/gittree",
            || {
                let config = StateConfig::from_env().expect("config");
                assert_eq!(config.bind, "127.0.0.1:8082");
                assert_eq!(
                    config.storage.read_connection,
                    "postgres://user:pass@localhost:5432/gittree"
                );
            },
        );
    }

    #[test]
    fn config_requires_storage_url() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        without_env_var(ENV_STORAGE_READ_URL, || {
            let err = StateConfig::from_env().unwrap_err();
            assert!(matches!(
                err,
                super::StateConfigError::Storage(StorageConfigError::MissingEnv(_))
            ));
        });
    }
}
