use sqlx::Postgres;
use sqlx::pool::PoolOptions;
use sqlx::postgres::PgConnectOptions;
use std::time::Duration;

pub mod cache;
pub mod migrations;
pub mod postgres;
pub mod queries;
pub mod repo;
pub mod repo_mapping;
pub mod repositories;
pub mod relay_compat;

pub use cache::{CacheConfig, CachedRepositories};
pub use migrations::{Migration, MigrationRunner};
pub use postgres::PostgresRepositories;
pub use queries::RepoFilter;
pub use repo::{RepoAnnouncementRecord, RepoStateRecord};
pub use repo_mapping::RepoMappingRecord;
pub use relay_compat::{RelayCompatibilityRecord, RelayProbeMetadata};
pub use repositories::{
    AnnouncementRepository, InMemoryRepositories, RelayCompatibilityRepository,
    RepoMappingRepository, StateRepository,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageConfig {
    pub read_connection: String,
    pub write_connection: Option<String>,
    pub max_connections: u32,
    pub min_connections: u32,
    pub idle_timeout_secs: Option<u64>,
    pub max_lifetime_secs: Option<u64>,
    pub application_name: Option<String>,
}

#[derive(Debug)]
pub enum StorageError {
    InvalidConnectionString {
        value: String,
        source: sqlx::Error,
    },
    InvalidPoolConfig {
        field: &'static str,
        value: u32,
    },
    InvalidField {
        field: &'static str,
        value: String,
    },
    InvalidHex {
        field: &'static str,
        value: String,
    },
    Serialization {
        field: &'static str,
        source: serde_json::Error,
    },
    Internal {
        message: String,
    },
    Migration {
        message: String,
    },
    Database {
        source: sqlx::Error,
    },
}

impl std::fmt::Display for StorageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StorageError::InvalidConnectionString { value, source } => {
                write!(f, "invalid connection string {value}: {source}")
            }
            StorageError::InvalidPoolConfig { field, value } => {
                write!(f, "invalid pool config {field}: {value}")
            }
            StorageError::InvalidField { field, value } => {
                write!(f, "invalid {field}: {value}")
            }
            StorageError::InvalidHex { field, value } => {
                write!(f, "invalid hex {field}: {value}")
            }
            StorageError::Serialization { field, source } => {
                write!(f, "invalid {field}: {source}")
            }
            StorageError::Internal { message } => write!(f, "internal error: {message}"),
            StorageError::Migration { message } => write!(f, "migration error: {message}"),
            StorageError::Database { source } => write!(f, "database error: {source}"),
        }
    }
}

impl std::error::Error for StorageError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            StorageError::InvalidConnectionString { source, .. } => Some(source),
            StorageError::InvalidPoolConfig { .. } => None,
            StorageError::InvalidField { .. } => None,
            StorageError::InvalidHex { .. } => None,
            StorageError::Serialization { source, .. } => Some(source),
            StorageError::Internal { .. } => None,
            StorageError::Migration { .. } => None,
            StorageError::Database { source } => Some(source),
        }
    }
}

impl From<sqlx::Error> for StorageError {
    fn from(source: sqlx::Error) -> Self {
        StorageError::Database { source }
    }
}

impl StorageConfig {
    pub fn validate(&self) -> Result<(), StorageError> {
        if self.max_connections == 0 {
            return Err(StorageError::InvalidPoolConfig {
                field: "max_connections",
                value: self.max_connections,
            });
        }

        if self.min_connections > self.max_connections {
            return Err(StorageError::InvalidPoolConfig {
                field: "min_connections",
                value: self.min_connections,
            });
        }

        Ok(())
    }

    pub fn read_connect_options(&self) -> Result<PgConnectOptions, StorageError> {
        let options: PgConnectOptions = self.read_connection.parse().map_err(|source| {
            StorageError::InvalidConnectionString {
                value: self.read_connection.clone(),
                source,
            }
        })?;
        Ok(self.apply_connect_options(options))
    }

