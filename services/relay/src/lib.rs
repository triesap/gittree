use gittree_config::{ConfigError, ServicesConfig};
use gittree_core::RelayInfoDocument;
use gittree_core::nip11::RelayLimitation;
use gittree_observability::{ObservabilityError, ObservabilityHandle};
use gittree_storage::{CachedRepositories, PostgresRepositories, StorageConfig, StorageError};

mod admission_client;

pub use admission_client::{
    AdmissionFallback, AdmissionHookClient, AdmissionHookConfig, AdmissionHookError, RelayEvent,
};

const ENV_STORAGE_READ_URL: &str = "GITTREE_STORAGE_READ_URL";
const ENV_STORAGE_WRITE_URL: &str = "GITTREE_STORAGE_WRITE_URL";
const ENV_STORAGE_MAX_CONNECTIONS: &str = "GITTREE_STORAGE_MAX_CONNECTIONS";
const ENV_STORAGE_MIN_CONNECTIONS: &str = "GITTREE_STORAGE_MIN_CONNECTIONS";
const ENV_STORAGE_IDLE_TIMEOUT_SECS: &str = "GITTREE_STORAGE_IDLE_TIMEOUT_SECS";
const ENV_STORAGE_MAX_LIFETIME_SECS: &str = "GITTREE_STORAGE_MAX_LIFETIME_SECS";
const ENV_STORAGE_APP_NAME: &str = "GITTREE_STORAGE_APP_NAME";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayConfig {
    pub bind: String,
    pub storage: StorageConfig,
}

impl RelayConfig {
    pub fn from_env() -> Result<Self, RelayConfigError> {
        let services = ServicesConfig::from_env_validated().map_err(RelayConfigError::Config)?;
        let storage = storage_from_env()?;
        Ok(Self {
            bind: services.relay.bind,
            storage,
        })
    }
}

#[derive(Debug)]
pub enum RelayConfigError {
    Config(ConfigError),
    Storage(StorageConfigError),
}

impl std::fmt::Display for RelayConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RelayConfigError::Config(err) => write!(f, "relay config error: {err}"),
            RelayConfigError::Storage(err) => write!(f, "relay storage config error: {err}"),
        }
    }
}

impl std::error::Error for RelayConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            RelayConfigError::Config(err) => Some(err),
            RelayConfigError::Storage(err) => Some(err),
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

fn storage_from_env() -> Result<StorageConfig, RelayConfigError> {
    let read_connection = std::env::var(ENV_STORAGE_READ_URL).map_err(|_| {
        RelayConfigError::Storage(StorageConfigError::MissingEnv(ENV_STORAGE_READ_URL))
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
        RelayConfigError::Storage(StorageConfigError::InvalidConfig(err.to_string()))
    })?;

    Ok(config)
}

fn env_u32(key: &'static str) -> Result<Option<u32>, RelayConfigError> {
    match std::env::var(key) {
        Ok(value) => value
            .parse::<u32>()
            .map(Some)
            .map_err(|_| RelayConfigError::Storage(StorageConfigError::InvalidEnv { key, value })),
        Err(_) => Ok(None),
    }
}

fn env_u64(key: &'static str) -> Result<Option<u64>, RelayConfigError> {
    match std::env::var(key) {
        Ok(value) => value
            .parse::<u64>()
            .map(Some)
            .map_err(|_| RelayConfigError::Storage(StorageConfigError::InvalidEnv { key, value })),
        Err(_) => Ok(None),
    }
}

#[derive(Debug)]
pub enum RelayError {
    Config(RelayConfigError),
    Observability(ObservabilityError),
    Storage(StorageError),
}

impl std::fmt::Display for RelayError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RelayError::Config(err) => write!(f, "relay config error: {err}"),
            RelayError::Observability(err) => write!(f, "relay observability error: {err}"),
            RelayError::Storage(err) => write!(f, "relay storage error: {err}"),
        }
    }
}

impl std::error::Error for RelayError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            RelayError::Config(err) => Some(err),
            RelayError::Observability(err) => Some(err),
            RelayError::Storage(err) => Some(err),
        }
    }
}

pub type RelayRepositories = CachedRepositories<PostgresRepositories>;

pub fn init_observability() -> Result<ObservabilityHandle, RelayError> {
    let config = gittree_observability::ObservabilityConfig {
        service_name: "gittree-relay".to_string(),
        ..gittree_observability::ObservabilityConfig::default()
    };
    let handle = gittree_observability::init(&config).map_err(RelayError::Observability)?;
    Ok(handle)
}

