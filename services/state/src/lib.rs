use gittree_config::{ConfigError, RelayTargetsConfig, ServicesConfig};
use gittree_observability::{ObservabilityConfigError, ObservabilityError, ObservabilityHandle};
use gittree_storage::{
    AnnouncementRepository, RelayCompatibilityRepository, RepoFilter, StateRepository,
    StorageConfig, StorageError,
};
use serde::Serialize;
use std::collections::HashMap;
use std::time::{Duration, Instant};

const ENV_STORAGE_READ_URL: &str = "GITTREE_STORAGE_READ_URL";
const ENV_STORAGE_WRITE_URL: &str = "GITTREE_STORAGE_WRITE_URL";
const ENV_STORAGE_MAX_CONNECTIONS: &str = "GITTREE_STORAGE_MAX_CONNECTIONS";
const ENV_STORAGE_MIN_CONNECTIONS: &str = "GITTREE_STORAGE_MIN_CONNECTIONS";
const ENV_STORAGE_IDLE_TIMEOUT_SECS: &str = "GITTREE_STORAGE_IDLE_TIMEOUT_SECS";
const ENV_STORAGE_MAX_LIFETIME_SECS: &str = "GITTREE_STORAGE_MAX_LIFETIME_SECS";
const ENV_STORAGE_APP_NAME: &str = "GITTREE_STORAGE_APP_NAME";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateConfig {
    pub bind: String,
    pub storage: StorageConfig,
    pub relay_urls: Vec<String>,
}

impl StateConfig {
    pub fn from_env() -> Result<Self, StateConfigError> {
        let services = ServicesConfig::from_env_validated().map_err(StateConfigError::Config)?;
        let storage = storage_from_env()?;
        let relay_targets =
            RelayTargetsConfig::from_env_validated().map_err(StateConfigError::Config)?;
        Ok(Self {
            bind: services.state.bind,
            storage,
            relay_urls: relay_targets.relay_urls,
        })
    }
}

#[derive(Debug)]
pub enum StateConfigError {
    Config(ConfigError),
    Storage(StorageConfigError),
}

impl std::fmt::Display for StateConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StateConfigError::Config(err) => write!(f, "state config error: {err}"),
            StateConfigError::Storage(err) => write!(f, "state storage config error: {err}"),
        }
    }
}

impl std::error::Error for StateConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            StateConfigError::Config(err) => Some(err),
            StateConfigError::Storage(err) => Some(err),
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

fn storage_from_env() -> Result<StorageConfig, StateConfigError> {
    let read_connection = std::env::var(ENV_STORAGE_READ_URL).map_err(|_| {
        StateConfigError::Storage(StorageConfigError::MissingEnv(ENV_STORAGE_READ_URL))
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
        StateConfigError::Storage(StorageConfigError::InvalidConfig(err.to_string()))
    })?;

    Ok(config)
}

fn env_u32(key: &'static str) -> Result<Option<u32>, StateConfigError> {
    match std::env::var(key) {
        Ok(value) => {
            if value.trim().is_empty() {
                return Ok(None);
            }
            value.parse::<u32>().map(Some).map_err(|_| {
                StateConfigError::Storage(StorageConfigError::InvalidEnv { key, value })
            })
        }
        Err(_) => Ok(None),
    }
}

fn env_u64(key: &'static str) -> Result<Option<u64>, StateConfigError> {
    match std::env::var(key) {
        Ok(value) => {
            if value.trim().is_empty() {
                return Ok(None);
            }
            value.parse::<u64>().map(Some).map_err(|_| {
                StateConfigError::Storage(StorageConfigError::InvalidEnv { key, value })
            })
        }
        Err(_) => Ok(None),
    }
}

#[derive(Debug)]
pub enum StateError {
    Config(StateConfigError),
    ObservabilityConfig(ObservabilityConfigError),
    Observability(ObservabilityError),
}

impl std::fmt::Display for StateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StateError::Config(err) => write!(f, "state config error: {err}"),
            StateError::ObservabilityConfig(err) => {
                write!(f, "state observability config error: {err}")
            }
            StateError::Observability(err) => write!(f, "state observability error: {err}"),
        }
    }
}

impl std::error::Error for StateError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            StateError::Config(err) => Some(err),
            StateError::ObservabilityConfig(err) => Some(err),
            StateError::Observability(err) => Some(err),
        }
    }
}

