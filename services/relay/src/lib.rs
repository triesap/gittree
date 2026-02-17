use gittree_config::{ConfigError, RelayPolicyConfig, ServicesConfig};
use gittree_core::RelayInfoDocument;
use gittree_core::nip11::RelayLimitation;
use gittree_observability::{ObservabilityConfigError, ObservabilityError, ObservabilityHandle};
use gittree_storage::{
    CachedRepositories, PostgresRepositories, RelayTenantRecord, StorageConfig, StorageError,
};
use std::time::Duration;

mod admission_client;
mod cli;
mod driver;
mod event;
mod filter;
mod metrics;
mod notice;
mod policy;
mod protocol;
mod server;
mod session;
mod store;
mod subscription;
mod tags;

pub use admission_client::{
    AdmissionDecider, AdmissionFallback, AdmissionHookClient, AdmissionHookConfig,
    AdmissionHookError, RelayEvent,
};
pub use cli::{RelayCli, RelayCliError};
pub use driver::SessionDriver;
pub use event::{EventError, NostrEvent};
pub use filter::{Filter, FilterError};
pub use metrics::RelayMetrics;
pub use notice::Notice;
pub use policy::{Policy, PolicyError};
pub use protocol::{
    ClientMessage, ProtocolError, ServerMessage, decode_client_message, encode_server_message,
};
pub use server::serve;
pub use session::Session;
pub use store::{EventStore, MemoryStore, RepositoryStore, StoreError, StoreOutcome};
pub use subscription::{SubscriptionId, SubscriptionRegistry};
pub use tags::{TagError, TagIndex};

const ENV_STORAGE_READ_URL: &str = "GITTREE_STORAGE_READ_URL";
const ENV_STORAGE_WRITE_URL: &str = "GITTREE_STORAGE_WRITE_URL";
const ENV_STORAGE_MAX_CONNECTIONS: &str = "GITTREE_STORAGE_MAX_CONNECTIONS";
const ENV_STORAGE_MIN_CONNECTIONS: &str = "GITTREE_STORAGE_MIN_CONNECTIONS";
const ENV_STORAGE_IDLE_TIMEOUT_SECS: &str = "GITTREE_STORAGE_IDLE_TIMEOUT_SECS";
const ENV_STORAGE_MAX_LIFETIME_SECS: &str = "GITTREE_STORAGE_MAX_LIFETIME_SECS";
const ENV_STORAGE_APP_NAME: &str = "GITTREE_STORAGE_APP_NAME";
const ENV_ADMISSION_URL: &str = "GITTREE_ADMISSION_URL";
const ENV_ADMISSION_TIMEOUT_SECS: &str = "GITTREE_ADMISSION_TIMEOUT_SECS";
const ENV_ADMISSION_FALLBACK: &str = "GITTREE_ADMISSION_FALLBACK";
const DEFAULT_ADMISSION_TIMEOUT_SECS: u64 = 5;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayConfig {
    pub bind: String,
    pub storage: StorageConfig,
    pub policy: RelayPolicyConfig,
    pub admission: Option<AdmissionHookConfig>,
}

impl RelayConfig {
    pub fn from_env() -> Result<Self, RelayConfigError> {
        let services = ServicesConfig::from_env_validated().map_err(RelayConfigError::Config)?;
        let storage = storage_from_env()?;
        let policy = RelayPolicyConfig::from_env().map_err(RelayConfigError::Config)?;
        let admission = admission_from_env()?;
        Ok(Self {
            bind: services.relay.bind,
            storage,
            policy,
            admission,
        })
    }

    pub fn from_toml_file(path: impl AsRef<std::path::Path>) -> Result<Self, RelayConfigError> {
        let services =
            ServicesConfig::from_toml_file_validated(path).map_err(RelayConfigError::Config)?;
        let storage = storage_from_env()?;
        let policy = RelayPolicyConfig::from_env().map_err(RelayConfigError::Config)?;
        let admission = admission_from_env()?;
        Ok(Self {
            bind: services.relay.bind,
            storage,
            policy,
            admission,
        })
    }
}

#[derive(Debug)]
pub enum RelayConfigError {
    Config(ConfigError),
    Storage(StorageConfigError),
    Admission(AdmissionConfigError),
}

impl std::fmt::Display for RelayConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RelayConfigError::Config(err) => write!(f, "relay config error: {err}"),
            RelayConfigError::Storage(err) => write!(f, "relay storage config error: {err}"),
            RelayConfigError::Admission(err) => write!(f, "relay admission config error: {err}"),
        }
    }
}

