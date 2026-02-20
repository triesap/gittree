use gittree_app::{AppServiceConfig, AppServiceConfigError, StorageConfigError};
use std::sync::{Mutex, OnceLock};

const ENV_KEYS: [&str; 15] = [
    "GITTREE_APP_BIND",
    "GITTREE_APP_BASE_PATH",
    "GITTREE_APP_SITE_ROOT",
    "GITTREE_APP_SITE_PKG_DIR",
    "GITTREE_STORAGE_READ_URL",
    "GITTREE_STORAGE_WRITE_URL",
    "GITTREE_STORAGE_MAX_CONNECTIONS",
    "GITTREE_STORAGE_MIN_CONNECTIONS",
    "GITTREE_STORAGE_IDLE_TIMEOUT_SECS",
    "GITTREE_STORAGE_MAX_LIFETIME_SECS",
    "GITTREE_STORAGE_APP_NAME",
    "GITTREE_UI_REPO_ROOT",
    "GITTREE_UI_PUBLIC_GIT_URL",
    "GITTREE_UI_AUTH_URL",
    "GITTREE_UI_CONTROL_URL",
];

fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn with_env_overrides<F>(overrides: &[(&str, Option<&str>)], run: F)
where
    F: FnOnce(),
{
    let _guard = env_lock().lock().expect("env lock");
    let previous: Vec<(String, Option<String>)> = ENV_KEYS
        .iter()
        .map(|key| ((*key).to_string(), std::env::var(key).ok()))
        .collect();

    for key in ENV_KEYS {
        unsafe {
            std::env::remove_var(key);
        }
    }
    for (key, value) in overrides {
        match value {
            Some(value) => unsafe {
                std::env::set_var(key, value);
            },
            None => unsafe {
                std::env::remove_var(key);
            },
        }
    }

    run();

    for (key, value) in previous {
        match value {
            Some(value) => unsafe {
                std::env::set_var(&key, value);
            },
            None => unsafe {
                std::env::remove_var(&key);
            },
        }
    }
}

fn minimum_ui_env() -> [(&'static str, Option<&'static str>); 2] {
    [
        ("GITTREE_UI_REPO_ROOT", Some("/tmp/gittree-ui")),
        ("GITTREE_UI_PUBLIC_GIT_URL", Some("https://gittr.ee")),
    ]
}

#[test]
fn app_service_config_from_env_reads_values_and_normalizes_base_path() {
    with_env_overrides(
        &[
            ("GITTREE_APP_BIND", Some("127.0.0.1:9090")),
            ("GITTREE_APP_BASE_PATH", Some("ui/")),
            ("GITTREE_APP_SITE_ROOT", Some("/tmp/site-root")),
            ("GITTREE_APP_SITE_PKG_DIR", Some("pkg-static")),
            (
                "GITTREE_STORAGE_READ_URL",
                Some("postgres://gittree:gittree@127.0.0.1:5432/gittree"),
            ),
            (
                "GITTREE_STORAGE_WRITE_URL",
                Some("postgres://gittree:gittree@127.0.0.1:5432/gittree_rw"),
            ),
            ("GITTREE_STORAGE_MAX_CONNECTIONS", Some("33")),
            ("GITTREE_STORAGE_MIN_CONNECTIONS", Some("4")),
            ("GITTREE_STORAGE_IDLE_TIMEOUT_SECS", Some("45")),
            ("GITTREE_STORAGE_MAX_LIFETIME_SECS", Some("90")),
            ("GITTREE_STORAGE_APP_NAME", Some("gittree-app-tests")),
            ("GITTREE_UI_REPO_ROOT", Some("/tmp/gittree-ui")),
            ("GITTREE_UI_PUBLIC_GIT_URL", Some("https://gittr.ee")),
            ("GITTREE_UI_AUTH_URL", Some("https://auth.gittr.ee")),
            ("GITTREE_UI_CONTROL_URL", Some("https://control.gittr.ee")),
        ],
        || {
            let config = AppServiceConfig::from_env().expect("config");
            assert_eq!(config.bind.to_string(), "127.0.0.1:9090");
            assert_eq!(config.base_path, "/ui");
            assert_eq!(config.site_root, std::path::PathBuf::from("/tmp/site-root"));
            assert_eq!(config.site_pkg_dir, "pkg-static");
            assert_eq!(
                config.storage.read_connection,
                "postgres://gittree:gittree@127.0.0.1:5432/gittree"
            );
            assert_eq!(
                config.storage.write_connection.as_deref(),
                Some("postgres://gittree:gittree@127.0.0.1:5432/gittree_rw")
            );
            assert_eq!(config.storage.max_connections, 33);
            assert_eq!(config.storage.min_connections, 4);
            assert_eq!(config.storage.idle_timeout_secs, Some(45));
            assert_eq!(config.storage.max_lifetime_secs, Some(90));
            assert_eq!(
                config.storage.application_name.as_deref(),
                Some("gittree-app-tests")
            );
            assert_eq!(config.ui.auth_url, "https://auth.gittr.ee");
            assert_eq!(config.ui.control_url, "https://control.gittr.ee");
        },
    );
}

#[test]
fn app_service_config_from_env_requires_storage_read_url() {
    with_env_overrides(&minimum_ui_env(), || {
        let err = AppServiceConfig::from_env().expect_err("missing storage read url");
        assert!(matches!(
            err,
            AppServiceConfigError::Storage(StorageConfigError::MissingEnv(
                "GITTREE_STORAGE_READ_URL"
            ))
        ));
    });
}