pub fn init_observability() -> Result<ObservabilityHandle, StateError> {
    let config = gittree_observability::ObservabilityConfig::from_env("gittree-state")
        .map_err(StateError::ObservabilityConfig)?;
    let handle = gittree_observability::init(&config).map_err(StateError::Observability)?;
    Ok(handle)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StateResponse {
    pub event_id: String,
    pub pubkey: String,
    pub identifier: String,
    pub created_at: i64,
    pub state: HashMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MaintainersResponse {
    pub identifier: String,
    pub maintainers: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RelayCompatibilityResponse {
    pub relay_url: String,
    pub compatible: bool,
    pub supported_capabilities: Vec<String>,
    pub missing_required: Vec<String>,
    pub missing_optional: Vec<String>,
    pub checked_at: i64,
}

#[derive(Debug, Clone)]
pub struct StateCacheConfig {
    pub ttl: Option<Duration>,
    pub max_entries: usize,
}

impl StateCacheConfig {
    pub fn new(ttl: Option<Duration>, max_entries: usize) -> Self {
        Self { ttl, max_entries }
    }
}

impl Default for StateCacheConfig {
    fn default() -> Self {
        Self {
            ttl: Some(Duration::from_secs(30)),
            max_entries: 1024,
        }
    }
}

#[derive(Debug, Clone)]
struct CacheEntry<T> {
    value: T,
    stored_at: Instant,
}

#[derive(Debug)]
pub struct StateCache {
    config: StateCacheConfig,
    state_entries: std::sync::RwLock<HashMap<String, CacheEntry<StateResponse>>>,
    maintainer_entries: std::sync::RwLock<HashMap<String, CacheEntry<MaintainersResponse>>>,
    relay_entries: std::sync::RwLock<HashMap<String, CacheEntry<RelayCompatibilityResponse>>>,
}

impl StateCache {
    pub fn new(config: StateCacheConfig) -> Self {
        Self {
            config,
            state_entries: std::sync::RwLock::new(HashMap::new()),
            maintainer_entries: std::sync::RwLock::new(HashMap::new()),
            relay_entries: std::sync::RwLock::new(HashMap::new()),
        }
    }

    pub fn key(pubkey_hex: &str, identifier: &str) -> String {
        format!("{pubkey_hex}:{identifier}")
    }

    pub fn relay_key(relay_url: &str) -> String {
        relay_url.to_string()
    }

    fn cache_enabled(&self) -> bool {
        self.config.max_entries > 0
    }

    fn is_fresh<T>(&self, entry: &CacheEntry<T>) -> bool {
        match self.config.ttl {
            Some(ttl) => entry.stored_at.elapsed() < ttl,
            None => true,
        }
    }

    fn evict_if_needed<K, V>(&self, map: &mut HashMap<K, CacheEntry<V>>)
    where
        K: Clone + Eq + std::hash::Hash,
    {
        let max_entries = self.config.max_entries;
        if max_entries == 0 {
            map.clear();
            return;
        }

        while map.len() > max_entries {
            let Some(oldest) = map
                .iter()
                .min_by_key(|(_, entry)| entry.stored_at)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            map.remove(&oldest);
        }
    }

    pub fn get_state(&self, key: &str) -> Option<StateResponse> {
        if !self.cache_enabled() {
            return None;
        }
        let cached = {
            let entries = self.state_entries.read().ok()?;
            entries.get(key).cloned()
        };
        match cached {
            Some(entry) if self.is_fresh(&entry) => Some(entry.value),
            Some(_) => {
                if let Ok(mut entries) = self.state_entries.write() {
                    entries.remove(key);
                }
                None
            }
            None => None,
        }
    }

    pub fn insert_state(&self, key: String, value: StateResponse) {
        if !self.cache_enabled() {
            return;
        }
        if let Ok(mut entries) = self.state_entries.write() {
            entries.insert(
                key,
                CacheEntry {
                    value,
                    stored_at: Instant::now(),
                },
            );
            self.evict_if_needed(&mut entries);
        }
    }

    pub fn get_maintainers(&self, key: &str) -> Option<MaintainersResponse> {
        if !self.cache_enabled() {
            return None;
        }
        let cached = {
            let entries = self.maintainer_entries.read().ok()?;
            entries.get(key).cloned()
        };
        match cached {
            Some(entry) if self.is_fresh(&entry) => Some(entry.value),
            Some(_) => {
                if let Ok(mut entries) = self.maintainer_entries.write() {
                    entries.remove(key);
                }
                None
            }
            None => None,
        }
    }

    pub fn insert_maintainers(&self, key: String, value: MaintainersResponse) {
        if !self.cache_enabled() {
            return;
        }
        if let Ok(mut entries) = self.maintainer_entries.write() {
            entries.insert(
                key,
                CacheEntry {
                    value,
                    stored_at: Instant::now(),
                },
            );
            self.evict_if_needed(&mut entries);
        }
    }

    pub fn get_relay_compatibility(&self, key: &str) -> Option<RelayCompatibilityResponse> {
        if !self.cache_enabled() {
            return None;
        }
        let cached = {
            let entries = self.relay_entries.read().ok()?;
            entries.get(key).cloned()
        };
        match cached {
            Some(entry) if self.is_fresh(&entry) => Some(entry.value),
            Some(_) => {
                if let Ok(mut entries) = self.relay_entries.write() {
                    entries.remove(key);
                }
                None
            }
            None => None,
        }
    }

    pub fn insert_relay_compatibility(&self, key: String, value: RelayCompatibilityResponse) {
        if !self.cache_enabled() {
            return;
        }
        if let Ok(mut entries) = self.relay_entries.write() {
            entries.insert(
                key,
                CacheEntry {
                    value,
                    stored_at: Instant::now(),
                },
            );
            self.evict_if_needed(&mut entries);
        }
    }
}

#[derive(Debug)]
pub enum StateServiceError {
    InvalidInput { field: &'static str, value: String },
    NotFound { pubkey: String, identifier: String },
    RelayNotFound { relay_url: String },
    Storage(StorageError),
}

impl std::fmt::Display for StateServiceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StateServiceError::InvalidInput { field, value } => {
                write!(f, "invalid {field}: {value}")
            }
            StateServiceError::NotFound { pubkey, identifier } => {
                write!(f, "state not found for {pubkey}:{identifier}")
            }
            StateServiceError::RelayNotFound { relay_url } => {
                write!(f, "relay compatibility not found for {relay_url}")
            }
            StateServiceError::Storage(err) => write!(f, "storage error: {err}"),
        }
    }
}

impl std::error::Error for StateServiceError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            StateServiceError::Storage(err) => Some(err),
            _ => None,
        }
    }
}