    pub fn write_connect_options(&self) -> Result<PgConnectOptions, StorageError> {
        let connection = self
            .write_connection
            .as_ref()
            .unwrap_or(&self.read_connection);
        let options: PgConnectOptions =
            connection
                .parse()
                .map_err(|source| StorageError::InvalidConnectionString {
                    value: connection.clone(),
                    source,
                })?;
        Ok(self.apply_connect_options(options))
    }

    pub fn pool_options(&self) -> Result<PoolOptions<Postgres>, StorageError> {
        self.validate()?;

        let mut options = PoolOptions::new()
            .max_connections(self.max_connections)
            .min_connections(self.min_connections);

        options = options.idle_timeout(self.idle_timeout_secs.map(Duration::from_secs));
        options = options.max_lifetime(self.max_lifetime_secs.map(Duration::from_secs));

        Ok(options)
    }

    fn apply_connect_options(&self, mut options: PgConnectOptions) -> PgConnectOptions {
        if let Some(name) = &self.application_name {
            options = options.application_name(name);
        }
        options
    }
}

#[cfg(test)]
mod tests {
    use super::StorageConfig;
    use super::StorageError;
    use sqlx::postgres::PgSslMode;

    fn sample_config() -> StorageConfig {
        StorageConfig {
            read_connection: "postgres://user:pass@localhost:5432/gittree".to_string(),
            write_connection: None,
            max_connections: 10,
            min_connections: 2,
            idle_timeout_secs: Some(300),
            max_lifetime_secs: Some(3600),
            application_name: Some("gittree".to_string()),
        }
    }

    #[test]
    fn read_connect_options_parses_url() {
        let config = sample_config();
        let options = config.read_connect_options().expect("connect options");
        assert_eq!(options.get_host(), "localhost");
        assert_eq!(options.get_port(), 5432);
        assert_eq!(options.get_username(), "user");
        assert_eq!(options.get_database(), Some("gittree"));
        assert!(matches!(options.get_ssl_mode(), PgSslMode::Prefer));
        assert_eq!(options.get_application_name(), Some("gittree"));
    }

    #[test]
    fn write_connect_options_falls_back_to_read() {
        let config = sample_config();
        let read = config.read_connect_options().expect("read options");
        let write = config.write_connect_options().expect("write options");
        assert_eq!(read.get_host(), write.get_host());
        assert_eq!(read.get_port(), write.get_port());
        assert_eq!(read.get_database(), write.get_database());
    }

    #[test]
    fn pool_options_apply_limits() {
        let config = sample_config();
        let options = config.pool_options().expect("pool options");
        assert_eq!(options.get_max_connections(), 10);
        assert_eq!(options.get_min_connections(), 2);
        assert_eq!(options.get_idle_timeout().unwrap().as_secs(), 300);
        assert_eq!(options.get_max_lifetime().unwrap().as_secs(), 3600);
    }

    #[test]
    fn pool_options_rejects_zero_max() {
        let mut config = sample_config();
        config.max_connections = 0;
        let err = config.pool_options().unwrap_err();
        assert!(matches!(
            err,
            StorageError::InvalidPoolConfig {
                field: "max_connections",
                value: 0
            }
        ));
    }

    #[test]
    fn pool_options_rejects_min_greater_than_max() {
        let mut config = sample_config();
        config.min_connections = 12;
        let err = config.pool_options().unwrap_err();
        assert!(matches!(
            err,
            StorageError::InvalidPoolConfig {
                field: "min_connections",
                value: 12
            }
        ));
    }

    #[test]
    fn invalid_connection_string_is_rejected() {
        let mut config = sample_config();
        config.read_connection = "not-a-url".to_string();
        let err = config.read_connect_options().unwrap_err();
        assert!(matches!(err, StorageError::InvalidConnectionString { .. }));
    }

    #[test]
    fn internal_error_formats_message() {
        let err = StorageError::Internal {
            message: "lock".to_string(),
        };
        assert_eq!(err.to_string(), "internal error: lock");
    }
}
