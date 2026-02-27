use gittree_state::{StateConfig, StateConfigError, StateError, StorageConfigError};
use std::error::Error;
use std::sync::{Mutex, OnceLock};

fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn with_env_vars(vars: &[(&str, Option<&str>)], run: impl FnOnce()) {
    let _guard = env_lock().lock().expect("lock env");
    let previous: Vec<(&str, Option<std::ffi::OsString>)> = vars
        .iter()
        .map(|(key, _)| (*key, std::env::var_os(key)))
        .collect();

    for (key, value) in vars {
        match value {
            Some(value) => {
                // SAFETY: tests serialize environment mutation with a process-wide mutex.
                unsafe { std::env::set_var(key, value) };
            }
            None => {
                // SAFETY: tests serialize environment mutation with a process-wide mutex.
                unsafe { std::env::remove_var(key) };
            }
        }
    }

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(run));

    for (key, value) in previous {
        match value {
            Some(value) => {
                // SAFETY: tests serialize environment mutation with a process-wide mutex.
                unsafe { std::env::set_var(key, value) };
            }
            None => {
                // SAFETY: tests serialize environment mutation with a process-wide mutex.
                unsafe { std::env::remove_var(key) };
            }
        }
    }

    if let Err(payload) = result {
        std::panic::resume_unwind(payload);
    }
}

#[test]
fn state_runtime_error_traits_cover_additional_paths() {
    let storage_missing = StorageConfigError::MissingEnv("GITTREE_STORAGE_READ_URL");
    assert_eq!(
        storage_missing.to_string(),
        "missing env GITTREE_STORAGE_READ_URL"
    );
    let storage_missing_error: &dyn Error = &storage_missing;
    assert!(storage_missing_error.source().is_none());

    let config_storage = StateConfigError::Storage(StorageConfigError::InvalidConfig(
        "invalid pool".to_string(),
    ));
    assert!(config_storage.source().is_some());

    let serve_error = StateError::Serve("bind failed".to_string());
    assert_eq!(serve_error.to_string(), "state serve error: bind failed");
    assert!(serve_error.source().is_none());
}

#[test]
fn state_runtime_config_rejects_invalid_storage_numeric_env() {
    with_env_vars(
        &[
            ("GITTREE_STATE_BIND", Some("127.0.0.1:9098")),
            ("GITTREE_RELAY_URLS", Some("wss://relay.example")),
            (
                "GITTREE_STORAGE_READ_URL",
                Some("postgres://user:pass@localhost:5432/gittree"),
            ),
            ("GITTREE_STORAGE_MAX_CONNECTIONS", Some("not-a-number")),
        ],
        || {
            let err = StateConfig::from_env().expect_err("invalid storage env");
            match err {
                StateConfigError::Storage(StorageConfigError::InvalidEnv { key, value }) => {
                    assert_eq!(key, "GITTREE_STORAGE_MAX_CONNECTIONS");
                    assert_eq!(value, "not-a-number");
                }
                other => panic!("unexpected state config error: {other:?}"),
            }
        },
    );
}

#[test]
fn state_runtime_config_rejects_missing_storage_after_services_parse() {
    with_env_vars(
        &[
            ("GITTREE_STATE_BIND", Some("127.0.0.1:9098")),
            ("GITTREE_RELAY_URLS", Some("wss://relay.example")),
            ("GITTREE_STORAGE_READ_URL", None),
        ],
        || {
            let err = StateConfig::from_env().expect_err("missing storage env");
            match err {
                StateConfigError::Storage(StorageConfigError::MissingEnv(key)) => {
                    assert_eq!(key, "GITTREE_STORAGE_READ_URL");
                }
                other => panic!("unexpected state config error: {other:?}"),
            }
        },
    );
}

#[test]
fn state_runtime_config_loads_valid_storage_values() {
    with_env_vars(
        &[
            ("GITTREE_STATE_BIND", Some("127.0.0.1:9098")),
            ("GITTREE_RELAY_URLS", Some("wss://relay.example,wss://relay2.example")),
            (
                "GITTREE_STORAGE_READ_URL",
                Some("postgres://user:pass@localhost:5432/gittree"),
            ),
            ("GITTREE_STORAGE_WRITE_URL", Some("postgres://user:pass@localhost:5432/gittree")),
            ("GITTREE_STORAGE_MAX_CONNECTIONS", Some("16")),
            ("GITTREE_STORAGE_MIN_CONNECTIONS", Some("4")),
            ("GITTREE_STORAGE_IDLE_TIMEOUT_SECS", Some("30")),
            ("GITTREE_STORAGE_MAX_LIFETIME_SECS", Some("120")),
            ("GITTREE_STORAGE_APP_NAME", Some("gittree-state-tests")),
        ],
        || {
            let config = StateConfig::from_env().expect("valid state config");
            assert_eq!(config.storage.max_connections, 16);
            assert_eq!(config.storage.min_connections, 4);
            assert_eq!(config.storage.idle_timeout_secs, Some(30));
            assert_eq!(config.storage.max_lifetime_secs, Some(120));
            assert_eq!(config.storage.application_name.as_deref(), Some("gittree-state-tests"));
            assert_eq!(
                config.relay_urls,
                vec![
                    "wss://relay.example".to_string(),
                    "wss://relay2.example".to_string()
                ]
            );
        },
    );
}