pub async fn latest_state<S>(
    storage: &S,
    pubkey_hex: &str,
    identifier: &str,
) -> Result<StateResponse, StateServiceError>
where
    S: StateRepository,
{
    if identifier.trim().is_empty() {
        return Err(StateServiceError::InvalidInput {
            field: "identifier",
            value: identifier.to_string(),
        });
    }

    let filter =
        RepoFilter::from_hex(pubkey_hex, identifier).map_err(StateServiceError::Storage)?;
    let record = storage
        .latest_state(&filter.pubkey, &filter.identifier)
        .await
        .map_err(StateServiceError::Storage)?;

    let record = record.ok_or_else(|| StateServiceError::NotFound {
        pubkey: pubkey_hex.to_string(),
        identifier: identifier.to_string(),
    })?;

    let state = record.state_map().map_err(StateServiceError::Storage)?;

    Ok(StateResponse {
        event_id: hex::encode(record.event_id),
        pubkey: hex::encode(record.pubkey),
        identifier: record.identifier,
        created_at: record.created_at,
        state,
    })
}

pub async fn latest_state_cached<S>(
    storage: &S,
    cache: &StateCache,
    pubkey_hex: &str,
    identifier: &str,
) -> Result<StateResponse, StateServiceError>
where
    S: StateRepository,
{
    let key = StateCache::key(pubkey_hex, identifier);
    if let Some(value) = cache.get_state(&key) {
        return Ok(value);
    }
    let response = latest_state(storage, pubkey_hex, identifier).await?;
    cache.insert_state(key, response.clone());
    Ok(response)
}

