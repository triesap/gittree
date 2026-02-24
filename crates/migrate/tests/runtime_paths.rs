use gittree_migrate::{MigrationConfig, MigrationError};
use gittree_storage::StorageError;
use std::error::Error;
use std::ffi::OsString;
use std::sync::Mutex;

static ENV_LOCK: Mutex<()> = Mutex::new(());

const ENV_KEYS: [&str; 7] = [
    "GITTREE_STORAGE_READ_URL",
    "GITTREE_STORAGE_WRITE_URL",
    "GITTREE_STORAGE_MAX_CONNECTIONS",
    "GITTREE_STORAGE_MIN_CONNECTIONS",
    "GITTREE_STORAGE_IDLE_TIMEOUT_SECS",
    "GITTREE_STORAGE_MAX_LIFETIME_SECS",
    "GITTREE_STORAGE_APP_NAME",
];

fn capture_env(keys: &[&str]) -> Vec<(String, Option<OsString>)> {
    keys.iter()
        .map(|key| ((*key).to_string(), std::env::var_os(key)))
        .collect()
}

fn restore_env(values: Vec<(String, Option<OsString>)>) {
    for (key, value) in values {
        match value {
            Some(old) => {
                // SAFETY: tests are serialized by ENV_LOCK; restoring previous value is scoped.
                unsafe {
                    std::env::set_var(key, old);
                }
            }
            None => {
                // SAFETY: tests are serialized by ENV_LOCK; removing value is scoped.
                unsafe {
                    std::env::remove_var(key);
                }
            }
        }
    }
}

#[test]
fn runtime_config_reads_optional_storage_envs() {
    let _guard = ENV_LOCK.lock().expect("env lock");
    let previous = capture_env(&ENV_KEYS);
    // SAFETY: tests are serialized by ENV_LOCK; env mutation is restored at the end of the test.
    unsafe {
        std::env::set_var(
            "GITTREE_STORAGE_READ_URL",
            "postgres://user:pass@localhost:5432/gittree",
        );
        std::env::set_var(
            "GITTREE_STORAGE_WRITE_URL",
            "postgres://writer:pass@localhost:5432/gittree",
        );
        std::env::set_var("GITTREE_STORAGE_MAX_CONNECTIONS", "9");
        std::env::set_var("GITTREE_STORAGE_MIN_CONNECTIONS", "3");
        std::env::set_var("GITTREE_STORAGE_IDLE_TIMEOUT_SECS", "12");
        std::env::set_var("GITTREE_STORAGE_MAX_LIFETIME_SECS", "15");
        std::env::set_var("GITTREE_STORAGE_APP_NAME", "gittree-migrate-runtime");
    }
    let config = MigrationConfig::from_env().expect("runtime config");
    restore_env(previous);

    assert_eq!(
        config.storage.read_connection,
        "postgres://user:pass@localhost:5432/gittree"
    );
    assert_eq!(
        config.storage.write_connection.as_deref(),
        Some("postgres://writer:pass@localhost:5432/gittree")
    );
    assert_eq!(config.storage.max_connections, 9);
    assert_eq!(config.storage.min_connections, 3);
    assert_eq!(config.storage.idle_timeout_secs, Some(12));
    assert_eq!(config.storage.max_lifetime_secs, Some(15));
    assert_eq!(
        config.storage.application_name.as_deref(),
        Some("gittree-migrate-runtime")
    );
}

#[test]
fn runtime_migration_error_source_paths_are_stable() {
    let config = MigrationError::Config(gittree_migrate::MigrationConfigError::MissingEnv(
        "GITTREE_STORAGE_READ_URL",
    ));
    assert!(Error::source(&config).is_none());

    let storage = MigrationError::Storage(StorageError::Internal {
        message: "database error".to_string(),
    });
    assert!(Error::source(&storage).is_some());
}
