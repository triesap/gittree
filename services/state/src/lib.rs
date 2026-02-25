use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use gittree_config::{ConfigError, RelayTargetsConfig, ServicesConfig};
use gittree_observability::{ObservabilityConfigError, ObservabilityError, ObservabilityHandle};
use gittree_storage::{
    AnnouncementRepository, CachedRepositories, PostgresRepositories, RelayCompatibilityRepository,
    RepoFilter, StateRepository, StorageConfig, StorageError,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::future::{Future, IntoFuture};
use std::sync::Arc;
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
    Storage(StorageError),
    Serve(String),
}

impl std::fmt::Display for StateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StateError::Config(err) => write!(f, "state config error: {err}"),
            StateError::ObservabilityConfig(err) => {
                write!(f, "state observability config error: {err}")
            }
            StateError::Observability(err) => write!(f, "state observability error: {err}"),
            StateError::Storage(err) => write!(f, "state storage error: {err}"),
            StateError::Serve(err) => write!(f, "state serve error: {err}"),
        }
    }
}

impl std::error::Error for StateError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            StateError::Config(err) => Some(err),
            StateError::ObservabilityConfig(err) => Some(err),
            StateError::Observability(err) => Some(err),
            StateError::Storage(err) => Some(err),
            StateError::Serve(_) => None,
        }
    }
}

pub fn init_observability() -> Result<ObservabilityHandle, StateError> {
    let config = gittree_observability::ObservabilityConfig::from_env("gittree-state")
        .map_err(StateError::ObservabilityConfig)?;
    let handle = gittree_observability::init(&config).map_err(StateError::Observability)?;
    Ok(handle)
}

pub type StateRepositories = CachedRepositories<PostgresRepositories>;

pub fn build_repositories(config: &StateConfig) -> Result<StateRepositories, StateError> {
    let pool_options = config.storage.pool_options().map_err(StateError::Storage)?;
    let connect_options = config
        .storage
        .read_connect_options()
        .map_err(StateError::Storage)?;
    let pool = pool_options.connect_lazy_with(connect_options);
    let repos = PostgresRepositories::new(pool);
    Ok(CachedRepositories::new(repos))
}

struct StateAppState<R> {
    repositories: Arc<R>,
    cache: Arc<StateCache>,
}

impl<R> Clone for StateAppState<R> {
    fn clone(&self) -> Self {
        Self {
            repositories: Arc::clone(&self.repositories),
            cache: Arc::clone(&self.cache),
        }
    }
}

pub async fn serve(config: StateConfig) -> Result<(), StateError> {
    serve_with(config, init_observability, run_axum_server).await
}

async fn serve_with<InitFn, InitValue, ServerFn, ServerFut>(
    config: StateConfig,
    init_observability_fn: InitFn,
    run_server: ServerFn,
) -> Result<(), StateError>
where
    InitFn: FnOnce() -> Result<InitValue, StateError>,
    ServerFn: FnOnce(tokio::net::TcpListener, Router) -> ServerFut,
    ServerFut: Future<Output = Result<(), std::io::Error>>,
{
    let _observability = init_observability_fn()?;
    let repositories = build_repositories(&config)?;
    let cache = Arc::new(StateCache::new(StateCacheConfig::default()));
    let state = StateAppState {
        repositories: Arc::new(repositories),
        cache,
    };
    let router = build_router(state);
    let listener = tokio::net::TcpListener::bind(&config.bind)
        .await
        .map_err(|err| StateError::Serve(err.to_string()))?;
    run_server(listener, router)
        .await
        .map_err(|err| StateError::Serve(err.to_string()))?;
    Ok(())
}

fn run_axum_server(
    listener: tokio::net::TcpListener,
    router: Router,
) -> impl Future<Output = Result<(), std::io::Error>> + Send + 'static {
    axum::serve(listener, router).into_future()
}

fn build_router<R>(state: StateAppState<R>) -> Router
where
    R: AnnouncementRepository
        + RelayCompatibilityRepository
        + StateRepository
        + Send
        + Sync
        + 'static,
{
    Router::new()
        .route("/health", get(health_handler))
        .route("/relay-compatibility", get(relay_compatibility_handler))
        .with_state(state)
}

async fn health_handler() -> &'static str {
    "ok"
}

async fn relay_compatibility_handler<R>(
    State(state): State<StateAppState<R>>,
    Query(query): Query<RelayCompatQuery>,
) -> Result<Json<RelayCompatibilityResponse>, StateHttpError>
where
    R: RelayCompatibilityRepository + Send + Sync,
{
    let response = relay_compatibility_cached(
        state.repositories.as_ref(),
        state.cache.as_ref(),
        &query.relay_url,
    )
    .await?;
    Ok(Json(response))
}

#[derive(Debug)]
enum StateHttpError {
    BadRequest(String),
    NotFound(String),
    Storage(String),
}