#[test]
fn app_service_config_from_env_reports_invalid_numeric_storage_env() {
    with_env_overrides(
        &[
            (
                "GITTREE_STORAGE_READ_URL",
                Some("postgres://gittree:gittree@127.0.0.1:5432/gittree"),
            ),
            ("GITTREE_STORAGE_MAX_CONNECTIONS", Some("invalid")),
            ("GITTREE_UI_REPO_ROOT", Some("/tmp/gittree-ui")),
            ("GITTREE_UI_PUBLIC_GIT_URL", Some("https://gittr.ee")),
        ],
        || {
            let err = AppServiceConfig::from_env().expect_err("invalid storage number");
            assert!(matches!(
                err,
                AppServiceConfigError::Storage(StorageConfigError::InvalidEnv {
                    key: "GITTREE_STORAGE_MAX_CONNECTIONS",
                    ..
                })
            ));
        },
    );
}

#[test]
fn app_service_config_from_env_reports_invalid_u64_storage_env() {
    with_env_overrides(
        &[
            (
                "GITTREE_STORAGE_READ_URL",
                Some("postgres://gittree:gittree@127.0.0.1:5432/gittree"),
            ),
            ("GITTREE_STORAGE_IDLE_TIMEOUT_SECS", Some("not-a-number")),
            ("GITTREE_UI_REPO_ROOT", Some("/tmp/gittree-ui")),
            ("GITTREE_UI_PUBLIC_GIT_URL", Some("https://gittr.ee")),
        ],
        || {
            let err = AppServiceConfig::from_env().expect_err("invalid storage u64");
            assert!(matches!(
                err,
                AppServiceConfigError::Storage(StorageConfigError::InvalidEnv {
                    key: "GITTREE_STORAGE_IDLE_TIMEOUT_SECS",
                    ..
                })
            ));
        },
    );
}

#[test]
fn app_service_config_from_env_reports_invalid_pool_bounds() {
    with_env_overrides(
        &[
            (
                "GITTREE_STORAGE_READ_URL",
                Some("postgres://gittree:gittree@127.0.0.1:5432/gittree"),
            ),
            ("GITTREE_STORAGE_MAX_CONNECTIONS", Some("1")),
            ("GITTREE_STORAGE_MIN_CONNECTIONS", Some("2")),
            ("GITTREE_UI_REPO_ROOT", Some("/tmp/gittree-ui")),
            ("GITTREE_UI_PUBLIC_GIT_URL", Some("https://gittr.ee")),
        ],
        || {
            let err = AppServiceConfig::from_env().expect_err("invalid pool bounds");
            assert!(matches!(
                err,
                AppServiceConfigError::Storage(StorageConfigError::InvalidConfig(_))
            ));
        },
    );
}

#[test]
fn app_service_config_from_env_uses_defaults_for_empty_optional_values() {
    with_env_overrides(
        &[
            (
                "GITTREE_STORAGE_READ_URL",
                Some("postgres://gittree:gittree@127.0.0.1:5432/gittree"),
            ),
            ("GITTREE_APP_BIND", Some(" ")),
            ("GITTREE_APP_BASE_PATH", Some("   ")),
            ("GITTREE_APP_SITE_ROOT", Some("")),
            ("GITTREE_APP_SITE_PKG_DIR", Some(" ")),
            ("GITTREE_STORAGE_MAX_CONNECTIONS", Some("   ")),
            ("GITTREE_STORAGE_IDLE_TIMEOUT_SECS", Some(" ")),
            ("GITTREE_STORAGE_MAX_LIFETIME_SECS", Some("")),
            ("GITTREE_STORAGE_APP_NAME", Some("")),
            ("GITTREE_UI_REPO_ROOT", Some("/tmp/gittree-ui")),
            ("GITTREE_UI_PUBLIC_GIT_URL", Some("https://gittr.ee")),
        ],
        || {
            let config = AppServiceConfig::from_env().expect("config");
            assert_eq!(config.bind.to_string(), "127.0.0.1:8090");
            assert_eq!(config.base_path, "/ui");
            assert_eq!(config.site_root, std::path::PathBuf::from("crates/app-ui/dist"));
            assert_eq!(config.site_pkg_dir, "pkg");
            assert_eq!(config.storage.max_connections, 10);
            assert_eq!(config.storage.min_connections, 2);
            assert_eq!(config.storage.idle_timeout_secs, None);
            assert_eq!(config.storage.max_lifetime_secs, None);
            assert_eq!(config.storage.application_name.as_deref(), Some(""));
        },
    );
}

#[test]
fn app_service_config_from_env_reports_invalid_bind_env() {
    with_env_overrides(
        &[
            ("GITTREE_APP_BIND", Some("not-a-bind")),
            (
                "GITTREE_STORAGE_READ_URL",
                Some("postgres://gittree:gittree@127.0.0.1:5432/gittree"),
            ),
            ("GITTREE_UI_REPO_ROOT", Some("/tmp/gittree-ui")),
            ("GITTREE_UI_PUBLIC_GIT_URL", Some("https://gittr.ee")),
        ],
        || {
            let err = AppServiceConfig::from_env().expect_err("invalid bind");
            assert!(matches!(
                err,
                AppServiceConfigError::InvalidEnv {
                    key: "GITTREE_APP_BIND",
                    ..
                }
            ));
        },
    );
}
