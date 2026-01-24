use gittree_storage::{MigrationRunner, StorageConfig, StorageError};
use sqlx::{Connection, PgConnection};

const ENV_STORAGE_READ_URL: &str = "GITTREE_STORAGE_READ_URL";
const ENV_STORAGE_WRITE_URL: &str = "GITTREE_STORAGE_WRITE_URL";
const ENV_STORAGE_MAX_CONNECTIONS: &str = "GITTREE_STORAGE_MAX_CONNECTIONS";
const ENV_STORAGE_MIN_CONNECTIONS: &str = "GITTREE_STORAGE_MIN_CONNECTIONS";
const ENV_STORAGE_IDLE_TIMEOUT_SECS: &str = "GITTREE_STORAGE_IDLE_TIMEOUT_SECS";
const ENV_STORAGE_MAX_LIFETIME_SECS: &str = "GITTREE_STORAGE_MAX_LIFETIME_SECS";
const ENV_STORAGE_APP_NAME: &str = "GITTREE_STORAGE_APP_NAME";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationConfig {
    pub storage: StorageConfig,
}

impl MigrationConfig {
    pub fn from_env() -> Result<Self, MigrationConfigError> {
        Ok(Self {
            storage: storage_from_env()?,
        })
    }
}

#[derive(Debug)]
pub enum MigrationConfigError {
    MissingEnv(&'static str),
    InvalidEnv { key: &'static str, value: String },
    InvalidConfig(String),
}

impl std::fmt::Display for MigrationConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MigrationConfigError::MissingEnv(key) => write!(f, "missing env {key}"),
            MigrationConfigError::InvalidEnv { key, value } => {
                write!(f, "invalid env {key}: {value}")
            }
            MigrationConfigError::InvalidConfig(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for MigrationConfigError {}

#[derive(Debug)]
pub enum MigrationError {
    Config(MigrationConfigError),
    Storage(StorageError),
}

impl std::fmt::Display for MigrationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MigrationError::Config(err) => write!(f, "migration config error: {err}"),
            MigrationError::Storage(err) => write!(f, "migration storage error: {err}"),
        }
    }
}

impl std::error::Error for MigrationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            MigrationError::Config(_) => None,
            MigrationError::Storage(err) => Some(err),
        }
    }
}

pub async fn run() -> Result<i64, MigrationError> {
    let config = MigrationConfig::from_env().map_err(MigrationError::Config)?;
    run_with_config(&config).await
}

async fn run_with_config(config: &MigrationConfig) -> Result<i64, MigrationError> {
    let options = config
        .storage
        .write_connect_options()
        .map_err(MigrationError::Storage)?;
    let mut connection = PgConnection::connect_with(&options)
        .await
        .map_err(StorageError::from)
        .map_err(MigrationError::Storage)?;
    let runner = MigrationRunner::new(gittree_storage::migrations::core_migrations())
        .map_err(MigrationError::Storage)?;
    let version = runner
        .run(&mut connection)
        .await
        .map_err(MigrationError::Storage)?;
    Ok(version)
}

fn storage_from_env() -> Result<StorageConfig, MigrationConfigError> {
    let read_connection = std::env::var(ENV_STORAGE_READ_URL)
        .map_err(|_| MigrationConfigError::MissingEnv(ENV_STORAGE_READ_URL))?;
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

    config
        .validate()
        .map_err(|err| MigrationConfigError::InvalidConfig(err.to_string()))?;

    Ok(config)
}

fn env_u32(key: &'static str) -> Result<Option<u32>, MigrationConfigError> {
    match std::env::var(key) {
        Ok(value) => {
            if value.trim().is_empty() {
                return Ok(None);
            }
            value
                .parse::<u32>()
                .map(Some)
                .map_err(|_| MigrationConfigError::InvalidEnv { key, value })
        }
        Err(_) => Ok(None),
    }
}

fn env_u64(key: &'static str) -> Result<Option<u64>, MigrationConfigError> {
    match std::env::var(key) {
        Ok(value) => {
            if value.trim().is_empty() {
                return Ok(None);
            }
            value
                .parse::<u64>()
                .map(Some)
                .map_err(|_| MigrationConfigError::InvalidEnv { key, value })
        }
        Err(_) => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::{MigrationConfig, MigrationConfigError};
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

    #[test]
    fn config_requires_storage_url() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        unsafe {
            std::env::remove_var(super::ENV_STORAGE_READ_URL);
        }
        let err = MigrationConfig::from_env().unwrap_err();
        assert!(matches!(
            err,
            MigrationConfigError::MissingEnv(super::ENV_STORAGE_READ_URL)
        ));
    }

    #[test]
    fn config_loads_defaults() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var(
            super::ENV_STORAGE_READ_URL,
            "postgres://user:pass@localhost:5432/gittree",
            || {
                let config = MigrationConfig::from_env().expect("config");
                assert_eq!(config.storage.max_connections, 10);
                assert_eq!(config.storage.min_connections, 2);
                assert_eq!(config.storage.idle_timeout_secs, None);
                assert_eq!(config.storage.max_lifetime_secs, None);
            },
        );
    }

    #[test]
    fn config_ignores_empty_optional_envs() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var(
            super::ENV_STORAGE_READ_URL,
            "postgres://user:pass@localhost:5432/gittree",
            || {
                with_env_var(super::ENV_STORAGE_MAX_CONNECTIONS, "", || {
                    with_env_var(super::ENV_STORAGE_MIN_CONNECTIONS, "", || {
                        with_env_var(super::ENV_STORAGE_IDLE_TIMEOUT_SECS, "", || {
                            with_env_var(super::ENV_STORAGE_MAX_LIFETIME_SECS, "", || {
                                let config = MigrationConfig::from_env().expect("config");
                                assert_eq!(config.storage.max_connections, 10);
                                assert_eq!(config.storage.min_connections, 2);
                                assert_eq!(config.storage.idle_timeout_secs, None);
                                assert_eq!(config.storage.max_lifetime_secs, None);
                            });
                        });
                    });
                });
            },
        );
    }

    #[test]
    fn config_rejects_invalid_pool_limits() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var(
            super::ENV_STORAGE_READ_URL,
            "postgres://user:pass@localhost:5432/gittree",
            || {
                with_env_var(super::ENV_STORAGE_MAX_CONNECTIONS, "1", || {
                    with_env_var(super::ENV_STORAGE_MIN_CONNECTIONS, "2", || {
                        let err = MigrationConfig::from_env().unwrap_err();
                        assert!(matches!(err, MigrationConfigError::InvalidConfig(_)));
                    });
                });
            },
        );
    }

    #[test]
    fn config_rejects_invalid_numbers() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var(
            super::ENV_STORAGE_READ_URL,
            "postgres://user:pass@localhost:5432/gittree",
            || {
                with_env_var(super::ENV_STORAGE_MAX_CONNECTIONS, "nope", || {
                    let err = MigrationConfig::from_env().unwrap_err();
                    assert!(matches!(
                        err,
                        MigrationConfigError::InvalidEnv {
                            key: super::ENV_STORAGE_MAX_CONNECTIONS,
                            ..
                        }
                    ));
                });
            },
        );
    }
}