impl std::error::Error for RelayConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            RelayConfigError::Config(err) => Some(err),
            RelayConfigError::Storage(err) => Some(err),
            RelayConfigError::Admission(err) => Some(err),
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

#[derive(Debug)]
pub enum AdmissionConfigError {
    InvalidEndpoint(String),
    InvalidTimeout { value: String },
    InvalidFallback { value: String },
}

impl std::fmt::Display for AdmissionConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AdmissionConfigError::InvalidEndpoint(value) => {
                write!(f, "invalid admission endpoint: {value}")
            }
            AdmissionConfigError::InvalidTimeout { value } => {
                write!(f, "invalid admission timeout: {value}")
            }
            AdmissionConfigError::InvalidFallback { value } => {
                write!(f, "invalid admission fallback: {value}")
            }
        }
    }
}

impl std::error::Error for AdmissionConfigError {}

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

fn admission_from_env() -> Result<Option<AdmissionHookConfig>, RelayConfigError> {
    let endpoint = match std::env::var(ENV_ADMISSION_URL) {
        Ok(value) => {
            let trimmed = value.trim().to_string();
            if trimmed.is_empty() {
                return Err(RelayConfigError::Admission(
                    AdmissionConfigError::InvalidEndpoint(value),
                ));
            }
            trimmed
        }
        Err(_) => return Ok(None),
    };

    let timeout_secs = match std::env::var(ENV_ADMISSION_TIMEOUT_SECS) {
        Ok(value) => {
            if value.trim().is_empty() {
                DEFAULT_ADMISSION_TIMEOUT_SECS
            } else {
                value.parse::<u64>().map_err(|_| {
                    RelayConfigError::Admission(AdmissionConfigError::InvalidTimeout { value })
                })?
            }
        }
        Err(_) => DEFAULT_ADMISSION_TIMEOUT_SECS,
    };

    let fallback = match std::env::var(ENV_ADMISSION_FALLBACK) {
        Ok(value) => {
            if value.trim().is_empty() {
                AdmissionFallback::Reject
            } else {
                parse_admission_fallback(&value).ok_or_else(|| {
                    RelayConfigError::Admission(AdmissionConfigError::InvalidFallback { value })
                })?
            }
        }
        Err(_) => AdmissionFallback::Reject,
    };

    Ok(Some(AdmissionHookConfig::new(
        endpoint,
        Duration::from_secs(timeout_secs),
        fallback,
    )))
}

fn env_u32(key: &'static str) -> Result<Option<u32>, RelayConfigError> {
    match std::env::var(key) {
        Ok(value) => {
            if value.trim().is_empty() {
                return Ok(None);
            }
            value.parse::<u32>().map(Some).map_err(|_| {
                RelayConfigError::Storage(StorageConfigError::InvalidEnv { key, value })
            })
        }
        Err(_) => Ok(None),
    }
}

fn env_u64(key: &'static str) -> Result<Option<u64>, RelayConfigError> {
    match std::env::var(key) {
        Ok(value) => {
            if value.trim().is_empty() {
                return Ok(None);
            }
            value.parse::<u64>().map(Some).map_err(|_| {
                RelayConfigError::Storage(StorageConfigError::InvalidEnv { key, value })
            })
        }
        Err(_) => Ok(None),
    }
}

fn parse_admission_fallback(value: &str) -> Option<AdmissionFallback> {
    match value.trim().to_ascii_lowercase().as_str() {
        "accept" => Some(AdmissionFallback::Accept),
        "reject" => Some(AdmissionFallback::Reject),
        _ => None,
    }
}

#[derive(Debug)]
pub enum RelayError {
    Cli(RelayCliError),
    Config(RelayConfigError),
    Admission(AdmissionHookError),
    ObservabilityConfig(ObservabilityConfigError),
    Observability(ObservabilityError),
    Storage(StorageError),
    Serve(String),
}

impl std::fmt::Display for RelayError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RelayError::Cli(err) => write!(f, "relay cli error: {err}"),
            RelayError::Config(err) => write!(f, "relay config error: {err}"),
            RelayError::Admission(err) => write!(f, "relay admission error: {err}"),
            RelayError::ObservabilityConfig(err) => {
                write!(f, "relay observability config error: {err}")
            }
            RelayError::Observability(err) => write!(f, "relay observability error: {err}"),
            RelayError::Storage(err) => write!(f, "relay storage error: {err}"),
            RelayError::Serve(err) => write!(f, "relay serve error: {err}"),
        }
    }
}

