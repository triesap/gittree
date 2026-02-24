use crate::error::StorageError;
use sqlx::Postgres;
use sqlx::pool::PoolOptions;
use sqlx::postgres::PgConnectOptions;
use std::time::Duration;

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
        Ok(self.pool_options_validated())
    }

    pub fn pool_options_validated(&self) -> PoolOptions<Postgres> {
        let mut options = PoolOptions::new()
            .max_connections(self.max_connections)
            .min_connections(self.min_connections);

        options = options.idle_timeout(self.idle_timeout_secs.map(Duration::from_secs));
        options = options.max_lifetime(self.max_lifetime_secs.map(Duration::from_secs));

        options
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
    use crate::StorageError;
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

    fn assert_invalid_connection_string(err: StorageError) {
        if !matches!(err, StorageError::InvalidConnectionString { .. }) {
            panic!("expected invalid connection string error, got {err:?}");
        }
    }

    fn assert_ssl_mode_prefer(mode: PgSslMode) {
        if !matches!(mode, PgSslMode::Prefer) {
            panic!("expected PgSslMode::Prefer, got {mode:?}");
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
        assert_ssl_mode_prefer(options.get_ssl_mode());
        assert_eq!(options.get_application_name(), Some("gittree"));
    }

    #[test]
    fn read_connect_options_omits_application_name_when_not_set() {
        let mut config = sample_config();
        config.application_name = None;
        let options = config.read_connect_options().expect("connect options");
        assert_eq!(options.get_application_name(), None);
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
    fn write_connect_options_uses_write_connection_when_set() {
        let mut config = sample_config();
        config.write_connection =
            Some("postgres://writer:pass@writer-host:5432/gittree_write".to_string());
        let write = config.write_connect_options().expect("write options");
        assert_eq!(write.get_host(), "writer-host");
        assert_eq!(write.get_username(), "writer");
        assert_eq!(write.get_database(), Some("gittree_write"));
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
    fn pool_options_allow_none_timeouts() {
        let mut config = sample_config();
        config.idle_timeout_secs = None;
        config.max_lifetime_secs = None;
        let options = config.pool_options().expect("pool options");
        assert_eq!(options.get_idle_timeout(), None);
        assert_eq!(options.get_max_lifetime(), None);
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
        assert_invalid_connection_string(err);
    }

    #[test]
    fn invalid_write_connection_string_is_rejected() {
        let mut config = sample_config();
        config.write_connection = Some("not-a-url".to_string());
        let err = config.write_connect_options().unwrap_err();
        assert_invalid_connection_string(err);
    }

    #[test]
    #[should_panic(expected = "expected invalid connection string error")]
    fn assert_invalid_connection_string_panics_for_other_errors() {
        assert_invalid_connection_string(StorageError::InvalidPoolConfig {
            field: "max_connections",
            value: 0,
        });
    }

    #[test]
    #[should_panic(expected = "expected PgSslMode::Prefer")]
    fn assert_ssl_mode_prefer_panics_for_other_modes() {
        assert_ssl_mode_prefer(PgSslMode::Disable);
    }

    #[test]
    fn validate_accepts_equal_pool_bounds() {
        let mut config = sample_config();
        config.max_connections = 4;
        config.min_connections = 4;
        config.validate().expect("equal bounds should validate");
    }
}
