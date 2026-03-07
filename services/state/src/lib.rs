use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use gittree_app_core::{npub_from_bytes, pubkey_bytes_from_npub};
use gittree_config::{ConfigError, RelayTargetsConfig, ServicesConfig};
use gittree_observability::{ObservabilityConfigError, ObservabilityError, ObservabilityHandle};
use gittree_storage::{
    AccountStateRecord, AnnouncementRepository, CachedRepositories, PostgresRepositories,
    ProfileStateRecord, RelayCompatibilityRepository, RepoFilter, RepoStateV1Record,
    StateRepository, StorageConfig, StorageError,
};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::future::{Future, pending};
use std::pin::Pin;
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
trait StateRepositoriesDyn:
    AnnouncementRepository + RelayCompatibilityRepository + StateRepository
{
}

impl<T> StateRepositoriesDyn for T where
    T: AnnouncementRepository + RelayCompatibilityRepository + StateRepository
{
}

type DynStateRepositories = dyn StateRepositoriesDyn + Send + Sync;
type ProjectionAccountFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Option<AccountStateRecord>, StorageError>> + Send + 'a>>;
type ProjectionProfileFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Option<ProfileStateRecord>, StorageError>> + Send + 'a>>;
type ProjectionRepoFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Option<RepoStateV1Record>, StorageError>> + Send + 'a>>;
type ProjectionMaintainersFuture<'a> =
    Pin<Box<dyn Future<Output = Result<HashSet<Vec<u8>>, StorageError>> + Send + 'a>>;

trait ProjectionRepositoriesDyn {
    fn v1_account_state<'a>(&'a self, pubkey: &'a [u8]) -> ProjectionAccountFuture<'a>;
    fn v1_profile_state<'a>(&'a self, pubkey: &'a [u8]) -> ProjectionProfileFuture<'a>;
    fn v1_repo_state<'a>(
        &'a self,
        owner_pubkey: &'a [u8],
        repo_name: &'a str,
    ) -> ProjectionRepoFuture<'a>;
    fn v1_list_active_repo_maintainers<'a>(
        &'a self,
        owner_pubkey: &'a [u8],
        repo_name: &'a str,
    ) -> ProjectionMaintainersFuture<'a>;
}

impl ProjectionRepositoriesDyn for PostgresRepositories {
    fn v1_account_state<'a>(&'a self, pubkey: &'a [u8]) -> ProjectionAccountFuture<'a> {
        Box::pin(PostgresRepositories::v1_account_state(self, pubkey))
    }

    fn v1_profile_state<'a>(&'a self, pubkey: &'a [u8]) -> ProjectionProfileFuture<'a> {
        Box::pin(PostgresRepositories::v1_profile_state(self, pubkey))
    }

    fn v1_repo_state<'a>(
        &'a self,
        owner_pubkey: &'a [u8],
        repo_name: &'a str,
    ) -> ProjectionRepoFuture<'a> {
        Box::pin(PostgresRepositories::v1_repo_state(
            self,
            owner_pubkey,
            repo_name,
        ))
    }

    fn v1_list_active_repo_maintainers<'a>(
        &'a self,
        owner_pubkey: &'a [u8],
        repo_name: &'a str,
    ) -> ProjectionMaintainersFuture<'a> {
        Box::pin(PostgresRepositories::v1_list_active_repo_maintainers(
            self,
            owner_pubkey,
            repo_name,
        ))
    }
}

type DynProjectionRepositories = dyn ProjectionRepositoriesDyn + Send + Sync;
type StateServerFuture = Pin<Box<dyn Future<Output = Result<(), std::io::Error>> + Send>>;
type StateShutdownFuture = Pin<Box<dyn Future<Output = ()> + Send>>;
type BuildRepositoriesFn = fn(&StateConfig) -> Result<StateRepositories, StateError>;
type BuildProjectionRepositoriesFn = fn(&StateConfig) -> Result<PostgresRepositories, StateError>;

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

pub fn build_projection_repositories(
    config: &StateConfig,
) -> Result<PostgresRepositories, StateError> {
    let pool_options = config.storage.pool_options().map_err(StateError::Storage)?;
    let connect_options = config
        .storage
        .read_connect_options()
        .map_err(StateError::Storage)?;
    let pool = pool_options.connect_lazy_with(connect_options);
    Ok(PostgresRepositories::new(pool))
}

struct StateAppState {
    repositories: Arc<DynStateRepositories>,
    projection_repositories: Arc<DynProjectionRepositories>,
    cache: Arc<StateCache>,
}

impl Clone for StateAppState {
    fn clone(&self) -> Self {
        Self {
            repositories: Arc::clone(&self.repositories),
            projection_repositories: Arc::clone(&self.projection_repositories),
            cache: Arc::clone(&self.cache),
        }
    }
}

pub async fn serve(config: StateConfig) -> Result<(), StateError> {
    serve_with(config, init_observability_unit, run_axum_server_boxed).await
}

fn init_observability_unit() -> Result<(), StateError> {
    init_observability()?;
    Ok(())
}

async fn serve_with(
    config: StateConfig,
    init_observability_fn: fn() -> Result<(), StateError>,
    run_server: fn(tokio::net::TcpListener, Router) -> StateServerFuture,
) -> Result<(), StateError> {
    serve_with_components(
        config,
        init_observability_fn,
        build_repositories,
        build_projection_repositories,
        run_server,
    )
    .await
}