impl std::error::Error for RelayError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            RelayError::Cli(err) => Some(err),
            RelayError::Config(err) => Some(err),
            RelayError::Admission(err) => Some(err),
            RelayError::ObservabilityConfig(err) => Some(err),
            RelayError::Observability(err) => Some(err),
            RelayError::Storage(err) => Some(err),
            RelayError::Serve(_) => None,
        }
    }
}

pub type RelayRepositories = CachedRepositories<PostgresRepositories>;

pub fn init_observability() -> Result<ObservabilityHandle, RelayError> {
    let config = gittree_observability::ObservabilityConfig::from_env("gittree-relay")
        .map_err(RelayError::ObservabilityConfig)?;
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

pub fn build_nip11_document(
    config: &RelayConfig,
    policy: &Policy,
    tenant: Option<&RelayTenantRecord>,
) -> RelayInfoDocument {
    let tenant_name = tenant.and_then(|record| record.name.clone());
    let name = tenant_name
        .or_else(|| config.storage.application_name.clone())
        .or_else(|| Some("gittree".to_string()));
    let tenant_pubkey = tenant.map(|record| hex::encode(&record.relay_pubkey));
    let tenant_auth_required = tenant.map(|record| record.auth_required);
    let tenant_restricted_writes = tenant.map(|record| !record.public_write);

    RelayInfoDocument {
        name,
        description: tenant.and_then(|record| record.description.clone()),
        banner: tenant.and_then(|record| record.banner.clone()),
        icon: tenant.and_then(|record| record.icon.clone()),
        pubkey: tenant_pubkey.clone(),
        self_pubkey: tenant_pubkey,
        contact: tenant.and_then(|record| record.contact.clone()),
        supported_nips: Some(vec![1, 11, 34]),
        software: Some("https://github.com/triesap/gittree".to_string()),
        version: Some(env!("CARGO_PKG_VERSION").to_string()),
        privacy_policy: None,
        terms_of_service: None,
        limitation: Some(RelayLimitation {
            max_message_length: config.policy.max_message_bytes,
            max_subscriptions: config.policy.max_subscriptions,
            max_limit: config.policy.max_limit,
            max_subid_length: None,
            max_event_tags: Some(policy.max_tags as u64),
            max_content_length: Some(policy.max_content_len as u64),
            min_pow_difficulty: None,
            auth_required: Some(tenant_auth_required.unwrap_or(config.policy.auth_required)),
            payment_required: None,
            restricted_writes: Some(tenant_restricted_writes.unwrap_or(true)),
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
    use super::AdmissionConfigError;
    use super::AdmissionFallback;
    use super::AdmissionHookError;
    use super::ConfigError;
    use super::DEFAULT_ADMISSION_TIMEOUT_SECS;
    use super::ENV_ADMISSION_FALLBACK;
    use super::ENV_ADMISSION_TIMEOUT_SECS;
    use super::ENV_ADMISSION_URL;
    use super::ENV_STORAGE_APP_NAME;
    use super::ENV_STORAGE_MAX_LIFETIME_SECS;
    use super::ENV_STORAGE_MAX_CONNECTIONS;
    use super::ENV_STORAGE_MIN_CONNECTIONS;
    use super::ENV_STORAGE_READ_URL;
    use super::Policy;
    use super::RelayConfig;
    use super::RelayConfigError;
    use super::RelayError;
    use super::StorageConfig;
    use super::StorageConfigError;
    use super::build_nip11_document;
    use super::build_repositories;
    use super::init_observability;
    use gittree_config::RelayPolicyConfig;
    use gittree_storage::RelayTenantRecord;
    use std::error::Error;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::Mutex;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

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

    fn write_temp_services_config(contents: &str) -> PathBuf {
        let mut path = std::env::temp_dir();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        path.push(format!("gittree-relay-services-{now}.toml"));
        fs::write(&path, contents).expect("write temp services config");
        path
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
    fn config_loads_admission_from_env() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var(
            ENV_STORAGE_READ_URL,
            "postgres://user:pass@localhost:5432/gittree",
            || {
                with_env_var(ENV_ADMISSION_URL, "http://localhost:8081/decide", || {
                    with_env_var(ENV_ADMISSION_TIMEOUT_SECS, "9", || {
                        with_env_var(ENV_ADMISSION_FALLBACK, "accept", || {
                            let config = RelayConfig::from_env().expect("config");
                            let admission = config.admission.expect("admission config");
                            assert_eq!(admission.endpoint, "http://localhost:8081/decide");
                            assert_eq!(admission.timeout, Duration::from_secs(9));
                            assert_eq!(admission.fallback, AdmissionFallback::Accept);
                        });
                    });
                });
            },
        );
    }

    #[test]
    fn config_rejects_invalid_admission_fallback() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var(
            ENV_STORAGE_READ_URL,
            "postgres://user:pass@localhost:5432/gittree",
            || {
                with_env_var(ENV_ADMISSION_URL, "http://localhost:8081/decide", || {
                    with_env_var(ENV_ADMISSION_FALLBACK, "nope", || {
                        let err = RelayConfig::from_env().unwrap_err();
                        assert!(err.to_string().contains("invalid admission fallback"));
                    });
                });
            },
        );
    }

    #[test]
    fn config_ignores_empty_pool_timeouts() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var(
            ENV_STORAGE_READ_URL,
            "postgres://user:pass@localhost:5432/gittree",
            || {
                with_env_var(super::ENV_STORAGE_IDLE_TIMEOUT_SECS, "", || {
                    with_env_var(super::ENV_STORAGE_MAX_LIFETIME_SECS, "", || {
                        let config = RelayConfig::from_env().expect("config");
                        assert_eq!(config.storage.idle_timeout_secs, None);
                        assert_eq!(config.storage.max_lifetime_secs, None);
                    });
                });
            },
        );
    }

    #[test]
    fn config_ignores_empty_max_connections_env() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var(
            ENV_STORAGE_READ_URL,
            "postgres://user:pass@localhost:5432/gittree",
            || {
                with_env_var(ENV_STORAGE_MAX_CONNECTIONS, "", || {
                    let config = RelayConfig::from_env().expect("config");
                    assert_eq!(config.storage.max_connections, 10);
                });
            },
        );
    }

    #[test]
    fn config_reports_invalid_max_connections_env() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var(
            ENV_STORAGE_READ_URL,
            "postgres://user:pass@localhost:5432/gittree",
            || {
                with_env_var(ENV_STORAGE_MAX_CONNECTIONS, "bad", || {
                    let err = RelayConfig::from_env().expect_err("invalid max connections");
                    assert!(matches!(
                        err,
                        RelayConfigError::Storage(StorageConfigError::InvalidEnv {
                            key: ENV_STORAGE_MAX_CONNECTIONS,
                            value
                        }) if value == "bad"
                    ));
                });
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
                    with_env_var("GITTREE_RELAY_POLICY_MAX_MESSAGE_BYTES", "4096", || {
                        with_env_var("GITTREE_RELAY_POLICY_MAX_SUBSCRIPTIONS", "7", || {
                            with_env_var("GITTREE_RELAY_POLICY_MAX_LIMIT", "120", || {
                                with_env_var("GITTREE_RELAY_POLICY_AUTH_REQUIRED", "true", || {
                                    let config = RelayConfig::from_env().expect("config");
                                    let policy = Policy::default();
                                    let doc = build_nip11_document(&config, &policy, None);
                                    assert_eq!(doc.name, Some("gittree-relay".to_string()));
                                    assert!(doc.supported_nips.as_ref().unwrap().contains(&34));
                                    let limitation = doc.limitation.as_ref().expect("limitation");
                                    assert_eq!(limitation.restricted_writes, Some(true));
                                    assert_eq!(
                                        limitation.max_event_tags,
                                        Some(policy.max_tags as u64)
                                    );
                                    assert_eq!(
                                        limitation.max_content_length,
                                        Some(policy.max_content_len as u64)
                                    );
                                    assert_eq!(limitation.max_message_length, Some(4096));
                                    assert_eq!(limitation.max_subscriptions, Some(7));
                                    assert_eq!(limitation.max_limit, Some(120));
                                    assert_eq!(limitation.auth_required, Some(true));
                                });
                            });
                        });
                    });
                });
            },
        );
    }

    #[test]
    fn nip11_builder_overrides_with_tenant_metadata() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var(
            ENV_STORAGE_READ_URL,
            "postgres://user:pass@localhost:5432/gittree",
            || {
                let config = RelayConfig::from_env().expect("config");
                let policy = Policy::default();
                let tenant = RelayTenantRecord::new(
                    "tenant-1",
                    "relay.gittr.ee",
                    &"11".repeat(32),
                    vec![1],
                    vec![2],
                    "v1",
                    Some("Tenant Relay".to_string()),
                    Some("Tenant description".to_string()),
                    Some("https://example.com/icon.png".to_string()),
                    None,
                    Some("ops@example.com".to_string()),
                    false,
                    true,
                    true,
                    10,
                    10,
                )
                .expect("tenant");
                let doc = build_nip11_document(&config, &policy, Some(&tenant));
                assert_eq!(doc.name, Some("Tenant Relay".to_string()));
                assert_eq!(doc.description, Some("Tenant description".to_string()));
                assert_eq!(doc.pubkey, Some("11".repeat(32)));
                let limitation = doc.limitation.as_ref().expect("limitation");
                assert_eq!(limitation.auth_required, Some(false));
                assert_eq!(limitation.restricted_writes, Some(false));
            },
        );
    }

    #[test]
    fn config_requires_storage_url() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        without_env_var(ENV_STORAGE_READ_URL, || {
            let err = RelayConfig::from_env().unwrap_err();
            assert!(err.to_string().contains("missing env"));
            assert!(err.to_string().contains(ENV_STORAGE_READ_URL));
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
            policy: RelayPolicyConfig::default(),
            admission: None,
        };

        let err = build_repositories(&config).unwrap_err();
        assert!(err.to_string().contains("relay storage error"));
    }

    #[test]
    fn repository_builder_rejects_invalid_pool_settings() {
        let config = RelayConfig {
            bind: "0.0.0.0:8080".to_string(),
            storage: gittree_storage::StorageConfig {
                read_connection: "postgres://user:pass@localhost:5432/gittree".to_string(),
                write_connection: None,
                max_connections: 0,
                min_connections: 0,
                idle_timeout_secs: None,
                max_lifetime_secs: None,
                application_name: Some("gittree".to_string()),
            },
            policy: RelayPolicyConfig::default(),
            admission: None,
        };

        let err = build_repositories(&config).expect_err("invalid pool settings");
        assert!(err.to_string().contains("relay storage error"));
    }

    #[test]
    fn observability_init_reports_invalid_env() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var("GITTREE_LOG_JSON", "definitely-not-bool", || {
            let err = init_observability().expect_err("invalid observability env");
            assert!(err.to_string().contains("observability config error"));
        });
    }

    #[test]
    fn observability_init_second_call_reports_subscriber_conflict() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        let _ = init_observability();
        let second = init_observability().expect_err("second init should fail");
        assert!(second.to_string().contains("relay observability error"));
    }

    #[test]
    fn config_loads_from_toml_file() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var(
            ENV_STORAGE_READ_URL,
            "postgres://user:pass@localhost:5432/gittree",
            || {
                let path = write_temp_services_config(
                    r#"
[services.relay]
bind = "127.0.0.1:9010"
"#,
                );
                let config = RelayConfig::from_toml_file(&path).expect("relay config");
                assert_eq!(config.bind, "127.0.0.1:9010");
                let _ = fs::remove_file(path);
            },
        );
    }

    #[test]
    fn config_reports_invalid_admission_settings() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var(
            ENV_STORAGE_READ_URL,
            "postgres://user:pass@localhost:5432/gittree",
            || {
                with_env_var(ENV_ADMISSION_URL, " ", || {
                    let err = RelayConfig::from_env().expect_err("invalid endpoint");
                    assert!(err.to_string().contains("invalid admission endpoint"));
                });
                with_env_var(ENV_ADMISSION_URL, "http://localhost:8081/decide", || {
                    with_env_var(ENV_ADMISSION_TIMEOUT_SECS, "bad", || {
                        let err = RelayConfig::from_env().expect_err("invalid timeout");
                        assert!(err.to_string().contains("invalid admission timeout"));
                    });
                });
            },
        );
    }

    #[test]
    fn config_reports_invalid_storage_settings() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var(
            ENV_STORAGE_READ_URL,
            "postgres://user:pass@localhost:5432/gittree",
            || {
                with_env_var(super::ENV_STORAGE_IDLE_TIMEOUT_SECS, "bad", || {
                    let err = RelayConfig::from_env().expect_err("invalid timeout");
                    assert!(err.to_string().contains(super::ENV_STORAGE_IDLE_TIMEOUT_SECS));
                });
                with_env_var(super::ENV_STORAGE_MAX_CONNECTIONS, "1", || {
                    with_env_var(super::ENV_STORAGE_MIN_CONNECTIONS, "2", || {
                        let err = RelayConfig::from_env().expect_err("invalid config");
                        assert!(err.to_string().contains("min_connections"));
                    });
                });
            },
        );
    }

    #[test]
    fn config_reports_invalid_min_connections_and_max_lifetime_env() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var(
            ENV_STORAGE_READ_URL,
            "postgres://user:pass@localhost:5432/gittree",
            || {
                with_env_var(ENV_STORAGE_MIN_CONNECTIONS, "bad", || {
                    let err = RelayConfig::from_env().expect_err("invalid min connections");
                    assert!(matches!(
                        err,
                        RelayConfigError::Storage(StorageConfigError::InvalidEnv {
                            key: ENV_STORAGE_MIN_CONNECTIONS,
                            ..
                        })
                    ));
                });
                with_env_var(ENV_STORAGE_MAX_LIFETIME_SECS, "bad", || {
                    let err = RelayConfig::from_env().expect_err("invalid max lifetime");
                    assert!(matches!(
                        err,
                        RelayConfigError::Storage(StorageConfigError::InvalidEnv {
                            key: ENV_STORAGE_MAX_LIFETIME_SECS,
                            ..
                        })
                    ));
                });
            },
        );
    }

    #[test]
    fn config_from_toml_file_reports_config_error_for_missing_path() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var(
            ENV_STORAGE_READ_URL,
            "postgres://user:pass@localhost:5432/gittree",
            || {
                let mut path = std::env::temp_dir();
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos();
                path.push(format!("missing-relay-config-{now}.toml"));
                let err = RelayConfig::from_toml_file(&path).expect_err("missing path");
                assert!(err.to_string().contains("config error"));
            },
        );
    }

    #[test]
    fn config_reports_invalid_policy_settings() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var(
            ENV_STORAGE_READ_URL,
            "postgres://user:pass@localhost:5432/gittree",
            || {
                with_env_var("GITTREE_RELAY_POLICY_MAX_LIMIT", "bad", || {
                    let err = RelayConfig::from_env().expect_err("invalid policy");
                    assert!(err.to_string().contains("config error"));
                });
            },
        );
    }

    #[test]
    fn config_from_toml_file_reports_storage_policy_and_admission_errors() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        let path = write_temp_services_config(
            r#"
[services.relay]
bind = "127.0.0.1:9010"
"#,
        );

        without_env_var(ENV_STORAGE_READ_URL, || {
            let err = RelayConfig::from_toml_file(&path).expect_err("missing storage env");
            assert!(err.to_string().contains("missing env"));
            assert!(err.to_string().contains(ENV_STORAGE_READ_URL));
        });

        with_env_var(
            ENV_STORAGE_READ_URL,
            "postgres://user:pass@localhost:5432/gittree",
            || {
                with_env_var("GITTREE_RELAY_POLICY_MAX_LIMIT", "bad", || {
                    let err = RelayConfig::from_toml_file(&path).expect_err("invalid policy");
                    assert!(err.to_string().contains("config error"));
                });
                with_env_var(ENV_ADMISSION_URL, " ", || {
                    let err = RelayConfig::from_toml_file(&path).expect_err("invalid admission");
                    assert!(err.to_string().contains("invalid admission endpoint"));
                });
            },
        );

        let _ = fs::remove_file(path);
    }

    #[test]
    fn config_parses_min_connections_and_lifetimes_from_env() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var(
            ENV_STORAGE_READ_URL,
            "postgres://user:pass@localhost:5432/gittree",
            || {
                with_env_var(ENV_STORAGE_MIN_CONNECTIONS, "3", || {
                    with_env_var(ENV_STORAGE_MAX_LIFETIME_SECS, "42", || {
                        let config = RelayConfig::from_env().expect("config");
                        assert_eq!(config.storage.min_connections, 3);
                        assert_eq!(config.storage.max_lifetime_secs, Some(42));
                    });
                });
            },
        );
    }

    #[test]
    fn config_error_and_admission_error_display_variants_are_stable() {
        let config_err = RelayConfigError::Config(ConfigError::MissingEnv("MISSING_CONFIG"));
        assert_eq!(
            config_err.to_string(),
            "relay config error: missing env MISSING_CONFIG"
        );

        let storage_err = RelayConfigError::Storage(StorageConfigError::InvalidEnv {
            key: "GITTREE_STORAGE_MAX_CONNECTIONS",
            value: "bad".to_string(),
        });
        assert_eq!(
            storage_err.to_string(),
            "relay storage config error: invalid env GITTREE_STORAGE_MAX_CONNECTIONS: bad"
        );

        let admission_err = RelayConfigError::Admission(AdmissionConfigError::InvalidFallback {
            value: "unknown".to_string(),
        });
        assert_eq!(
            admission_err.to_string(),
            "relay admission config error: invalid admission fallback: unknown"
        );
        assert!(admission_err.source().is_some());
    }

    #[test]
    fn storage_and_admission_config_error_display_variants_are_stable() {
        let storage_missing = StorageConfigError::MissingEnv("GITTREE_STORAGE_READ_URL");
        assert_eq!(
            storage_missing.to_string(),
            "missing env GITTREE_STORAGE_READ_URL"
        );
        let storage_invalid =
            StorageConfigError::InvalidConfig("invalid pool config max_connections: 0".to_string());
        assert_eq!(
            storage_invalid.to_string(),
            "invalid pool config max_connections: 0"
        );

        let endpoint_err = AdmissionConfigError::InvalidEndpoint("bad://endpoint".to_string());
        assert_eq!(
            endpoint_err.to_string(),
            "invalid admission endpoint: bad://endpoint"
        );
        let timeout_err = AdmissionConfigError::InvalidTimeout {
            value: "abc".to_string(),
        };
        assert_eq!(timeout_err.to_string(), "invalid admission timeout: abc");
    }

    #[test]
    fn admission_env_defaults_and_empty_fallback_resolve_to_reject() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var(ENV_ADMISSION_URL, "http://localhost:8081/decide", || {
            without_env_var(ENV_ADMISSION_FALLBACK, || {
                let config = super::admission_from_env()
                    .expect("admission")
                    .expect("configured");
                assert_eq!(config.fallback, AdmissionFallback::Reject);
            });
            with_env_var(ENV_ADMISSION_FALLBACK, "", || {
                let config = super::admission_from_env()
                    .expect("admission")
                    .expect("configured");
                assert_eq!(config.fallback, AdmissionFallback::Reject);
            });
        });
    }

    #[test]
    fn admission_env_empty_timeout_uses_default_timeout() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var(ENV_ADMISSION_URL, "http://localhost:8081/decide", || {
            with_env_var(ENV_ADMISSION_TIMEOUT_SECS, "", || {
                let config = super::admission_from_env()
                    .expect("admission")
                    .expect("configured");
                assert_eq!(
                    config.timeout,
                    Duration::from_secs(DEFAULT_ADMISSION_TIMEOUT_SECS)
                );
            });
        });
    }

    #[test]
    fn admission_env_fallback_parsing_trims_and_ignores_case() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var(ENV_ADMISSION_URL, "http://localhost:8081/decide", || {
            with_env_var(ENV_ADMISSION_FALLBACK, "  AcCePt  ", || {
                let config = super::admission_from_env()
                    .expect("admission")
                    .expect("configured");
                assert_eq!(config.fallback, AdmissionFallback::Accept);
            });
        });
    }

    #[test]
    fn admission_env_fallback_parses_explicit_reject() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var(ENV_ADMISSION_URL, "http://localhost:8081/decide", || {
            with_env_var(ENV_ADMISSION_FALLBACK, " reject ", || {
                let config = super::admission_from_env()
                    .expect("admission")
                    .expect("configured");
                assert_eq!(config.fallback, AdmissionFallback::Reject);
            });
        });
    }

    #[test]
    fn admission_env_without_url_disables_hook() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        without_env_var(ENV_ADMISSION_URL, || {
            let config = super::admission_from_env().expect("admission");
            assert!(config.is_none());
        });
    }

    #[test]
    fn env_helpers_restore_existing_values() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        let restore_key = "GITTREE_RELAY_ENV_RESTORE_TEST";
        // SAFETY: this test owns environment mutation and is serialized by ENV_LOCK.
        unsafe {
            std::env::set_var(restore_key, "before");
        }
        with_env_var(restore_key, "during", || {
            let current = std::env::var(restore_key).expect("var during");
            assert_eq!(current, "during");
        });
        let after = std::env::var(restore_key).expect("var after with_env_var");
        assert_eq!(after, "before");

        without_env_var(restore_key, || {
            assert!(std::env::var(restore_key).is_err());
        });
        let restored = std::env::var(restore_key).expect("var after without_env_var");
        assert_eq!(restored, "before");

        // SAFETY: this test owns environment mutation and is serialized by ENV_LOCK.
        unsafe {
            std::env::remove_var(restore_key);
        }
    }

    #[test]
    fn relay_and_config_errors_expose_sources() {
        let _guard = ENV_LOCK.lock().expect("env lock");

        let storage_err = RelayConfigError::Storage(StorageConfigError::MissingEnv("MISSING"));
        assert!(
            storage_err
                .to_string()
                .contains("relay storage config error")
        );
        assert!(storage_err.source().is_some());

        with_env_var("GITTREE_RELAY_BIND", "bad-bind", || {
            with_env_var(
                ENV_STORAGE_READ_URL,
                "postgres://user:pass@localhost:5432/gittree",
                || {
                    let err = RelayConfig::from_env().expect_err("invalid relay bind");
                    assert!(err.to_string().contains("config error"));
                    assert!(err.source().is_some());
                },
            );
        });

        let relay_err = RelayError::Admission(AdmissionHookError::Transport("offline".to_string()));
        assert!(relay_err.to_string().contains("relay admission error"));
        assert!(relay_err.source().is_some());
        let relay_err = RelayError::Serve("boom".to_string());
        assert!(relay_err.to_string().contains("relay serve error"));
        assert!(relay_err.source().is_none());
    }

    #[test]
    fn relay_error_display_and_source_cover_all_variants() {
        let cli = RelayError::Cli(super::RelayCliError::UnknownFlag("--bad".to_string()));
        assert_eq!(cli.to_string(), "relay cli error: unknown flag --bad");
        assert!(cli.source().is_some());

        let config =
            RelayError::Config(RelayConfigError::Config(ConfigError::MissingEnv("MISSING")));
        assert!(
            config
                .to_string()
                .contains("relay config error: missing env MISSING")
        );
        assert!(config.source().is_some());

        let observability_config =
            RelayError::ObservabilityConfig(super::ObservabilityConfigError::InvalidEnv {
                key: "GITTREE_LOG_JSON",
                value: "maybe".to_string(),
            });
        assert!(
            observability_config
                .to_string()
                .contains("relay observability config error")
        );
        assert!(observability_config.source().is_some());

        let observability =
            RelayError::Observability(super::ObservabilityError::SubscriberInit("dup".to_string()));
        assert!(
            observability
                .to_string()
                .contains("relay observability error")
        );
        assert!(observability.source().is_some());

        let storage = RelayError::Storage(gittree_storage::StorageError::Internal {
            message: "backend".to_string(),
        });
        assert!(storage.to_string().contains("relay storage error"));
        assert!(storage.source().is_some());

        let serve = RelayError::Serve("boom".to_string());
        assert_eq!(serve.to_string(), "relay serve error: boom");
        assert!(serve.source().is_none());
    }

    #[test]
    fn nip11_builder_uses_default_name_without_app_or_tenant_name() {
        let config = RelayConfig {
            bind: "0.0.0.0:8080".to_string(),
            storage: StorageConfig {
                read_connection: "postgres://gittree:gittree@127.0.0.1:5432/gittree".to_string(),
                write_connection: None,
                max_connections: 10,
                min_connections: 2,
                idle_timeout_secs: None,
                max_lifetime_secs: None,
                application_name: None,
            },
            policy: RelayPolicyConfig::default(),
            admission: None,
        };
        let policy = Policy::default();
        let doc = build_nip11_document(&config, &policy, None);
        assert_eq!(doc.name, Some("gittree".to_string()));
    }

    #[tokio::test]
    async fn repository_builder_accepts_valid_storage_config() {
        let config = RelayConfig {
            bind: "0.0.0.0:8080".to_string(),
            storage: StorageConfig {
                read_connection: "postgres://gittree:gittree@127.0.0.1:5432/gittree".to_string(),
                write_connection: None,
                max_connections: 10,
                min_connections: 2,
                idle_timeout_secs: None,
                max_lifetime_secs: None,
                application_name: Some("gittree-relay".to_string()),
            },
            policy: RelayPolicyConfig::default(),
            admission: None,
        };
        let repositories = build_repositories(&config).expect("repositories");
        let _ = repositories;
    }
}