pub async fn resolve_maintainers<S>(
    storage: &S,
    pubkey_hex: &str,
    identifier: &str,
) -> Result<MaintainersResponse, StateServiceError>
where
    S: AnnouncementRepository,
{
    if identifier.trim().is_empty() {
        return Err(StateServiceError::InvalidInput {
            field: "identifier",
            value: identifier.to_string(),
        });
    }

    let mut pending = vec![pubkey_hex.to_string()];
    let mut seen = std::collections::HashSet::new();

    while let Some(pubkey) = pending.pop() {
        if seen.contains(&pubkey) {
            continue;
        }

        let filter =
            RepoFilter::from_hex(&pubkey, identifier).map_err(StateServiceError::Storage)?;
        let announcement = storage
            .latest_announcement(&filter.pubkey, &filter.identifier)
            .await
            .map_err(StateServiceError::Storage)?;

        let Some(announcement) = announcement else {
            continue;
        };

        seen.insert(pubkey);

        for maintainer in announcement.maintainers {
            if !seen.contains(&maintainer) {
                pending.push(maintainer);
            }
        }
    }

    let mut maintainers: Vec<String> = seen.into_iter().collect();
    maintainers.sort();

    Ok(MaintainersResponse {
        identifier: identifier.to_string(),
        maintainers,
    })
}

pub async fn resolve_maintainers_cached<S>(
    storage: &S,
    cache: &StateCache,
    pubkey_hex: &str,
    identifier: &str,
) -> Result<MaintainersResponse, StateServiceError>
where
    S: AnnouncementRepository,
{
    let key = StateCache::key(pubkey_hex, identifier);
    if let Some(value) = cache.get_maintainers(&key) {
        return Ok(value);
    }
    let response = resolve_maintainers(storage, pubkey_hex, identifier).await?;
    cache.insert_maintainers(key, response.clone());
    Ok(response)
}

pub async fn relay_compatibility<S>(
    storage: &S,
    relay_url: &str,
) -> Result<RelayCompatibilityResponse, StateServiceError>
where
    S: RelayCompatibilityRepository,
{
    if relay_url.trim().is_empty() {
        return Err(StateServiceError::InvalidInput {
            field: "relay_url",
            value: relay_url.to_string(),
        });
    }

    let record = storage
        .relay_compatibility(relay_url)
        .await
        .map_err(StateServiceError::Storage)?;
    let record = record.ok_or_else(|| StateServiceError::RelayNotFound {
        relay_url: relay_url.to_string(),
    })?;

    Ok(RelayCompatibilityResponse {
        relay_url: record.relay_url,
        compatible: record.compatible,
        supported_capabilities: record.supported_capabilities,
        missing_required: record.missing_required,
        missing_optional: record.missing_optional,
        checked_at: record.checked_at,
    })
}