async fn serve_with_components(
    config: StateConfig,
    init_observability_fn: fn() -> Result<(), StateError>,
    build_repositories_fn: BuildRepositoriesFn,
    build_projection_repositories_fn: BuildProjectionRepositoriesFn,
    run_server: fn(tokio::net::TcpListener, Router) -> StateServerFuture,
) -> Result<(), StateError> {
    let _observability = init_observability_fn()?;
    let repositories = build_repositories_fn(&config)?;
    let projection_repositories = build_projection_repositories_fn(&config)?;
    let cache = Arc::new(StateCache::new(StateCacheConfig::default()));
    let state = StateAppState {
        repositories: Arc::new(repositories),
        projection_repositories: Arc::new(projection_repositories),
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

fn run_axum_server_boxed(listener: tokio::net::TcpListener, router: Router) -> StateServerFuture {
    run_axum_server_with_shutdown(listener, router, Box::pin(pending()))
}

fn run_axum_server_with_shutdown(
    listener: tokio::net::TcpListener,
    router: Router,
    shutdown: StateShutdownFuture,
) -> StateServerFuture {
    Box::pin(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(shutdown)
            .await
    })
}

fn build_router(state: StateAppState) -> Router {
    Router::new()
        .route("/health", get(health_handler))
        .route("/relay-compatibility", get(relay_compatibility_handler))
        .route("/v1/accounts/:npub", get(account_view_handler))
        .route("/v1/profiles/:npub", get(profile_view_handler))
        .route("/v1/repos/:owner/:repo", get(repo_view_handler))
        .route(
            "/v1/repos/:owner/:repo/maintainers",
            get(repo_maintainers_view_handler),
        )
        .route(
            "/v1/repos/:owner/:repo/activity",
            get(repo_activity_view_handler),
        )
        .with_state(state)
}

async fn health_handler() -> &'static str {
    "ok"
}

