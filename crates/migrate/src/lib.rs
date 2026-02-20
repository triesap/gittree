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
    use super::{MigrationConfig, MigrationConfigError, MigrationError, run, run_with_config};
    use gittree_storage::{StorageConfig, StorageError};
    use std::error::Error;
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

    #[test]
    fn config_rejects_invalid_u64_numbers() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var(
            super::ENV_STORAGE_READ_URL,
            "postgres://user:pass@localhost:5432/gittree",
            || {
                with_env_var(super::ENV_STORAGE_IDLE_TIMEOUT_SECS, "nope", || {
                    let err = MigrationConfig::from_env().unwrap_err();
                    assert!(matches!(
                        err,
                        MigrationConfigError::InvalidEnv {
                            key: super::ENV_STORAGE_IDLE_TIMEOUT_SECS,
                            ..
                        }
                    ));
                });
            },
        );
    }

    #[test]
    fn migration_error_display_and_source_paths_are_stable() {
        let config = MigrationConfigError::MissingEnv("READ_URL");
        assert_eq!(format!("{config}"), "missing env READ_URL");

        let invalid = MigrationConfigError::InvalidEnv {
            key: "MAX",
            value: "bad".to_string(),
        };
        assert_eq!(format!("{invalid}"), "invalid env MAX: bad");

        let invalid_cfg = MigrationConfigError::InvalidConfig("invalid".to_string());
        assert_eq!(format!("{invalid_cfg}"), "invalid");

        let config_error = MigrationError::Config(MigrationConfigError::MissingEnv("READ_URL"));
        assert!(format!("{config_error}").contains("migration config error"));
        assert!(config_error.source().is_none());

        let storage_error = MigrationError::Storage(StorageError::Internal {
            message: "db".to_string(),
        });
        assert!(format!("{storage_error}").contains("migration storage error"));
        assert!(storage_error.source().is_some());
    }

    #[tokio::test]
    async fn run_maps_config_error_from_env() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        unsafe {
            std::env::remove_var(super::ENV_STORAGE_READ_URL);
        }
        let err = run().await.expect_err("missing env");
        assert!(matches!(
            err,
            MigrationError::Config(MigrationConfigError::MissingEnv(
                super::ENV_STORAGE_READ_URL
            ))
        ));
    }

    #[tokio::test]
    async fn run_with_config_maps_write_option_and_connect_errors() {
        let invalid_options = MigrationConfig {
            storage: StorageConfig {
                read_connection: "postgres://user:pass@127.0.0.1:5432/gittree".to_string(),
                write_connection: Some("://invalid".to_string()),
                max_connections: 5,
                min_connections: 1,
                idle_timeout_secs: None,
                max_lifetime_secs: None,
                application_name: None,
            },
        };
        let invalid_err = run_with_config(&invalid_options)
            .await
            .expect_err("invalid options");
        assert!(matches!(invalid_err, MigrationError::Storage(_)));

        let connect_error_config = MigrationConfig {
            storage: StorageConfig {
                read_connection: "postgres://user:pass@127.0.0.1:1/gittree".to_string(),
                write_connection: None,
                max_connections: 5,
                min_connections: 1,
                idle_timeout_secs: None,
                max_lifetime_secs: None,
                application_name: None,
            },
        };
        let connect_err = run_with_config(&connect_error_config)
            .await
            .expect_err("connect failure");
        assert!(matches!(connect_err, MigrationError::Storage(_)));
    }

    #[test]
    fn with_env_var_restores_previous_values() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        unsafe {
            std::env::set_var(super::ENV_STORAGE_READ_URL, "before");
        }
        with_env_var(
            super::ENV_STORAGE_READ_URL,
            "postgres://user:pass@localhost:5432/gittree",
            || {
                assert_eq!(
                    std::env::var(super::ENV_STORAGE_READ_URL).as_deref(),
                    Ok("postgres://user:pass@localhost:5432/gittree")
                );
            },
        );
        assert_eq!(
            std::env::var(super::ENV_STORAGE_READ_URL).as_deref(),
            Ok("before")
        );
        unsafe {
            std::env::remove_var(super::ENV_STORAGE_READ_URL);
        }
    }
}