pub async fn relay_compatibility_cached<S>(
    storage: &S,
    cache: &StateCache,
    relay_url: &str,
) -> Result<RelayCompatibilityResponse, StateServiceError>
where
    S: RelayCompatibilityRepository,
{
    let key = StateCache::relay_key(relay_url);
    if let Some(value) = cache.get_relay_compatibility(&key) {
        return Ok(value);
    }
    let response = relay_compatibility(storage, relay_url).await?;
    cache.insert_relay_compatibility(key, response.clone());
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::ENV_STORAGE_READ_URL;
    use super::ObservabilityHandle;
    use super::StateCache;
    use super::StateCacheConfig;
    use super::StateConfig;
    use super::StateServiceError;
    use super::StorageConfigError;
    use super::init_observability;
    use super::latest_state;
    use super::latest_state_cached;
    use super::relay_compatibility;
    use super::relay_compatibility_cached;
    use super::resolve_maintainers;
    use super::resolve_maintainers_cached;
    use gittree_core::RepoAnnouncement;
    use gittree_core::RepoState;
    use gittree_core::{RelayCapability, RelayCompatibilityReport};
    use gittree_storage::AnnouncementRepository;
    use gittree_storage::InMemoryRepositories;
    use gittree_storage::RelayCompatibilityRecord;
    use gittree_storage::RelayCompatibilityRepository;
    use gittree_storage::RepoAnnouncementRecord;
    use gittree_storage::StateRepository;
    use std::collections::HashMap;
    use std::sync::Mutex;
    use std::sync::OnceLock;
    use std::time::Duration;

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

    fn sample_compat_record(relay_url: &str) -> RelayCompatibilityRecord {
        let report = RelayCompatibilityReport {
            relay_url: relay_url.to_string(),
            supported: vec![RelayCapability::Nip01, RelayCapability::Nip34],
            missing_required: Vec::new(),
            missing_optional: Vec::new(),
        };
        RelayCompatibilityRecord::new(&report, 123, &gittree_storage::RelayProbeMetadata::default())
            .expect("record")
    }

    #[test]
    fn config_loads_from_env() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var(
            ENV_STORAGE_READ_URL,
            "postgres://user:pass@localhost:5432/gittree",
            || {
                with_env_var("GITTREE_RELAY_URLS", "wss://relay.example", || {
                    let config = StateConfig::from_env().expect("config");
                    assert_eq!(config.bind, "127.0.0.1:8082");
                    assert_eq!(
                        config.storage.read_connection,
                        "postgres://user:pass@localhost:5432/gittree"
                    );
                    assert_eq!(
                        config.relay_urls,
                        vec!["wss://relay.example".to_string()]
                    );
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
                        let config = StateConfig::from_env().expect("config");
                        assert_eq!(config.storage.idle_timeout_secs, None);
                        assert_eq!(config.storage.max_lifetime_secs, None);
                    });
                });
            },
        );
    }

    #[test]
    fn config_requires_storage_url() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        without_env_var(ENV_STORAGE_READ_URL, || {
            let err = StateConfig::from_env().unwrap_err();
            assert!(matches!(
                err,
                super::StateConfigError::Storage(StorageConfigError::MissingEnv(_))
            ));
        });
    }

    #[tokio::test]
    async fn latest_state_returns_record() {
        let repo = InMemoryRepositories::default();
        let mut state_map = HashMap::new();
        state_map.insert("refs/heads/main".to_string(), "a".repeat(40));
        state_map.insert("HEAD".to_string(), "ref: refs/heads/main".to_string());
        let state = RepoState {
            identifier: "repo".to_string(),
            state: state_map,
        };
        let record =
            gittree_storage::RepoStateRecord::new(&"11".repeat(32), &"22".repeat(32), 100, &state)
                .expect("record");
        repo.insert_state(record).await.expect("insert");

        let response = latest_state(&repo, &"22".repeat(32), "repo")
            .await
            .expect("response");
        assert_eq!(response.identifier, "repo");
        assert_eq!(response.created_at, 100);
        assert!(response.state.contains_key("refs/heads/main"));
    }

    #[tokio::test]
    async fn latest_state_reports_missing() {
        let repo = InMemoryRepositories::default();
        let err = latest_state(&repo, &"22".repeat(32), "repo")
            .await
            .unwrap_err();
        assert!(matches!(err, StateServiceError::NotFound { .. }));
    }

    #[tokio::test]
    async fn relay_compatibility_returns_record() {
        let repo = InMemoryRepositories::default();
        let record = sample_compat_record("wss://relay.example");
        repo.upsert_relay_compatibility(record).await.expect("upsert");

        let response = relay_compatibility(&repo, "wss://relay.example")
            .await
            .expect("response");
        assert_eq!(response.relay_url, "wss://relay.example");
        assert!(response.compatible);
        assert_eq!(response.checked_at, 123);
    }

    #[tokio::test]
    async fn relay_compatibility_reports_missing() {
        let repo = InMemoryRepositories::default();
        let err = relay_compatibility(&repo, "wss://relay.example")
            .await
            .unwrap_err();
        assert!(matches!(err, StateServiceError::RelayNotFound { .. }));
    }

    #[tokio::test]
    async fn resolve_maintainers_recurses() {
        let repo = InMemoryRepositories::default();
        let announcement = RepoAnnouncement {
            identifier: "repo".to_string(),
            name: None,
            description: None,
            root_commit: None,
            clone: vec!["https://git.example/repo.git".to_string()],
            web: Vec::new(),
            relays: vec!["wss://relay.example".to_string()],
            blossoms: Vec::new(),
            hashtags: Vec::new(),
            maintainers: vec!["22".repeat(32)],
        };
        let record =
            RepoAnnouncementRecord::new(&"aa".repeat(32), &"11".repeat(32), 10, &announcement)
                .expect("record");
        repo.insert_announcement(record).await.expect("insert root");

        let secondary = RepoAnnouncement {
            identifier: "repo".to_string(),
            name: None,
            description: None,
            root_commit: None,
            clone: vec!["https://git.example/repo.git".to_string()],
            web: Vec::new(),
            relays: vec!["wss://relay.example".to_string()],
            blossoms: Vec::new(),
            hashtags: Vec::new(),
            maintainers: vec!["33".repeat(32)],
        };
        let record =
            RepoAnnouncementRecord::new(&"bb".repeat(32), &"22".repeat(32), 11, &secondary)
                .expect("record");
        repo.insert_announcement(record)
            .await
            .expect("insert maintainer");

        let tertiary = RepoAnnouncement {
            identifier: "repo".to_string(),
            name: None,
            description: None,
            root_commit: None,
            clone: vec!["https://git.example/repo.git".to_string()],
            web: Vec::new(),
            relays: vec!["wss://relay.example".to_string()],
            blossoms: Vec::new(),
            hashtags: Vec::new(),
            maintainers: Vec::new(),
        };
        let record = RepoAnnouncementRecord::new(&"cc".repeat(32), &"33".repeat(32), 12, &tertiary)
            .expect("record");
        repo.insert_announcement(record).await.expect("insert leaf");

        let response = resolve_maintainers(&repo, &"11".repeat(32), "repo")
            .await
            .expect("maintainers");
        assert_eq!(
            response.maintainers,
            vec!["11".repeat(32), "22".repeat(32), "33".repeat(32)]
        );
    }

    #[tokio::test]
    async fn cache_returns_state_response() {
        let repo = InMemoryRepositories::default();
        let cache = StateCache::new(StateCacheConfig::new(None, 10));
        let mut state_map = HashMap::new();
        state_map.insert("refs/heads/main".to_string(), "a".repeat(40));
        state_map.insert("HEAD".to_string(), "ref: refs/heads/main".to_string());
        let state = RepoState {
            identifier: "repo".to_string(),
            state: state_map,
        };
        let record =
            gittree_storage::RepoStateRecord::new(&"11".repeat(32), &"22".repeat(32), 100, &state)
                .expect("record");
        repo.insert_state(record).await.expect("insert");

        let first = latest_state_cached(&repo, &cache, &"22".repeat(32), "repo")
            .await
            .expect("first");
        let second = latest_state_cached(&repo, &cache, &"22".repeat(32), "repo")
            .await
            .expect("second");
        assert_eq!(first, second);
    }

    #[tokio::test]
    async fn cache_returns_relay_compatibility() {
        let repo = InMemoryRepositories::default();
        let cache = StateCache::new(StateCacheConfig::new(None, 10));
        let record = sample_compat_record("wss://relay.example");
        repo.upsert_relay_compatibility(record).await.expect("upsert");

        let first = relay_compatibility_cached(&repo, &cache, "wss://relay.example")
            .await
            .expect("first");
        let second = relay_compatibility_cached(&repo, &cache, "wss://relay.example")
            .await
            .expect("second");
        assert_eq!(first, second);
    }

    #[test]
    fn cache_respects_ttl() {
        let cache = StateCache::new(StateCacheConfig::new(Some(Duration::from_millis(1)), 10));
        let response = super::StateResponse {
            event_id: "aa".to_string(),
            pubkey: "bb".to_string(),
            identifier: "repo".to_string(),
            created_at: 1,
            state: HashMap::new(),
        };
        let key = StateCache::key("bb", "repo");
        cache.insert_state(key.clone(), response);
        std::thread::sleep(Duration::from_millis(2));
        assert!(cache.get_state(&key).is_none());
    }

    #[tokio::test]
    async fn cache_returns_maintainers_response() {
        let repo = InMemoryRepositories::default();
        let cache = StateCache::new(StateCacheConfig::new(None, 10));
        let announcement = RepoAnnouncement {
            identifier: "repo".to_string(),
            name: None,
            description: None,
            root_commit: None,
            clone: vec!["https://git.example/repo.git".to_string()],
            web: Vec::new(),
            relays: vec!["wss://relay.example".to_string()],
            blossoms: Vec::new(),
            hashtags: Vec::new(),
            maintainers: Vec::new(),
        };
        let record =
            RepoAnnouncementRecord::new(&"aa".repeat(32), &"11".repeat(32), 10, &announcement)
                .expect("record");
        repo.insert_announcement(record).await.expect("insert root");

        let first = resolve_maintainers_cached(&repo, &cache, &"11".repeat(32), "repo")
            .await
            .expect("first");
        let second = resolve_maintainers_cached(&repo, &cache, &"11".repeat(32), "repo")
            .await
            .expect("second");
        assert_eq!(first, second);
    }

    #[test]
    fn observability_init_returns_registry() {
        let handle = OBSERVABILITY.get_or_init(|| init_observability().expect("init"));
        assert!(handle.prometheus_registry().is_some());
    }
}
