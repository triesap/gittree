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

#[cfg(test)]
mod tests {
    use super::StorageError;
    use std::error::Error as _;

    #[test]
    fn storage_error_display_and_source_cover_variants() {
        let invalid_conn = StorageError::InvalidConnectionString {
            value: "bad".to_string(),
            source: sqlx::Error::PoolTimedOut,
        };
        assert!(invalid_conn
            .to_string()
            .contains("invalid connection string bad"));
        assert!(invalid_conn.source().is_some());

        let invalid_pool = StorageError::InvalidPoolConfig {
            field: "max_connections",
            value: 0,
        };
        assert_eq!(
            invalid_pool.to_string(),
            "invalid pool config max_connections: 0"
        );
        assert!(invalid_pool.source().is_none());

        let invalid_field = StorageError::InvalidField {
            field: "tenant_id",
            value: "empty".to_string(),
        };
        assert_eq!(invalid_field.to_string(), "invalid tenant_id: empty");
        assert!(invalid_field.source().is_none());

        let invalid_hex = StorageError::InvalidHex {
            field: "pubkey",
            value: "zz".to_string(),
        };
        assert_eq!(invalid_hex.to_string(), "invalid hex pubkey: zz");
        assert!(invalid_hex.source().is_none());

        let serde_err = serde_json::from_str::<serde_json::Value>("{").expect_err("serde error");
        let serialization = StorageError::Serialization {
            field: "report",
            source: serde_err,
        };
        assert!(serialization.to_string().contains("invalid report:"));
        assert!(serialization.source().is_some());

        let migration = StorageError::Migration {
            message: "stopped".to_string(),
        };
        assert_eq!(migration.to_string(), "migration error: stopped");
        assert!(migration.source().is_none());

        let internal = StorageError::Internal {
            message: "boom".to_string(),
        };
        assert_eq!(internal.to_string(), "internal error: boom");
        assert!(internal.source().is_none());

        let database = StorageError::Database {
            source: sqlx::Error::RowNotFound,
        };
        assert!(database.to_string().contains("database error:"));
        assert!(database.source().is_some());
    }

    #[test]
    fn internal_error_formats_message() {
        let err = StorageError::Internal {
            message: "lock".to_string(),
        };
        assert_eq!(err.to_string(), "internal error: lock");
    }

    #[test]
    fn sqlx_error_maps_to_database_variant() {
        let err = StorageError::from(sqlx::Error::PoolTimedOut);
        assert!(matches!(err, StorageError::Database { .. }));
    }
}