async fn relay_compatibility_handler(
    State(state): State<StateAppState>,
    Query(query): Query<RelayCompatQuery>,
) -> Result<Json<RelayCompatibilityResponse>, StateHttpError> {
    let response = relay_compatibility_cached(
        state.repositories.as_ref(),
        state.cache.as_ref(),
        &query.relay_url,
    )
    .await?;
    Ok(Json(response))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct AccountViewResponse {
    npub: String,
    status: String,
    created_at: i64,
    updated_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    deleted_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct ProfileViewResponse {
    npub: String,
    display_name: Option<String>,
    bio: Option<String>,
    avatar_url: Option<String>,
    website_url: Option<String>,
    location: Option<String>,
    visibility: String,
    updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct RepoViewResponse {
    owner: String,
    repo: String,
    description: Option<String>,
    website_url: Option<String>,
    visibility: String,
    default_branch: String,
    archived: bool,
    updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct RepoMaintainersResponse {
    owner: String,
    repo: String,
    maintainers: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct RepoActivityResponse {
    owner: String,
    repo: String,
    activity: Vec<String>,
}

async fn account_view_handler(
    State(state): State<StateAppState>,
    Path(npub): Path<String>,
) -> Result<Json<AccountViewResponse>, StateHttpError> {
    let pubkey = parse_npub_param(&npub)?;
    let Some(record) = state
        .projection_repositories
        .v1_account_state(&pubkey)
        .await
        .map_err(|err| StateHttpError::Storage(err.to_string()))?
    else {
        return Err(StateHttpError::NotFound("account".to_string()));
    };

    Ok(Json(AccountViewResponse {
        npub,
        status: record.status.as_str().to_string(),
        created_at: record.created_at,
        updated_at: record.updated_at,
        deleted_at: record.deleted_at,
    }))
}

async fn profile_view_handler(
    State(state): State<StateAppState>,
    Path(npub): Path<String>,
) -> Result<Json<ProfileViewResponse>, StateHttpError> {
    let pubkey = parse_npub_param(&npub)?;
    let Some(record) = state
        .projection_repositories
        .v1_profile_state(&pubkey)
        .await
        .map_err(|err| StateHttpError::Storage(err.to_string()))?
    else {
        return Err(StateHttpError::NotFound("profile".to_string()));
    };
    if record.visibility.as_str() == "private" {
        return Err(StateHttpError::NotFound("profile".to_string()));
    }

    Ok(Json(ProfileViewResponse {
        npub,
        display_name: record.display_name,
        bio: record.bio,
        avatar_url: record.avatar_url,
        website_url: record.website_url,
        location: record.location,
        visibility: record.visibility.as_str().to_string(),
        updated_at: record.updated_at,
    }))
}

async fn repo_view_handler(
    State(state): State<StateAppState>,
    Path((owner, repo)): Path<(String, String)>,
) -> Result<Json<RepoViewResponse>, StateHttpError> {
    let owner_pubkey = parse_npub_param(&owner)?;
    let Some(record) = state
        .projection_repositories
        .v1_repo_state(&owner_pubkey, &repo)
        .await
        .map_err(|err| StateHttpError::Storage(err.to_string()))?
    else {
        return Err(StateHttpError::NotFound("repo".to_string()));
    };

    Ok(Json(RepoViewResponse {
        owner,
        repo: record.repo_name,
        description: record.description,
        website_url: record.website_url,
        visibility: record.visibility.as_str().to_string(),
        default_branch: record.default_branch,
        archived: record.archived,
        updated_at: record.updated_at,
    }))
}

async fn repo_maintainers_view_handler(
    State(state): State<StateAppState>,
    Path((owner, repo)): Path<(String, String)>,
) -> Result<Json<RepoMaintainersResponse>, StateHttpError> {
    let owner_pubkey = parse_npub_param(&owner)?;
    let maintainers = state
        .projection_repositories
        .v1_list_active_repo_maintainers(&owner_pubkey, &repo)
        .await
        .map_err(|err| StateHttpError::Storage(err.to_string()))?;
    let mut values = maintainers
        .into_iter()
        .filter_map(|maintainer| npub_from_bytes(&maintainer).ok())
        .collect::<Vec<_>>();
    values.sort();

    Ok(Json(RepoMaintainersResponse {
        owner,
        repo,
        maintainers: values,
    }))
}

async fn repo_activity_view_handler(
    Path((owner, repo)): Path<(String, String)>,
) -> Result<Json<RepoActivityResponse>, StateHttpError> {
    Ok(Json(RepoActivityResponse {
        owner,
        repo,
        activity: Vec::new(),
    }))
}

fn parse_npub_param(npub: &str) -> Result<Vec<u8>, StateHttpError> {
    pubkey_bytes_from_npub(npub)
        .map_err(|_| StateHttpError::BadRequest(format!("invalid npub: {npub}")))
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
                let _ = self
                    .state_entries
                    .write()
                    .ok()
                    .map(|mut entries| entries.remove(key));
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
                let _ = self
                    .maintainer_entries
                    .write()
                    .ok()
                    .map(|mut entries| entries.remove(key));
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
                let _ = self
                    .relay_entries
                    .write()
                    .ok()
                    .map(|mut entries| entries.remove(key));
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

pub async fn latest_state(
    storage: &dyn StateRepository,
    pubkey_hex: &str,
    identifier: &str,
) -> Result<StateResponse, StateServiceError> {
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

pub async fn latest_state_cached(
    storage: &dyn StateRepository,
    cache: &StateCache,
    pubkey_hex: &str,
    identifier: &str,
) -> Result<StateResponse, StateServiceError> {
    let key = StateCache::key(pubkey_hex, identifier);
    if let Some(value) = cache.get_state(&key) {
        return Ok(value);
    }
    let response = latest_state(storage, pubkey_hex, identifier).await?;
    cache.insert_state(key, response.clone());
    Ok(response)
}

pub async fn resolve_maintainers(
    storage: &dyn AnnouncementRepository,
    pubkey_hex: &str,
    identifier: &str,
) -> Result<MaintainersResponse, StateServiceError> {
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

pub async fn resolve_maintainers_cached(
    storage: &dyn AnnouncementRepository,
    cache: &StateCache,
    pubkey_hex: &str,
    identifier: &str,
) -> Result<MaintainersResponse, StateServiceError> {
    let key = StateCache::key(pubkey_hex, identifier);
    if let Some(value) = cache.get_maintainers(&key) {
        return Ok(value);
    }
    let response = resolve_maintainers(storage, pubkey_hex, identifier).await?;
    cache.insert_maintainers(key, response.clone());
    Ok(response)
}

pub async fn relay_compatibility(
    storage: &dyn RelayCompatibilityRepository,
    relay_url: &str,
) -> Result<RelayCompatibilityResponse, StateServiceError> {
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

pub async fn relay_compatibility_cached(
    storage: &dyn RelayCompatibilityRepository,
    cache: &StateCache,
    relay_url: &str,
) -> Result<RelayCompatibilityResponse, StateServiceError> {
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
    use gittree_storage::AccountLifecycle;
    use gittree_storage::AnnouncementRepository;
    use gittree_storage::InMemoryRepositories;
    use gittree_storage::PostgresRepositories;
    use gittree_storage::ProfileVisibilityV1;
    use gittree_storage::RelayCompatibilityRecord;
    use gittree_storage::RelayCompatibilityRepository;
    use gittree_storage::RelayProbeMetadata;
    use gittree_storage::RepoAnnouncementRecord;
    use gittree_storage::RepoVisibilityV1;
    use gittree_storage::StateRepository;
    use gittree_storage::StorageConfig;
    use gittree_storage::StorageError;
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

    fn projection_repo_for_tests() -> std::sync::Arc<super::DynProjectionRepositories> {
        let config = StateConfig {
            bind: "127.0.0.1:18082".to_string(),
            storage: StorageConfig {
                read_connection: "postgres://localhost/gittree".to_string(),
                write_connection: None,
                max_connections: 5,
                min_connections: 1,
                idle_timeout_secs: None,
                max_lifetime_secs: None,
                application_name: None,
            },
            relay_urls: vec!["wss://gittr.ee".to_string()],
        };
        std::sync::Arc::new(
            super::build_projection_repositories(&config).expect("projection repositories"),
        )
    }

    async fn closed_projection_repo_for_tests() -> std::sync::Arc<super::DynProjectionRepositories>
    {
        let storage = StorageConfig {
            read_connection: "postgres://gittree:gittree@127.0.0.1:5432/gittree".to_string(),
            write_connection: None,
            max_connections: 1,
            min_connections: 1,
            idle_timeout_secs: None,
            max_lifetime_secs: None,
            application_name: Some("gittree-state-closed-projection".to_string()),
        };
        let pool_options = storage.pool_options().expect("pool options");
        let connect_options = storage.read_connect_options().expect("connect options");
        let pool = pool_options.connect_lazy_with(connect_options);
        pool.close().await;
        std::sync::Arc::new(gittree_storage::PostgresRepositories::new(pool))
    }

    #[derive(Default)]
    struct FakeProjectionRepositories {
        account: Option<gittree_storage::AccountStateRecord>,
        account_error: Option<String>,
        profile: Option<gittree_storage::ProfileStateRecord>,
        profile_error: Option<String>,
        repo: Option<gittree_storage::RepoStateV1Record>,
        repo_error: Option<String>,
        maintainers: std::collections::HashSet<Vec<u8>>,
        maintainers_error: Option<String>,
    }

    impl super::ProjectionRepositoriesDyn for FakeProjectionRepositories {
        fn v1_account_state<'a>(&'a self, _pubkey: &'a [u8]) -> super::ProjectionAccountFuture<'a> {
            Box::pin(async move {
                if let Some(message) = &self.account_error {
                    return Err(gittree_storage::StorageError::Internal {
                        message: message.clone(),
                    });
                }
                Ok(self.account.clone())
            })
        }

        fn v1_profile_state<'a>(&'a self, _pubkey: &'a [u8]) -> super::ProjectionProfileFuture<'a> {
            Box::pin(async move {
                if let Some(message) = &self.profile_error {
                    return Err(gittree_storage::StorageError::Internal {
                        message: message.clone(),
                    });
                }
                Ok(self.profile.clone())
            })
        }

        fn v1_repo_state<'a>(
            &'a self,
            _owner_pubkey: &'a [u8],
            _repo_name: &'a str,
        ) -> super::ProjectionRepoFuture<'a> {
            Box::pin(async move {
                if let Some(message) = &self.repo_error {
                    return Err(gittree_storage::StorageError::Internal {
                        message: message.clone(),
                    });
                }
                Ok(self.repo.clone())
            })
        }

        fn v1_list_active_repo_maintainers<'a>(
            &'a self,
            _owner_pubkey: &'a [u8],
            _repo_name: &'a str,
        ) -> super::ProjectionMaintainersFuture<'a> {
            Box::pin(async move {
                if let Some(message) = &self.maintainers_error {
                    return Err(gittree_storage::StorageError::Internal {
                        message: message.clone(),
                    });
                }
                Ok(self.maintainers.clone())
            })
        }
    }

    fn app_with_projection(
        projection_repositories: std::sync::Arc<super::DynProjectionRepositories>,
    ) -> Router {
        let repositories = std::sync::Arc::new(InMemoryRepositories::new());
        let cache = std::sync::Arc::new(StateCache::new(StateCacheConfig::default()));
        super::build_router(super::StateAppState {
            repositories,
            projection_repositories,
            cache,
        })
    }

    fn noop_init_observability() -> Result<(), StateError> {
        Ok(())
    }

    fn failing_init_observability() -> Result<(), StateError> {
        Err(StateError::ObservabilityConfig(
            gittree_observability::ObservabilityConfigError::InvalidEnv {
                key: "GITTREE_LOG_JSON",
                value: "bad".to_string(),
            },
        ))
    }

    fn noop_server(
        _listener: tokio::net::TcpListener,
        _router: Router,
    ) -> super::StateServerFuture {
        Box::pin(async { Ok(()) })
    }

    fn failing_server(
        _listener: tokio::net::TcpListener,
        _router: Router,
    ) -> super::StateServerFuture {
        Box::pin(async { Err(std::io::Error::other("boom")) })
    }

    fn failing_projection_repositories(
        _config: &StateConfig,
    ) -> Result<PostgresRepositories, StateError> {
        Err(StateError::Storage(StorageError::Internal {
            message: "projection build failed".to_string(),
        }))
    }

    fn assert_non_empty_message(message: &str) {
        assert!(!message.is_empty());
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
        assert!(state_err.to_string().contains("internal error"));

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
        assert!(list_err.to_string().contains("internal error"));
        let latest_err = repo
            .latest_announcement(&[0u8; 32], "repo")
            .await
            .expect_err("latest error");
        assert!(latest_err.to_string().contains("internal error"));

        let compat_record = sample_compat_record("wss://relay.example");
        repo.upsert_relay_compatibility(compat_record)
            .await
            .expect("upsert compatibility");
        let compat_err = repo
            .relay_compatibility("wss://relay.example")
            .await
            .expect_err("compatibility error");
        assert!(compat_err.to_string().contains("internal error"));
    }

    #[tokio::test]
    async fn projection_repository_wrapper_methods_cover_postgres_delegate_paths() {
        let repositories = closed_projection_repo_for_tests().await;
        let owner = vec![0x44; 32];

        let account = tokio::time::timeout(
            Duration::from_secs(3),
            repositories.v1_account_state(&owner),
        )
        .await
        .expect("account timeout");
        assert!(account.is_err());

        let profile = tokio::time::timeout(
            Duration::from_secs(3),
            repositories.v1_profile_state(&owner),
        )
        .await
        .expect("profile timeout");
        assert!(profile.is_err());

        let repo = tokio::time::timeout(
            Duration::from_secs(3),
            repositories.v1_repo_state(&owner, "demo"),
        )
        .await
        .expect("repo timeout");
        assert!(repo.is_err());

        let maintainers = tokio::time::timeout(
            Duration::from_secs(3),
            repositories.v1_list_active_repo_maintainers(&owner, "demo"),
        )
        .await
        .expect("maintainers timeout");
        assert!(maintainers.is_err());
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
                    assert!(err.to_string().contains(ENV_STORAGE_READ_URL));
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
                        assert!(err.to_string().contains(super::ENV_STORAGE_MAX_CONNECTIONS));
                    });
                    with_env_var(super::ENV_STORAGE_IDLE_TIMEOUT_SECS, "nope", &mut || {
                        let err = StateConfig::from_env().expect_err("invalid idle timeout");
                        assert!(
                            err.to_string()
                                .contains(super::ENV_STORAGE_IDLE_TIMEOUT_SECS)
                        );
                    });
                    with_env_var(super::ENV_STORAGE_MIN_CONNECTIONS, "bad", &mut || {
                        let err = StateConfig::from_env().expect_err("invalid min connections");
                        assert!(err.to_string().contains(super::ENV_STORAGE_MIN_CONNECTIONS));
                    });
                    with_env_var(super::ENV_STORAGE_MAX_LIFETIME_SECS, "bad", &mut || {
                        let err = StateConfig::from_env().expect_err("invalid max lifetime");
                        assert!(
                            err.to_string()
                                .contains(super::ENV_STORAGE_MAX_LIFETIME_SECS)
                        );
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
                            assert!(err.to_string().contains("min_connections"));
                        });
                    });
                });
            },
        );
    }

    #[test]
    fn env_helpers_handle_missing_blank_and_invalid_values() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        without_env_var("GITTREE_STORAGE_MAX_CONNECTIONS", &mut || {
            assert!(
                super::env_u32("GITTREE_STORAGE_MAX_CONNECTIONS")
                    .expect("missing is none")
                    .is_none()
            );
        });
        with_env_var("GITTREE_STORAGE_MAX_CONNECTIONS", "  ", &mut || {
            assert!(
                super::env_u32("GITTREE_STORAGE_MAX_CONNECTIONS")
                    .expect("blank is none")
                    .is_none()
            );
        });
        with_env_var("GITTREE_STORAGE_MAX_CONNECTIONS", "bad", &mut || {
            let err = super::env_u32("GITTREE_STORAGE_MAX_CONNECTIONS").expect_err("invalid");
            assert!(
                err.to_string()
                    .contains("invalid env GITTREE_STORAGE_MAX_CONNECTIONS: bad")
            );
        });

        without_env_var("GITTREE_STORAGE_IDLE_TIMEOUT_SECS", &mut || {
            assert!(
                super::env_u64("GITTREE_STORAGE_IDLE_TIMEOUT_SECS")
                    .expect("missing is none")
                    .is_none()
            );
        });
        with_env_var("GITTREE_STORAGE_IDLE_TIMEOUT_SECS", "", &mut || {
            assert!(
                super::env_u64("GITTREE_STORAGE_IDLE_TIMEOUT_SECS")
                    .expect("blank is none")
                    .is_none()
            );
        });
        with_env_var("GITTREE_STORAGE_IDLE_TIMEOUT_SECS", "oops", &mut || {
            let err = super::env_u64("GITTREE_STORAGE_IDLE_TIMEOUT_SECS").expect_err("invalid");
            assert!(
                err.to_string()
                    .contains("invalid env GITTREE_STORAGE_IDLE_TIMEOUT_SECS: oops")
            );
        });
    }

    #[test]
    fn storage_from_env_covers_success_and_invalid_config_paths() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var(
            ENV_STORAGE_READ_URL,
            "postgres://user:pass@localhost:5432/gittree",
            &mut || {
                with_env_var(
                    "GITTREE_STORAGE_WRITE_URL",
                    "postgres://user:pass@localhost:5432/gittree",
                    &mut || {
                        with_env_var("GITTREE_STORAGE_MAX_CONNECTIONS", "5", &mut || {
                            with_env_var("GITTREE_STORAGE_MIN_CONNECTIONS", "1", &mut || {
                                with_env_var(
                                    "GITTREE_STORAGE_APP_NAME",
                                    "state-tests",
                                    &mut || {
                                        let cfg =
                                            super::storage_from_env().expect("storage config");
                                        assert!(cfg.write_connection.is_some());
                                        assert_eq!(cfg.max_connections, 5);
                                        assert_eq!(cfg.min_connections, 1);
                                        assert_eq!(
                                            cfg.application_name.as_deref(),
                                            Some("state-tests")
                                        );
                                    },
                                );
                            });
                        });
                    },
                );

                with_env_var("GITTREE_STORAGE_MAX_CONNECTIONS", "1", &mut || {
                    with_env_var("GITTREE_STORAGE_MIN_CONNECTIONS", "2", &mut || {
                        let err = super::storage_from_env().expect_err("invalid config");
                        let message = err.to_string();
                        assert_non_empty_message(&message);
                    });
                });
            },
        );
    }

    #[test]
    fn parse_npub_param_covers_valid_and_invalid_paths() {
        let pubkey = vec![0x11; 32];
        let npub = gittree_app_core::npub_from_bytes(&pubkey).expect("npub");
        let parsed = super::parse_npub_param(&npub).expect("pubkey");
        assert_eq!(parsed, pubkey);

        let err = super::parse_npub_param("not-an-npub").expect_err("invalid npub");
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
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
    fn config_reads_explicit_min_connections_and_max_lifetime() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var(
            ENV_STORAGE_READ_URL,
            "postgres://user:pass@localhost:5432/gittree",
            &mut || {
                with_env_var("GITTREE_RELAY_URLS", "wss://relay.example", &mut || {
                    with_env_var(super::ENV_STORAGE_MIN_CONNECTIONS, "5", &mut || {
                        with_env_var(super::ENV_STORAGE_MAX_LIFETIME_SECS, "600", &mut || {
                            let config = StateConfig::from_env().expect("config");
                            assert_eq!(config.storage.min_connections, 5);
                            assert_eq!(config.storage.max_lifetime_secs, Some(600));
                        });
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
                        assert!(err.to_string().contains("state config error"));
                    });
                });
                with_env_var("GITTREE_STATE_BIND", "127.0.0.1:8082", &mut || {
                    with_env_var("GITTREE_RELAY_URLS", "not-a-url", &mut || {
                        let err = StateConfig::from_env().expect_err("invalid relay targets");
                        assert!(err.to_string().contains("state config error"));
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
        assert!(err.to_string().contains("state not found"));
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
        assert!(err.to_string().contains("storage error"));
    }

    #[tokio::test]
    async fn latest_state_cached_propagates_lookup_errors() {
        let repo = ErrorRepositories;
        let cache = StateCache::new(StateCacheConfig::new(None, 10));
        let err = latest_state_cached(&repo, &cache, &"11".repeat(32), "repo")
            .await
            .expect_err("cached error");
        assert!(err.to_string().contains("storage error"));
    }

    #[tokio::test]
    async fn latest_state_maps_invalid_state_json_to_storage_error() {
        let repo = InMemoryRepositories::default();
        repo.insert_state(gittree_storage::RepoStateRecord {
            event_id: vec![0x11; 32],
            pubkey: vec![0x22; 32],
            identifier: "repo".to_string(),
            created_at: 42,
            state_json: "{not-json".to_string(),
        })
        .await
        .expect("insert state");

        let err = latest_state(&repo, &"22".repeat(32), "repo")
            .await
            .expect_err("serialization error");
        assert!(err.to_string().contains("storage error"));
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
        assert!(err.to_string().contains("relay compatibility not found"));
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
        assert!(err.to_string().contains("storage error"));
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
        assert!(err.to_string().contains("storage error"));
    }

    #[tokio::test]
    async fn resolve_maintainers_cached_propagates_lookup_errors() {
        let repo = ErrorRepositories;
        let cache = StateCache::new(StateCacheConfig::new(None, 10));
        let err = resolve_maintainers_cached(&repo, &cache, &"11".repeat(32), "repo")
            .await
            .expect_err("cached error");
        assert!(err.to_string().contains("storage error"));
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
    fn cache_stale_get_handles_poisoned_write_locks() {
        let cache = StateCache::new(StateCacheConfig::new(Some(Duration::from_secs(0)), 10));

        let state_key = StateCache::key("aa", "repo");
        cache.insert_state(
            state_key.clone(),
            super::StateResponse {
                event_id: "state-event".to_string(),
                pubkey: "aa".to_string(),
                identifier: "repo".to_string(),
                created_at: 1,
                state: HashMap::new(),
            },
        );
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = cache.state_entries.write().expect("lock");
            panic!("poison state write lock");
        }));
        assert!(cache.get_state(&state_key).is_none());

        let maintainers_key = StateCache::key("bb", "repo");
        cache.insert_maintainers(
            maintainers_key.clone(),
            super::MaintainersResponse {
                identifier: "repo".to_string(),
                maintainers: vec!["bb".to_string()],
            },
        );
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = cache.maintainer_entries.write().expect("lock");
            panic!("poison maintainers write lock");
        }));
        assert!(cache.get_maintainers(&maintainers_key).is_none());

        let relay_key = StateCache::relay_key("wss://relay.example");
        cache.insert_relay_compatibility(
            relay_key.clone(),
            super::RelayCompatibilityResponse {
                relay_url: "wss://relay.example".to_string(),
                compatible: true,
                supported_capabilities: vec!["NIP-01".to_string()],
                missing_required: Vec::new(),
                missing_optional: Vec::new(),
                nip11_url: None,
                nip11_available: true,
                active_probe_ok: Some(true),
                active_probe_error: None,
                checked_at: 1,
            },
        );
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = cache.relay_entries.write().expect("lock");
            panic!("poison relay write lock");
        }));
        assert!(cache.get_relay_compatibility(&relay_key).is_none());
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
    fn cache_evicts_oldest_entries_for_maintainers_and_relay() {
        let cache = StateCache::new(StateCacheConfig::new(Some(Duration::from_secs(30)), 1));

        let old_maintainers_key = StateCache::key("aa", "repo");
        let new_maintainers_key = StateCache::key("bb", "repo");
        cache.insert_maintainers(
            old_maintainers_key.clone(),
            super::MaintainersResponse {
                identifier: "repo".to_string(),
                maintainers: vec!["aa".to_string()],
            },
        );
        let new_maintainers = super::MaintainersResponse {
            identifier: "repo".to_string(),
            maintainers: vec!["bb".to_string()],
        };
        cache.insert_maintainers(new_maintainers_key.clone(), new_maintainers.clone());

        assert!(cache.get_maintainers(&old_maintainers_key).is_none());
        assert_eq!(
            cache.get_maintainers(&new_maintainers_key),
            Some(new_maintainers)
        );

        let old_relay_key = StateCache::relay_key("wss://old.example");
        let new_relay_key = StateCache::relay_key("wss://new.example");
        cache.insert_relay_compatibility(
            old_relay_key.clone(),
            super::RelayCompatibilityResponse {
                relay_url: "wss://old.example".to_string(),
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
        let new_relay = super::RelayCompatibilityResponse {
            relay_url: "wss://new.example".to_string(),
            compatible: false,
            supported_capabilities: vec!["nip01".to_string()],
            missing_required: vec!["nip34".to_string()],
            missing_optional: Vec::new(),
            nip11_url: None,
            nip11_available: false,
            active_probe_ok: Some(false),
            active_probe_error: Some("timeout".to_string()),
            checked_at: 2,
        };
        cache.insert_relay_compatibility(new_relay_key.clone(), new_relay.clone());

        assert!(cache.get_relay_compatibility(&old_relay_key).is_none());
        assert_eq!(
            cache.get_relay_compatibility(&new_relay_key),
            Some(new_relay)
        );
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
            projection_repositories: projection_repo_for_tests(),
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
            projection_repositories: projection_repo_for_tests(),
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
            projection_repositories: projection_repo_for_tests(),
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

    #[tokio::test]
    async fn v1_account_endpoint_covers_success_not_found_bad_request_and_storage() {
        let pubkey = vec![0x11; 32];
        let npub = gittree_app_core::npub_from_bytes(&pubkey).expect("npub");
        let account = gittree_storage::AccountStateRecord {
            pubkey: pubkey.clone(),
            status: AccountLifecycle::Active,
            created_at: 10,
            updated_at: 20,
            deleted_at: None,
        };

        let app = app_with_projection(std::sync::Arc::new(FakeProjectionRepositories {
            account: Some(account),
            ..Default::default()
        }));
        let success = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/v1/accounts/{npub}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(success.status(), StatusCode::OK);
        let body = to_bytes(success.into_body(), usize::MAX)
            .await
            .expect("body");
        let json: serde_json::Value = serde_json::from_slice(&body).expect("json");
        assert_eq!(json["npub"], npub);
        assert_eq!(json["status"], "active");
        assert_eq!(json["created_at"], 10);
        assert_eq!(json["updated_at"], 20);

        let missing =
            app_with_projection(std::sync::Arc::new(FakeProjectionRepositories::default()))
                .oneshot(
                    Request::builder()
                        .uri(format!("/v1/accounts/{npub}"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .expect("response");
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);

        let bad_request =
            app_with_projection(std::sync::Arc::new(FakeProjectionRepositories::default()))
                .oneshot(
                    Request::builder()
                        .uri("/v1/accounts/not-an-npub")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .expect("response");
        assert_eq!(bad_request.status(), StatusCode::BAD_REQUEST);

        let storage_error = app_with_projection(std::sync::Arc::new(FakeProjectionRepositories {
            account_error: Some("account failure".to_string()),
            ..Default::default()
        }))
        .oneshot(
            Request::builder()
                .uri(format!("/v1/accounts/{npub}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("response");
        assert_eq!(storage_error.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[tokio::test]
    async fn v1_profile_endpoint_covers_public_private_not_found_and_storage() {
        let pubkey = vec![0x22; 32];
        let npub = gittree_app_core::npub_from_bytes(&pubkey).expect("npub");
        let profile = gittree_storage::ProfileStateRecord {
            pubkey: pubkey.clone(),
            display_name: Some("alice".to_string()),
            bio: Some("builder".to_string()),
            avatar_url: None,
            website_url: None,
            location: Some("earth".to_string()),
            visibility: ProfileVisibilityV1::Public,
            updated_at: 30,
        };

        let app = app_with_projection(std::sync::Arc::new(FakeProjectionRepositories {
            profile: Some(profile),
            ..Default::default()
        }));
        let success = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/v1/profiles/{npub}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(success.status(), StatusCode::OK);

        let private = app_with_projection(std::sync::Arc::new(FakeProjectionRepositories {
            profile: Some(gittree_storage::ProfileStateRecord {
                pubkey,
                display_name: None,
                bio: None,
                avatar_url: None,
                website_url: None,
                location: None,
                visibility: ProfileVisibilityV1::Private,
                updated_at: 40,
            }),
            ..Default::default()
        }))
        .oneshot(
            Request::builder()
                .uri(format!("/v1/profiles/{npub}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("response");
        assert_eq!(private.status(), StatusCode::NOT_FOUND);

        let missing =
            app_with_projection(std::sync::Arc::new(FakeProjectionRepositories::default()))
                .oneshot(
                    Request::builder()
                        .uri(format!("/v1/profiles/{npub}"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .expect("response");
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);

        let storage_error = app_with_projection(std::sync::Arc::new(FakeProjectionRepositories {
            profile_error: Some("profile failure".to_string()),
            ..Default::default()
        }))
        .oneshot(
            Request::builder()
                .uri(format!("/v1/profiles/{npub}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("response");
        assert_eq!(storage_error.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[tokio::test]
    async fn v1_repo_endpoint_covers_success_not_found_bad_request_and_storage() {
        let owner_pubkey = vec![0x33; 32];
        let owner = gittree_app_core::npub_from_bytes(&owner_pubkey).expect("npub");
        let app = app_with_projection(std::sync::Arc::new(FakeProjectionRepositories {
            repo: Some(gittree_storage::RepoStateV1Record {
                owner_pubkey: owner_pubkey.clone(),
                repo_name: "demo".to_string(),
                description: Some("demo repo".to_string()),
                website_url: None,
                visibility: RepoVisibilityV1::Public,
                default_branch: "main".to_string(),
                archived: false,
                updated_at: 50,
            }),
            ..Default::default()
        }));

        let success = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/v1/repos/{owner}/demo"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(success.status(), StatusCode::OK);

        let missing =
            app_with_projection(std::sync::Arc::new(FakeProjectionRepositories::default()))
                .oneshot(
                    Request::builder()
                        .uri(format!("/v1/repos/{owner}/demo"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .expect("response");
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);

        let bad_request =
            app_with_projection(std::sync::Arc::new(FakeProjectionRepositories::default()))
                .oneshot(
                    Request::builder()
                        .uri("/v1/repos/not-an-npub/demo")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .expect("response");
        assert_eq!(bad_request.status(), StatusCode::BAD_REQUEST);

        let storage_error = app_with_projection(std::sync::Arc::new(FakeProjectionRepositories {
            repo_error: Some("repo failure".to_string()),
            ..Default::default()
        }))
        .oneshot(
            Request::builder()
                .uri(format!("/v1/repos/{owner}/demo"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("response");
        assert_eq!(storage_error.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[tokio::test]
    async fn v1_repo_maintainers_endpoint_covers_sorting_and_storage() {
        let owner_pubkey = vec![0x44; 32];
        let owner = gittree_app_core::npub_from_bytes(&owner_pubkey).expect("npub");
        let maintainer_a = vec![0x21; 32];
        let maintainer_b = vec![0x20; 32];
        let maintainer_short = vec![0x01, 0x02];
        let npub_a = gittree_app_core::npub_from_bytes(&maintainer_a).expect("npub");
        let npub_b = gittree_app_core::npub_from_bytes(&maintainer_b).expect("npub");
        let npub_short = gittree_app_core::npub_from_bytes(&maintainer_short).expect("npub");
        let app = app_with_projection(std::sync::Arc::new(FakeProjectionRepositories {
            maintainers: std::collections::HashSet::from([
                maintainer_a,
                maintainer_short,
                maintainer_b,
            ]),
            ..Default::default()
        }));

        let success = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/v1/repos/{owner}/demo/maintainers"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(success.status(), StatusCode::OK);
        let body = to_bytes(success.into_body(), usize::MAX)
            .await
            .expect("body");
        let json: serde_json::Value = serde_json::from_slice(&body).expect("json");
        let mut expected = vec![npub_a, npub_b, npub_short];
        expected.sort();
        assert_eq!(json["maintainers"], serde_json::json!(expected));

        let bad_request =
            app_with_projection(std::sync::Arc::new(FakeProjectionRepositories::default()))
                .oneshot(
                    Request::builder()
                        .uri("/v1/repos/not-an-npub/demo/maintainers")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .expect("response");
        assert_eq!(bad_request.status(), StatusCode::BAD_REQUEST);

        let storage_error = app_with_projection(std::sync::Arc::new(FakeProjectionRepositories {
            maintainers_error: Some("maintainer failure".to_string()),
            ..Default::default()
        }))
        .oneshot(
            Request::builder()
                .uri(format!("/v1/repos/{owner}/demo/maintainers"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("response");
        assert_eq!(storage_error.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[tokio::test]
    async fn v1_repo_activity_endpoint_returns_empty_activity() {
        let owner = gittree_app_core::npub_from_bytes(&[0x55; 32]).expect("npub");
        let app = app_with_projection(std::sync::Arc::new(FakeProjectionRepositories::default()));
        let response = app
            .oneshot(
                Request::builder()
                    .uri(format!("/v1/repos/{owner}/demo/activity"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let json: serde_json::Value = serde_json::from_slice(&body).expect("json");
        assert_eq!(json["activity"], serde_json::json!([]));
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

    #[tokio::test]
    async fn build_projection_repositories_constructs_lazy_pool() {
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

        let repos = super::build_projection_repositories(&config).expect("repositories");
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
        assert!(err.to_string().contains("state storage error"));
    }

    #[test]
    fn build_repositories_rejects_invalid_pool_config() {
        let config = StateConfig {
            bind: "127.0.0.1:0".to_string(),
            storage: StorageConfig {
                read_connection: "postgres://gittree:gittree@127.0.0.1:5432/gittree".to_string(),
                write_connection: None,
                max_connections: 0,
                min_connections: 0,
                idle_timeout_secs: None,
                max_lifetime_secs: None,
                application_name: None,
            },
            relay_urls: Vec::new(),
        };
        let err = super::build_repositories(&config).expect_err("invalid pool config");
        assert!(err.to_string().contains("state storage error"));
    }

    #[test]
    fn build_projection_repositories_reject_invalid_connection_and_pool() {
        let invalid_connection = StateConfig {
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
        let err = super::build_projection_repositories(&invalid_connection)
            .expect_err("invalid connection");
        assert!(err.to_string().contains("state storage error"));

        let invalid_pool = StateConfig {
            bind: "127.0.0.1:0".to_string(),
            storage: StorageConfig {
                read_connection: "postgres://gittree:gittree@127.0.0.1:5432/gittree".to_string(),
                write_connection: None,
                max_connections: 0,
                min_connections: 0,
                idle_timeout_secs: None,
                max_lifetime_secs: None,
                application_name: None,
            },
            relay_urls: Vec::new(),
        };
        let err =
            super::build_projection_repositories(&invalid_pool).expect_err("invalid pool config");
        assert!(err.to_string().contains("state storage error"));
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
        assert!(err.to_string().starts_with("state "));
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
        let err = super::serve_with(config, failing_init_observability, noop_server)
            .await
            .expect_err("observability error");
        assert!(err.to_string().contains("state observability config error"));
    }

    #[tokio::test]
    async fn serve_with_maps_repository_build_errors() {
        let config = StateConfig {
            bind: "127.0.0.1:0".to_string(),
            storage: StorageConfig {
                read_connection: "postgres://gittree:gittree@127.0.0.1:5432/gittree".to_string(),
                write_connection: None,
                max_connections: 0,
                min_connections: 0,
                idle_timeout_secs: None,
                max_lifetime_secs: None,
                application_name: None,
            },
            relay_urls: Vec::new(),
        };
        let err = super::serve_with(config, noop_init_observability, noop_server)
            .await
            .expect_err("repository build error");
        assert!(err.to_string().contains("state storage error"));
    }

    #[tokio::test]
    async fn serve_with_components_maps_projection_repository_build_errors() {
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
        let err = super::serve_with_components(
            config,
            noop_init_observability,
            super::build_repositories,
            failing_projection_repositories,
            noop_server,
        )
        .await
        .expect_err("projection repository build error");
        assert!(err.to_string().contains("projection build failed"));
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
        let err = super::serve_with(config, noop_init_observability, failing_server)
            .await
            .expect_err("server error");
        assert_eq!(err.to_string(), "state serve error: boom");
    }

    #[tokio::test]
    async fn serve_with_maps_bind_errors_before_server_start() {
        let config = StateConfig {
            bind: "not-a-bind".to_string(),
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
        let err = super::serve_with(config, noop_init_observability, noop_server)
            .await
            .expect_err("bind error");
        assert!(err.to_string().starts_with("state serve error:"));
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
        let result = super::serve_with(config, noop_init_observability, noop_server).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn run_axum_server_with_shutdown_returns_ok() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let router = Router::new().route("/health", axum::routing::get(super::health_handler));
        let result =
            super::run_axum_server_with_shutdown(listener, router, Box::pin(async {})).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn run_axum_server_boxed_can_start_and_be_aborted() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let router = Router::new().route("/health", axum::routing::get(super::health_handler));
        let result = tokio::time::timeout(
            Duration::from_millis(5),
            super::run_axum_server_boxed(listener, router),
        )
        .await;
        assert!(result.is_err());
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
    fn assert_non_empty_message_panics_for_empty_input() {
        let panic = std::panic::catch_unwind(|| assert_non_empty_message(""));
        assert!(panic.is_err());
    }

    #[test]
    fn observability_init_reports_config_error() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var("GITTREE_LOG_STDOUT", "invalid-bool", &mut || {
            let err = init_observability().expect_err("invalid observability config");
            assert!(err.to_string().contains("state observability config error"));
        });
    }

    #[test]
    fn init_observability_unit_maps_config_error() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var("GITTREE_LOG_STDOUT", "invalid-bool", &mut || {
            let err = super::init_observability_unit().expect_err("invalid observability config");
            assert!(err.to_string().contains("state observability config error"));
        });
    }
}