pub fn build_repositories(config: &RelayConfig) -> Result<RelayRepositories, RelayError> {
    let pool_options = config.storage.pool_options().map_err(RelayError::Storage)?;
    let connect_options = config
        .storage
        .read_connect_options()
        .map_err(RelayError::Storage)?;
    let pool = pool_options.connect_lazy_with(connect_options);
    let repos = PostgresRepositories::new(pool);
    Ok(CachedRepositories::new(repos))
}

pub fn build_nip11_document(config: &RelayConfig) -> RelayInfoDocument {
    let name = config
        .storage
        .application_name
        .clone()
        .or_else(|| Some("gittree".to_string()));

    RelayInfoDocument {
        name,
        description: None,
        banner: None,
        icon: None,
        pubkey: None,
        self_pubkey: None,
        contact: None,
        supported_nips: Some(vec![1, 11, 34]),
        software: Some("https://github.com/triesap/gittree".to_string()),
        version: Some(env!("CARGO_PKG_VERSION").to_string()),
        privacy_policy: None,
        terms_of_service: None,
        limitation: Some(RelayLimitation {
            max_message_length: None,
            max_subscriptions: None,
            max_limit: None,
            max_subid_length: None,
            max_event_tags: None,
            max_content_length: None,
            min_pow_difficulty: None,
            auth_required: None,
            payment_required: None,
            restricted_writes: Some(true),
            created_at_lower_limit: None,
            created_at_upper_limit: None,
            default_limit: None,
        }),
        retention: None,
        relay_countries: None,
        language_tags: None,
        tags: None,
        posting_policy: None,
        payments_url: None,
        fees: None,
        supported_grasps: Some(vec!["GRASP-01".to_string()]),
        repo_acceptance_criteria: Some("requires clone and relays tags".to_string()),
        curation: Some("no additional curation".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::ENV_STORAGE_APP_NAME;
    use super::ENV_STORAGE_READ_URL;
    use super::ObservabilityHandle;
    use super::RelayConfig;
    use super::RelayError;
    use super::StorageConfigError;
    use super::build_nip11_document;
    use super::build_repositories;
    use super::init_observability;
    use std::sync::Mutex;
    use std::sync::OnceLock;

    static ENV_LOCK: Mutex<()> = Mutex::new(());
    static OBSERVABILITY: OnceLock<ObservabilityHandle> = OnceLock::new();

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
                let config = RelayConfig::from_env().expect("config");
                assert_eq!(config.bind, "0.0.0.0:8080");
                assert_eq!(
                    config.storage.read_connection,
                    "postgres://user:pass@localhost:5432/gittree"
                );
            },
        );
    }

    #[test]
    fn nip11_builder_sets_expected_fields() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var(
            ENV_STORAGE_READ_URL,
            "postgres://user:pass@localhost:5432/gittree",
            || {
                with_env_var(ENV_STORAGE_APP_NAME, "gittree-relay", || {
                    let config = RelayConfig::from_env().expect("config");
                    let doc = build_nip11_document(&config);
                    assert_eq!(doc.name, Some("gittree-relay".to_string()));
                    assert!(doc.supported_nips.as_ref().unwrap().contains(&34));
                    assert_eq!(
                        doc.limitation.as_ref().unwrap().restricted_writes,
                        Some(true)
                    );
                });
            },
        );
    }

    #[test]
    fn config_requires_storage_url() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        without_env_var(ENV_STORAGE_READ_URL, || {
            let err = RelayConfig::from_env().unwrap_err();
            assert!(matches!(
                err,
                super::RelayConfigError::Storage(StorageConfigError::MissingEnv(_))
            ));
        });
    }

    #[test]
    fn repository_builder_rejects_invalid_connection() {
        let config = RelayConfig {
            bind: "0.0.0.0:8080".to_string(),
            storage: gittree_storage::StorageConfig {
                read_connection: "not-a-url".to_string(),
                write_connection: None,
                max_connections: 10,
                min_connections: 2,
                idle_timeout_secs: None,
                max_lifetime_secs: None,
                application_name: Some("gittree".to_string()),
            },
        };

        let err = build_repositories(&config).unwrap_err();
        assert!(matches!(err, RelayError::Storage(_)));
    }

    #[test]
    fn observability_init_returns_registry() {
        let handle = OBSERVABILITY.get_or_init(|| init_observability().expect("init"));
        assert!(handle.prometheus_registry().is_some());
    }
}