impl From<StateServiceError> for StateHttpError {
    fn from(err: StateServiceError) -> Self {
        match err {
            StateServiceError::InvalidInput { field, value } => {
                StateHttpError::BadRequest(format!("{field}: {value}"))
            }
            StateServiceError::NotFound { pubkey, identifier } => {
                StateHttpError::NotFound(format!("{pubkey}:{identifier}"))
            }
            StateServiceError::RelayNotFound { relay_url } => StateHttpError::NotFound(relay_url),
            StateServiceError::Storage(err) => StateHttpError::Storage(err.to_string()),
        }
    }
}

impl IntoResponse for StateHttpError {
    fn into_response(self) -> Response {
        match self {
            StateHttpError::BadRequest(message) => {
                (StatusCode::BAD_REQUEST, message).into_response()
            }
            StateHttpError::NotFound(message) => (StatusCode::NOT_FOUND, message).into_response(),
            StateHttpError::Storage(message) => {
                (StatusCode::INTERNAL_SERVER_ERROR, message).into_response()
            }
        }
    }
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nip11_url: Option<String>,
    pub nip11_available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_probe_ok: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_probe_error: Option<String>,
    pub checked_at: i64,
}

#[derive(Debug, Deserialize)]
struct RelayCompatQuery {
    relay_url: String,
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
        while map.len() > max_entries {
            let oldest = map
                .iter()
                .min_by_key(|(_, entry)| entry.stored_at)
                .map(|(key, _)| key.clone())
                .expect("map has entries while len exceeds max");
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
        nip11_url: record.nip11_url,
        nip11_available: record.nip11_available,
        active_probe_ok: record.active_probe_ok,
        active_probe_error: record.active_probe_error,
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
    use super::StateCache;
    use super::StateCacheConfig;
    use super::StateConfig;
    use super::StateConfigError;
    use super::StateError;
    use super::StateServiceError;
    use super::StorageConfigError;
    use super::init_observability;
    use super::latest_state;
    use super::latest_state_cached;
    use super::relay_compatibility;
    use super::relay_compatibility_cached;
    use super::resolve_maintainers;
    use super::resolve_maintainers_cached;
    use axum::Router;
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use axum::response::IntoResponse;
    use gittree_core::RepoAnnouncement;
    use gittree_core::RepoState;
    use gittree_core::{RelayCapability, RelayCompatibilityReport};
    use gittree_storage::AnnouncementRepository;
    use gittree_storage::InMemoryRepositories;
    use gittree_storage::RelayCompatibilityRecord;
    use gittree_storage::RelayCompatibilityRepository;
    use gittree_storage::RelayProbeMetadata;
    use gittree_storage::RepoAnnouncementRecord;
    use gittree_storage::StateRepository;
    use gittree_storage::StorageConfig;
    use std::collections::HashMap;
    use std::sync::Mutex;
    use std::time::Duration;
    use tower::ServiceExt;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn with_env_var(key: &str, value: &str, f: &mut dyn FnMut()) {
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

    fn without_env_var(key: &str, f: &mut dyn FnMut()) {
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
        let metadata = gittree_storage::RelayProbeMetadata {
            nip11_url: Some("https://relay.example/".to_string()),
            nip11_available: true,
            active_probe_ok: Some(true),
            active_probe_error: None,
        };
        RelayCompatibilityRecord::new(&report, 123, &metadata).expect("record")
    }

    async fn noop_server(
        _listener: tokio::net::TcpListener,
        _router: Router,
    ) -> Result<(), std::io::Error> {
        Ok(())
    }

    #[derive(Debug, Default)]
    struct ErrorRepositories;

    #[async_trait::async_trait]
    impl StateRepository for ErrorRepositories {
        async fn insert_state(
            &self,
            _record: gittree_storage::RepoStateRecord,
        ) -> Result<(), gittree_storage::StorageError> {
            Ok(())
        }

        async fn latest_state(
            &self,
            _pubkey: &[u8],
            _identifier: &str,
        ) -> Result<Option<gittree_storage::RepoStateRecord>, gittree_storage::StorageError>
        {
            Err(gittree_storage::StorageError::Internal {
                message: "state backend error".to_string(),
            })
        }
    }

    #[async_trait::async_trait]
    impl AnnouncementRepository for ErrorRepositories {
        async fn insert_announcement(
            &self,
            _record: RepoAnnouncementRecord,
        ) -> Result<(), gittree_storage::StorageError> {
            Ok(())
        }

        async fn list_announcements(
            &self,
            _pubkey: &[u8],
            _identifier: &str,
        ) -> Result<Vec<RepoAnnouncementRecord>, gittree_storage::StorageError> {
            Err(gittree_storage::StorageError::Internal {
                message: "announcement backend error".to_string(),
            })
        }

        async fn latest_announcement(
            &self,
            _pubkey: &[u8],
            _identifier: &str,
        ) -> Result<Option<RepoAnnouncementRecord>, gittree_storage::StorageError> {
            Err(gittree_storage::StorageError::Internal {
                message: "announcement backend error".to_string(),
            })
        }
    }

    #[async_trait::async_trait]
    impl RelayCompatibilityRepository for ErrorRepositories {
        async fn upsert_relay_compatibility(
            &self,
            _record: RelayCompatibilityRecord,
        ) -> Result<(), gittree_storage::StorageError> {
            Ok(())
        }

        async fn relay_compatibility(
            &self,
            _relay_url: &str,
        ) -> Result<Option<RelayCompatibilityRecord>, gittree_storage::StorageError> {
            Err(gittree_storage::StorageError::Internal {
                message: "relay compatibility backend error".to_string(),
            })
        }
    }

    #[tokio::test]
    async fn error_repositories_trait_methods_cover_branches() {
        let repo = ErrorRepositories;
        let state = RepoState {
            identifier: "repo".to_string(),
            state: HashMap::new(),
        };
        let state_record =
            gittree_storage::RepoStateRecord::new(&"11".repeat(32), &"22".repeat(32), 1, &state)
                .expect("state record");
        repo.insert_state(state_record).await.expect("insert state");
        let state_err = repo
            .latest_state(&[0u8; 32], "repo")
            .await
            .expect_err("latest state error");
        assert!(matches!(
            state_err,
            gittree_storage::StorageError::Internal { .. }
        ));

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
        let announcement_record =
            RepoAnnouncementRecord::new(&"aa".repeat(32), &"11".repeat(32), 1, &announcement)
                .expect("announcement record");
        repo.insert_announcement(announcement_record)
            .await
            .expect("insert announcement");
        let list_err = repo
            .list_announcements(&[0u8; 32], "repo")
            .await
            .expect_err("list error");
        assert!(matches!(
            list_err,
            gittree_storage::StorageError::Internal { .. }
        ));
        let latest_err = repo
            .latest_announcement(&[0u8; 32], "repo")
            .await
            .expect_err("latest error");
        assert!(matches!(
            latest_err,
            gittree_storage::StorageError::Internal { .. }
        ));

        let compat_record = sample_compat_record("wss://relay.example");
        repo.upsert_relay_compatibility(compat_record)
            .await
            .expect("upsert compatibility");
        let compat_err = repo
            .relay_compatibility("wss://relay.example")
            .await
            .expect_err("compatibility error");
        assert!(matches!(
            compat_err,
            gittree_storage::StorageError::Internal { .. }
        ));
    }

    #[test]
    fn config_loads_from_env() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var(
            ENV_STORAGE_READ_URL,
            "postgres://user:pass@localhost:5432/gittree",
            &mut || {
                with_env_var("GITTREE_RELAY_URLS", "wss://relay.example", &mut || {
                    with_env_var("GITTREE_STATE_BIND", "127.0.0.1:8082", &mut || {
                        let config = StateConfig::from_env().expect("config");
                        assert_eq!(config.bind, "127.0.0.1:8082");
                        assert_eq!(
                            config.storage.read_connection,
                            "postgres://user:pass@localhost:5432/gittree"
                        );
                        assert_eq!(config.relay_urls, vec!["wss://relay.example".to_string()]);
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
            &mut || {
                with_env_var("GITTREE_STATE_BIND", "127.0.0.1:8082", &mut || {
                    with_env_var(super::ENV_STORAGE_IDLE_TIMEOUT_SECS, "", &mut || {
                        with_env_var(super::ENV_STORAGE_MAX_LIFETIME_SECS, "", &mut || {
                            let config = StateConfig::from_env().expect("config");
                            assert_eq!(config.storage.idle_timeout_secs, None);
                            assert_eq!(config.storage.max_lifetime_secs, None);
                        });
                    });
                });
            },
        );
    }

    #[test]
    fn config_requires_storage_url() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var("GITTREE_STATE_BIND", "127.0.0.1:8082", &mut || {
            with_env_var("GITTREE_RELAY_URLS", "wss://relay.example", &mut || {
                without_env_var(ENV_STORAGE_READ_URL, &mut || {
                    let err = StateConfig::from_env().unwrap_err();
                    assert!(matches!(
                        err,
                        super::StateConfigError::Storage(StorageConfigError::MissingEnv(_))
                    ));
                });
            });
        });
    }

    #[test]
    fn config_rejects_invalid_numeric_storage_values() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var(
            ENV_STORAGE_READ_URL,
            "postgres://user:pass@localhost:5432/gittree",
            &mut || {
                with_env_var("GITTREE_RELAY_URLS", "wss://relay.example", &mut || {
                    with_env_var(super::ENV_STORAGE_MAX_CONNECTIONS, "oops", &mut || {
                        let err = StateConfig::from_env().expect_err("invalid max connections");
                        assert!(matches!(
                            err,
                            StateConfigError::Storage(StorageConfigError::InvalidEnv {
                                key: super::ENV_STORAGE_MAX_CONNECTIONS,
                                ..
                            })
                        ));
                    });
                    with_env_var(super::ENV_STORAGE_IDLE_TIMEOUT_SECS, "nope", &mut || {
                        let err = StateConfig::from_env().expect_err("invalid idle timeout");
                        assert!(matches!(
                            err,
                            StateConfigError::Storage(StorageConfigError::InvalidEnv {
                                key: super::ENV_STORAGE_IDLE_TIMEOUT_SECS,
                                ..
                            })
                        ));
                    });
                });
            },
        );
    }

    #[test]
    fn config_rejects_invalid_storage_bounds() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var(
            ENV_STORAGE_READ_URL,
            "postgres://user:pass@localhost:5432/gittree",
            &mut || {
                with_env_var("GITTREE_RELAY_URLS", "wss://relay.example", &mut || {
                    with_env_var(super::ENV_STORAGE_MAX_CONNECTIONS, "1", &mut || {
                        with_env_var(super::ENV_STORAGE_MIN_CONNECTIONS, "2", &mut || {
                            let err = StateConfig::from_env().expect_err("invalid bounds");
                            assert!(matches!(
                                err,
                                StateConfigError::Storage(StorageConfigError::InvalidConfig(_))
                            ));
                        });
                    });
                });
            },
        );
    }

    #[test]
    fn config_uses_default_min_connections_when_empty() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var(
            ENV_STORAGE_READ_URL,
            "postgres://user:pass@localhost:5432/gittree",
            &mut || {
                with_env_var("GITTREE_RELAY_URLS", "wss://relay.example", &mut || {
                    with_env_var(super::ENV_STORAGE_MIN_CONNECTIONS, "", &mut || {
                        let config = StateConfig::from_env().expect("config");
                        assert_eq!(config.storage.min_connections, 2);
                    });
                });
            },
        );
    }

    #[test]
    fn config_maps_services_and_relay_target_errors() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var(
            ENV_STORAGE_READ_URL,
            "postgres://user:pass@localhost:5432/gittree",
            &mut || {
                with_env_var("GITTREE_RELAY_URLS", "wss://relay.example", &mut || {
                    with_env_var("GITTREE_STATE_BIND", "invalid-bind", &mut || {
                        let err = StateConfig::from_env().expect_err("invalid bind");
                        assert!(matches!(err, StateConfigError::Config(_)));
                    });
                });
                with_env_var("GITTREE_STATE_BIND", "127.0.0.1:8082", &mut || {
                    with_env_var("GITTREE_RELAY_URLS", "not-a-url", &mut || {
                        let err = StateConfig::from_env().expect_err("invalid relay targets");
                        assert!(matches!(err, StateConfigError::Config(_)));
                    });
                });
            },
        );
    }

    #[test]
    fn config_error_display_and_source_are_stable() {
        let cfg = StateConfigError::Config(gittree_config::ConfigError::MissingEnv(
            "GITTREE_RELAY_URLS",
        ));
        assert!(cfg.to_string().contains("state config error"));
        assert!(std::error::Error::source(&cfg).is_some());

        let storage = StateConfigError::Storage(StorageConfigError::InvalidConfig(
            "pool bounds invalid".to_string(),
        ));
        assert!(storage.to_string().contains("state storage config error"));
        assert!(std::error::Error::source(&storage).is_some());
    }

    #[test]
    fn state_and_service_error_display_and_source_are_stable() {
        let state_config = StateError::Config(StateConfigError::Storage(
            StorageConfigError::InvalidConfig("bad".to_string()),
        ));
        assert!(state_config.to_string().contains("state config error"));
        assert!(std::error::Error::source(&state_config).is_some());

        let state_observability_config = StateError::ObservabilityConfig(
            gittree_observability::ObservabilityConfigError::InvalidEnv {
                key: "GITTREE_LOG_JSON",
                value: "wat".to_string(),
            },
        );
        assert!(
            state_observability_config
                .to_string()
                .contains("state observability config error")
        );
        assert!(std::error::Error::source(&state_observability_config).is_some());

        let state_observability = StateError::Observability(
            gittree_observability::ObservabilityError::SubscriberInit("dup".to_string()),
        );
        assert!(
            state_observability
                .to_string()
                .contains("state observability error")
        );
        assert!(std::error::Error::source(&state_observability).is_some());

        let state_storage = StateError::Storage(gittree_storage::StorageError::Internal {
            message: "fail".to_string(),
        });
        assert!(state_storage.to_string().contains("state storage error"));
        assert!(std::error::Error::source(&state_storage).is_some());

        let state_serve = StateError::Serve("bind failed".to_string());
        assert_eq!(state_serve.to_string(), "state serve error: bind failed");
        assert!(std::error::Error::source(&state_serve).is_none());

        let invalid = StateServiceError::InvalidInput {
            field: "identifier",
            value: "".to_string(),
        };
        assert_eq!(invalid.to_string(), "invalid identifier: ");
        assert!(std::error::Error::source(&invalid).is_none());

        let missing = StateServiceError::NotFound {
            pubkey: "aa".to_string(),
            identifier: "repo".to_string(),
        };
        assert_eq!(missing.to_string(), "state not found for aa:repo");

        let relay_missing = StateServiceError::RelayNotFound {
            relay_url: "wss://relay.example".to_string(),
        };
        assert_eq!(
            relay_missing.to_string(),
            "relay compatibility not found for wss://relay.example"
        );

        let storage = StateServiceError::Storage(gittree_storage::StorageError::Internal {
            message: "fail".to_string(),
        });
        assert!(storage.to_string().contains("storage error"));
        assert!(std::error::Error::source(&storage).is_some());

        let invalid_storage_env = StorageConfigError::InvalidEnv {
            key: "GITTREE_STORAGE_MAX_CONNECTIONS",
            value: "bad".to_string(),
        };
        assert_eq!(
            invalid_storage_env.to_string(),
            "invalid env GITTREE_STORAGE_MAX_CONNECTIONS: bad"
        );
        assert_eq!(
            StorageConfigError::MissingEnv("GITTREE_STORAGE_READ_URL").to_string(),
            "missing env GITTREE_STORAGE_READ_URL"
        );
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
    async fn latest_state_rejects_empty_identifier() {
        let repo = InMemoryRepositories::default();
        let err = latest_state(&repo, &"22".repeat(32), "")
            .await
            .expect_err("invalid identifier");
        assert!(matches!(
            err,
            StateServiceError::InvalidInput {
                field: "identifier",
                ..
            }
        ));
    }

    #[tokio::test]
    async fn latest_state_rejects_invalid_pubkey_hex() {
        let repo = InMemoryRepositories::default();
        let err = latest_state(&repo, "not-hex", "repo")
            .await
            .expect_err("invalid pubkey");
        assert!(matches!(err, StateServiceError::Storage(_)));
    }

    #[tokio::test]
    async fn latest_state_cached_propagates_lookup_errors() {
        let repo = ErrorRepositories;
        let cache = StateCache::new(StateCacheConfig::new(None, 10));
        let err = latest_state_cached(&repo, &cache, &"11".repeat(32), "repo")
            .await
            .expect_err("cached error");
        assert!(matches!(err, StateServiceError::Storage(_)));
    }

    #[tokio::test]
    async fn relay_compatibility_returns_record() {
        let repo = InMemoryRepositories::default();
        let record = sample_compat_record("wss://relay.example");
        repo.upsert_relay_compatibility(record)
            .await
            .expect("upsert");

        let response = relay_compatibility(&repo, "wss://relay.example")
            .await
            .expect("response");
        assert_eq!(response.relay_url, "wss://relay.example");
        assert!(response.compatible);
        assert!(response.nip11_available);
        assert_eq!(response.active_probe_ok, Some(true));
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
    async fn relay_compatibility_rejects_empty_relay_url() {
        let repo = InMemoryRepositories::default();
        let err = relay_compatibility(&repo, " ")
            .await
            .expect_err("invalid relay");
        assert!(matches!(
            err,
            StateServiceError::InvalidInput {
                field: "relay_url",
                ..
            }
        ));
    }

    #[tokio::test]
    async fn relay_compatibility_maps_repository_errors() {
        let repo = ErrorRepositories;
        let err = relay_compatibility(&repo, "wss://relay.example")
            .await
            .expect_err("storage error");
        assert!(matches!(err, StateServiceError::Storage(_)));
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
    async fn resolve_maintainers_rejects_empty_identifier() {
        let repo = InMemoryRepositories::default();
        let err = resolve_maintainers(&repo, &"11".repeat(32), "")
            .await
            .expect_err("invalid identifier");
        assert!(matches!(
            err,
            StateServiceError::InvalidInput {
                field: "identifier",
                ..
            }
        ));
    }

    #[tokio::test]
    async fn resolve_maintainers_rejects_invalid_pubkey_hex() {
        let repo = InMemoryRepositories::default();
        let err = resolve_maintainers(&repo, "not-hex", "repo")
            .await
            .expect_err("invalid pubkey");
        assert!(matches!(err, StateServiceError::Storage(_)));
    }

    #[tokio::test]
    async fn resolve_maintainers_cached_propagates_lookup_errors() {
        let repo = ErrorRepositories;
        let cache = StateCache::new(StateCacheConfig::new(None, 10));
        let err = resolve_maintainers_cached(&repo, &cache, &"11".repeat(32), "repo")
            .await
            .expect_err("cached error");
        assert!(matches!(err, StateServiceError::Storage(_)));
    }

    #[tokio::test]
    async fn resolve_maintainers_skips_missing_announcements_and_seen_nodes() {
        let repo = InMemoryRepositories::default();
        let root = RepoAnnouncement {
            identifier: "repo".to_string(),
            name: None,
            description: None,
            root_commit: None,
            clone: vec!["https://git.example/repo.git".to_string()],
            web: Vec::new(),
            relays: vec!["wss://relay.example".to_string()],
            blossoms: Vec::new(),
            hashtags: Vec::new(),
            maintainers: vec!["22".repeat(32), "22".repeat(32), "44".repeat(32)],
        };
        let root_record =
            RepoAnnouncementRecord::new(&"aa".repeat(32), &"11".repeat(32), 10, &root)
                .expect("root");
        repo.insert_announcement(root_record)
            .await
            .expect("insert root");

        let child = RepoAnnouncement {
            identifier: "repo".to_string(),
            name: None,
            description: None,
            root_commit: None,
            clone: vec!["https://git.example/repo.git".to_string()],
            web: Vec::new(),
            relays: vec!["wss://relay.example".to_string()],
            blossoms: Vec::new(),
            hashtags: Vec::new(),
            maintainers: vec!["11".repeat(32)],
        };
        let child_record =
            RepoAnnouncementRecord::new(&"bb".repeat(32), &"22".repeat(32), 11, &child)
                .expect("child");
        repo.insert_announcement(child_record)
            .await
            .expect("insert child");

        let response = resolve_maintainers(&repo, &"11".repeat(32), "repo")
            .await
            .expect("maintainers");
        assert_eq!(response.maintainers, vec!["11".repeat(32), "22".repeat(32)]);
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
        repo.upsert_relay_compatibility(record)
            .await
            .expect("upsert");

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

    #[test]
    fn cache_zero_capacity_disables_entries() {
        let cache = StateCache::new(StateCacheConfig::new(Some(Duration::from_secs(30)), 0));
        let key = StateCache::key("bb", "repo");
        let response = super::StateResponse {
            event_id: "aa".to_string(),
            pubkey: "bb".to_string(),
            identifier: "repo".to_string(),
            created_at: 1,
            state: HashMap::new(),
        };
        cache.insert_state(key.clone(), response);
        assert!(cache.get_state(&key).is_none());

        let maintainers_key = StateCache::key("aa", "repo");
        cache.insert_maintainers(
            maintainers_key.clone(),
            super::MaintainersResponse {
                identifier: "repo".to_string(),
                maintainers: vec!["aa".to_string()],
            },
        );
        assert!(cache.get_maintainers(&maintainers_key).is_none());

        let relay_key = StateCache::relay_key("wss://relay.example");
        cache.insert_relay_compatibility(
            relay_key.clone(),
            super::RelayCompatibilityResponse {
                relay_url: "wss://relay.example".to_string(),
                compatible: true,
                supported_capabilities: vec!["nip01".to_string()],
                missing_required: Vec::new(),
                missing_optional: Vec::new(),
                nip11_url: None,
                nip11_available: true,
                active_probe_ok: Some(true),
                active_probe_error: None,
                checked_at: 1,
            },
        );
        assert!(cache.get_relay_compatibility(&relay_key).is_none());
    }

    #[test]
    fn cache_handles_poisoned_locks() {
        let cache = StateCache::new(StateCacheConfig::new(None, 10));
        let key = StateCache::key("aa", "repo");
        let _ = std::panic::catch_unwind(|| {
            let _guard = cache.state_entries.write().expect("lock");
            panic!("poison state");
        });
        assert!(cache.get_state(&key).is_none());
        cache.insert_state(
            key.clone(),
            super::StateResponse {
                event_id: "aa".to_string(),
                pubkey: "aa".to_string(),
                identifier: "repo".to_string(),
                created_at: 1,
                state: HashMap::new(),
            },
        );
        assert!(cache.get_state(&key).is_none());

        let maintainers_key = StateCache::key("bb", "repo");
        let _ = std::panic::catch_unwind(|| {
            let _guard = cache.maintainer_entries.write().expect("lock");
            panic!("poison maintainers");
        });
        assert!(cache.get_maintainers(&maintainers_key).is_none());
        cache.insert_maintainers(
            maintainers_key,
            super::MaintainersResponse {
                identifier: "repo".to_string(),
                maintainers: vec!["bb".to_string()],
            },
        );

        let relay_key = StateCache::relay_key("wss://relay.example");
        let _ = std::panic::catch_unwind(|| {
            let _guard = cache.relay_entries.write().expect("lock");
            panic!("poison relay");
        });
        assert!(cache.get_relay_compatibility(&relay_key).is_none());
        cache.insert_relay_compatibility(
            relay_key,
            super::RelayCompatibilityResponse {
                relay_url: "wss://relay.example".to_string(),
                compatible: true,
                supported_capabilities: Vec::new(),
                missing_required: Vec::new(),
                missing_optional: Vec::new(),
                nip11_url: None,
                nip11_available: false,
                active_probe_ok: None,
                active_probe_error: None,
                checked_at: 1,
            },
        );
    }

    #[test]
    fn cache_evicts_oldest_entries() {
        let cache = StateCache::new(StateCacheConfig::new(Some(Duration::from_secs(30)), 1));
        let old_key = StateCache::key("aa", "repo");
        let new_key = StateCache::key("bb", "repo");
        let old = super::StateResponse {
            event_id: "11".to_string(),
            pubkey: "aa".to_string(),
            identifier: "repo".to_string(),
            created_at: 1,
            state: HashMap::new(),
        };
        let new = super::StateResponse {
            event_id: "22".to_string(),
            pubkey: "bb".to_string(),
            identifier: "repo".to_string(),
            created_at: 2,
            state: HashMap::new(),
        };
        cache.insert_state(old_key.clone(), old);
        cache.insert_state(new_key.clone(), new.clone());

        assert!(cache.get_state(&old_key).is_none());
        assert_eq!(cache.get_state(&new_key), Some(new));
    }

    #[test]
    fn cache_respects_ttl_for_maintainers_and_relay_entries() {
        let cache = StateCache::new(StateCacheConfig::new(Some(Duration::from_millis(1)), 2));
        let maintainers_key = StateCache::key("11", "repo");
        let relay_key = StateCache::relay_key("wss://relay.example");
        cache.insert_maintainers(
            maintainers_key.clone(),
            super::MaintainersResponse {
                identifier: "repo".to_string(),
                maintainers: vec!["11".to_string()],
            },
        );
        cache.insert_relay_compatibility(
            relay_key.clone(),
            super::RelayCompatibilityResponse {
                relay_url: "wss://relay.example".to_string(),
                compatible: true,
                supported_capabilities: vec!["nip01".to_string()],
                missing_required: Vec::new(),
                missing_optional: Vec::new(),
                nip11_url: None,
                nip11_available: true,
                active_probe_ok: Some(true),
                active_probe_error: None,
                checked_at: 1,
            },
        );
        std::thread::sleep(Duration::from_millis(2));
        assert!(cache.get_maintainers(&maintainers_key).is_none());
        assert!(cache.get_relay_compatibility(&relay_key).is_none());
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

    #[tokio::test]
    async fn health_endpoint_returns_ok() {
        let repositories = std::sync::Arc::new(InMemoryRepositories::new());
        let cache = std::sync::Arc::new(StateCache::new(StateCacheConfig::default()));
        let app = super::build_router(super::StateAppState {
            repositories,
            cache,
        });
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), axum::http::StatusCode::OK);
    }

    #[tokio::test]
    async fn relay_compatibility_endpoint_returns_record() {
        let repositories = std::sync::Arc::new(InMemoryRepositories::new());
        let report = RelayCompatibilityReport {
            relay_url: "wss://relay.example".to_string(),
            supported: vec![RelayCapability::Nip01, RelayCapability::Nip34],
            missing_required: Vec::new(),
            missing_optional: Vec::new(),
        };
        let record = RelayCompatibilityRecord::new(&report, 10, &RelayProbeMetadata::default())
            .expect("record");
        repositories
            .upsert_relay_compatibility(record)
            .await
            .expect("upsert");

        let cache = std::sync::Arc::new(StateCache::new(StateCacheConfig::default()));
        let app = super::build_router(super::StateAppState {
            repositories,
            cache,
        });
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/relay-compatibility?relay_url=wss://relay.example")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), axum::http::StatusCode::OK);
    }

    #[tokio::test]
    async fn relay_compatibility_endpoint_maps_not_found_and_bad_input() {
        let repositories = std::sync::Arc::new(InMemoryRepositories::new());
        let cache = std::sync::Arc::new(StateCache::new(StateCacheConfig::default()));
        let app = super::build_router(super::StateAppState {
            repositories,
            cache,
        });

        let missing = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/relay-compatibility?relay_url=wss://missing.example")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);
        let missing_body = to_bytes(missing.into_body(), usize::MAX)
            .await
            .expect("body");
        assert!(String::from_utf8_lossy(&missing_body).contains("wss://missing.example"));

        let bad = app
            .oneshot(
                Request::builder()
                    .uri("/relay-compatibility?relay_url=%20")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(bad.status(), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn state_http_error_maps_service_errors() {
        let bad = super::StateHttpError::from(StateServiceError::InvalidInput {
            field: "relay_url",
            value: " ".to_string(),
        })
        .into_response();
        assert_eq!(bad.status(), StatusCode::BAD_REQUEST);

        let not_found = super::StateHttpError::from(StateServiceError::RelayNotFound {
            relay_url: "wss://relay.example".to_string(),
        })
        .into_response();
        assert_eq!(not_found.status(), StatusCode::NOT_FOUND);

        let state_not_found = super::StateHttpError::from(StateServiceError::NotFound {
            pubkey: "aa".to_string(),
            identifier: "repo".to_string(),
        })
        .into_response();
        assert_eq!(state_not_found.status(), StatusCode::NOT_FOUND);

        let storage = super::StateHttpError::from(StateServiceError::Storage(
            gittree_storage::StorageError::Internal {
                message: "fail".to_string(),
            },
        ))
        .into_response();
        assert_eq!(storage.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[tokio::test]
    async fn build_repositories_constructs_lazy_pool() {
        let config = StateConfig {
            bind: "127.0.0.1:0".to_string(),
            storage: StorageConfig {
                read_connection: "postgres://gittree:gittree@127.0.0.1:5432/gittree".to_string(),
                write_connection: None,
                max_connections: 10,
                min_connections: 2,
                idle_timeout_secs: None,
                max_lifetime_secs: None,
                application_name: Some("gittree-state-test".to_string()),
            },
            relay_urls: Vec::new(),
        };

        let repos = super::build_repositories(&config).expect("repositories");
        let _ = repos;
    }

    #[test]
    fn build_repositories_rejects_invalid_connection() {
        let config = StateConfig {
            bind: "127.0.0.1:0".to_string(),
            storage: StorageConfig {
                read_connection: "not-a-postgres-url".to_string(),
                write_connection: None,
                max_connections: 10,
                min_connections: 2,
                idle_timeout_secs: None,
                max_lifetime_secs: None,
                application_name: None,
            },
            relay_urls: Vec::new(),
        };
        let err = super::build_repositories(&config).expect_err("invalid connection");
        assert!(matches!(err, StateError::Storage(_)));
    }

    #[tokio::test]
    async fn serve_reports_bind_error() {
        let config = StateConfig {
            bind: "not-a-bind".to_string(),
            storage: StorageConfig {
                read_connection: "postgres://gittree:gittree@127.0.0.1:5432/gittree".to_string(),
                write_connection: None,
                max_connections: 10,
                min_connections: 2,
                idle_timeout_secs: None,
                max_lifetime_secs: None,
                application_name: Some("gittree-state-test".to_string()),
            },
            relay_urls: Vec::new(),
        };

        let err = super::serve(config).await.expect_err("serve error");
        assert!(matches!(
            err,
            StateError::Serve(_)
                | StateError::Observability(_)
                | StateError::ObservabilityConfig(_)
        ));
    }

    #[tokio::test]
    async fn serve_with_maps_observability_errors() {
        let config = StateConfig {
            bind: "127.0.0.1:0".to_string(),
            storage: StorageConfig {
                read_connection: "postgres://gittree:gittree@127.0.0.1:5432/gittree".to_string(),
                write_connection: None,
                max_connections: 10,
                min_connections: 2,
                idle_timeout_secs: None,
                max_lifetime_secs: None,
                application_name: None,
            },
            relay_urls: Vec::new(),
        };
        let err = super::serve_with(
            config,
            || {
                Err::<(), StateError>(StateError::ObservabilityConfig(
                    gittree_observability::ObservabilityConfigError::InvalidEnv {
                        key: "GITTREE_LOG_JSON",
                        value: "bad".to_string(),
                    },
                ))
            },
            noop_server,
        )
        .await
        .expect_err("observability error");
        assert!(matches!(err, StateError::ObservabilityConfig(_)));
    }

    #[tokio::test]
    async fn serve_with_maps_server_errors() {
        let config = StateConfig {
            bind: "127.0.0.1:0".to_string(),
            storage: StorageConfig {
                read_connection: "postgres://gittree:gittree@127.0.0.1:5432/gittree".to_string(),
                write_connection: None,
                max_connections: 10,
                min_connections: 2,
                idle_timeout_secs: None,
                max_lifetime_secs: None,
                application_name: None,
            },
            relay_urls: Vec::new(),
        };
        let err = super::serve_with(
            config,
            || Ok(()),
            |_listener, _router| async { Err(std::io::Error::other("boom")) },
        )
        .await
        .expect_err("server error");
        assert!(matches!(err, StateError::Serve(message) if message.contains("boom")));
    }

    #[tokio::test]
    async fn serve_with_returns_ok_when_server_returns_ok() {
        let config = StateConfig {
            bind: "127.0.0.1:0".to_string(),
            storage: StorageConfig {
                read_connection: "postgres://gittree:gittree@127.0.0.1:5432/gittree".to_string(),
                write_connection: None,
                max_connections: 10,
                min_connections: 2,
                idle_timeout_secs: None,
                max_lifetime_secs: None,
                application_name: None,
            },
            relay_urls: Vec::new(),
        };
        let result = super::serve_with(config, || Ok(()), noop_server).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn run_axum_server_can_start_and_be_aborted() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let router = Router::new().route("/health", axum::routing::get(super::health_handler));
        let task = tokio::spawn(super::run_axum_server(listener, router));
        tokio::time::sleep(Duration::from_millis(5)).await;
        task.abort();
        let _ = task.await;
    }

    #[test]
    fn env_helpers_restore_existing_values() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        const KEY: &str = "GITTREE_STATE_TEST_RESTORE";
        unsafe {
            std::env::set_var(KEY, "original");
        }
        with_env_var(KEY, "override", &mut || {
            assert_eq!(std::env::var(KEY).ok().as_deref(), Some("override"));
        });
        assert_eq!(std::env::var(KEY).ok().as_deref(), Some("original"));

        with_env_var(KEY, "original", &mut || {
            without_env_var(KEY, &mut || {
                assert!(std::env::var(KEY).is_err());
            });
        });
        assert_eq!(std::env::var(KEY).ok().as_deref(), Some("original"));
        unsafe {
            std::env::remove_var(KEY);
        }
    }

    #[test]
    fn observability_init_reports_config_error() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var("GITTREE_LOG_STDOUT", "invalid-bool", &mut || {
            let err = init_observability().expect_err("invalid observability config");
            assert!(matches!(err, StateError::ObservabilityConfig(_)));
        });
    }
}
