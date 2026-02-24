use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use gittree_config::{
    ConfigError, RelayCompatibilityConfig, RelayCompatibilityMode, ServicesConfig,
};
use gittree_core::kinds::KIND_GITTREE_CONTROL;
use gittree_core::nip34_common::RepoAddress;
use gittree_core::{CoreError, EventFilter};
use gittree_observability::{
    ObservabilityConfigError, ObservabilityError, ObservabilityHandle, RelayCompatibilityMetrics,
};
use gittree_storage::{
    AnnouncementRepository, CachedRepositories, PostgresRepositories, RelayCompatibilityRepository,
    StateRepository, StorageConfig, StorageError,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::future::Future;
use std::hash::Hash;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};
use tracing::warn;

const ENV_STORAGE_READ_URL: &str = "GITTREE_STORAGE_READ_URL";
const ENV_STORAGE_WRITE_URL: &str = "GITTREE_STORAGE_WRITE_URL";
const ENV_STORAGE_MAX_CONNECTIONS: &str = "GITTREE_STORAGE_MAX_CONNECTIONS";
const ENV_STORAGE_MIN_CONNECTIONS: &str = "GITTREE_STORAGE_MIN_CONNECTIONS";
const ENV_STORAGE_IDLE_TIMEOUT_SECS: &str = "GITTREE_STORAGE_IDLE_TIMEOUT_SECS";
const ENV_STORAGE_MAX_LIFETIME_SECS: &str = "GITTREE_STORAGE_MAX_LIFETIME_SECS";
const ENV_STORAGE_APP_NAME: &str = "GITTREE_STORAGE_APP_NAME";
const ENV_CONTROL_ADMIN_KEYS: &str = "GITTREE_CONTROL_ADMIN_KEYS";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdmissionConfig {
    pub bind: String,
    pub compatibility: RelayCompatibilityConfig,
    pub storage: StorageConfig,
    pub control_admin_keys: Vec<String>,
}

impl AdmissionConfig {
    pub fn from_env() -> Result<Self, AdmissionConfigError> {
        let services =
            ServicesConfig::from_env_validated().map_err(AdmissionConfigError::Config)?;
        let compatibility =
            RelayCompatibilityConfig::from_env().map_err(AdmissionConfigError::Config)?;
        let storage = storage_from_env()?;
        let control_admin_keys = admin_keys_from_env();
        Ok(Self {
            bind: services.admission.bind,
            compatibility,
            storage,
            control_admin_keys,
        })
    }
}

#[derive(Debug)]
pub enum AdmissionConfigError {
    Config(ConfigError),
    Storage(StorageConfigError),
}

impl std::fmt::Display for AdmissionConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AdmissionConfigError::Config(err) => write!(f, "admission config error: {err}"),
            AdmissionConfigError::Storage(err) => {
                write!(f, "admission storage config error: {err}")
            }
        }
    }
}

impl std::error::Error for AdmissionConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            AdmissionConfigError::Config(err) => Some(err),
            AdmissionConfigError::Storage(err) => Some(err),
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

fn storage_from_env() -> Result<StorageConfig, AdmissionConfigError> {
    let read_connection = std::env::var(ENV_STORAGE_READ_URL).map_err(|_| {
        AdmissionConfigError::Storage(StorageConfigError::MissingEnv(ENV_STORAGE_READ_URL))
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
        AdmissionConfigError::Storage(StorageConfigError::InvalidConfig(err.to_string()))
    })?;

    Ok(config)
}

fn env_u32(key: &'static str) -> Result<Option<u32>, AdmissionConfigError> {
    match std::env::var(key) {
        Ok(value) => {
            if value.trim().is_empty() {
                return Ok(None);
            }
            value.parse::<u32>().map(Some).map_err(|_| {
                AdmissionConfigError::Storage(StorageConfigError::InvalidEnv { key, value })
            })
        }
        Err(_) => Ok(None),
    }
}

fn env_u64(key: &'static str) -> Result<Option<u64>, AdmissionConfigError> {
    match std::env::var(key) {
        Ok(value) => {
            if value.trim().is_empty() {
                return Ok(None);
            }
            value.parse::<u64>().map(Some).map_err(|_| {
                AdmissionConfigError::Storage(StorageConfigError::InvalidEnv { key, value })
            })
        }
        Err(_) => Ok(None),
    }
}

fn admin_keys_from_env() -> Vec<String> {
    match std::env::var(ENV_CONTROL_ADMIN_KEYS) {
        Ok(value) => parse_csv_values(value),
        Err(_) => Vec::new(),
    }
}

fn parse_csv_values(raw: String) -> Vec<String> {
    raw.split(',')
        .map(|entry| entry.trim().to_string())
        .filter(|entry| !entry.is_empty())
        .collect()
}

#[derive(Debug)]
pub enum AdmissionError {
    Config(AdmissionConfigError),
    Request(AdmissionRequestError),
    Core(CoreError),
    ObservabilityConfig(ObservabilityConfigError),
    Observability(ObservabilityError),
    Storage(StorageError),
    Serve(String),
}

#[derive(Debug, Clone)]
pub struct AdmissionCacheConfig {
    pub ttl: Option<Duration>,
    pub max_entries: usize,
}

impl AdmissionCacheConfig {
    pub fn new(ttl: Option<Duration>, max_entries: usize) -> Self {
        Self { ttl, max_entries }
    }
}

impl Default for AdmissionCacheConfig {
    fn default() -> Self {
        Self {
            ttl: Some(Duration::from_secs(30)),
            max_entries: 1024,
        }
    }
}

#[derive(Debug, Clone)]
struct AdmissionCacheEntry {
    value: AdmissionDecision,
    stored_at: Instant,
}

#[derive(Debug)]
pub struct AdmissionCache {
    config: AdmissionCacheConfig,
    entries: RwLock<HashMap<String, AdmissionCacheEntry>>,
}

impl AdmissionCache {
    pub fn new(config: AdmissionCacheConfig) -> Self {
        Self {
            config,
            entries: RwLock::new(HashMap::new()),
        }
    }

    pub fn cache_key(&self, request: &AdmissionRequest) -> String {
        format!("{}:{}:{}", request.kind, request.pubkey, request.event_id)
    }

    fn cache_enabled(&self) -> bool {
        self.config.max_entries > 0
    }

    fn is_fresh(&self, entry: &AdmissionCacheEntry) -> bool {
        match self.config.ttl {
            Some(ttl) => entry.stored_at.elapsed() < ttl,
            None => true,
        }
    }

    fn evict_if_needed<K>(&self, map: &mut HashMap<K, AdmissionCacheEntry>)
    where
        K: Clone + Eq + Hash,
    {
        let max_entries = self.config.max_entries;
        if max_entries == 0 {
            map.clear();
            return;
        }
        if map.len() <= max_entries {
            return;
        }
        let remove_count = map.len() - max_entries;
        let mut oldest: Vec<(std::time::Instant, K)> = map
            .iter()
            .map(|(key, entry)| (entry.stored_at, key.clone()))
            .collect();
        oldest.sort_by_key(|(stored_at, _)| *stored_at);
        for (_, key) in oldest.into_iter().take(remove_count) {
            map.remove(&key);
        }
    }

    pub fn get(&self, key: &str) -> Option<AdmissionDecision> {
        if !self.cache_enabled() {
            return None;
        }
        let cached = {
            let entries = self.entries.read().ok()?;
            entries.get(key).cloned()
        };
        match cached {
            Some(entry) if self.is_fresh(&entry) => Some(entry.value),
            Some(_) => {
                if let Ok(mut entries) = self.entries.write() {
                    entries.remove(key);
                }
                None
            }
            None => None,
        }
    }

    pub fn insert(&self, key: String, decision: AdmissionDecision) {
        if !self.cache_enabled() {
            return;
        }
        if let Ok(mut entries) = self.entries.write() {
            entries.insert(
                key,
                AdmissionCacheEntry {
                    value: decision,
                    stored_at: Instant::now(),
                },
            );
            self.evict_if_needed(&mut entries);
        }
    }
}

impl Default for AdmissionCache {
    fn default() -> Self {
        Self::new(AdmissionCacheConfig::default())
    }
}

#[derive(Debug, Clone)]
pub struct RateLimitConfig {
    pub per_ip: u32,
    pub per_pubkey: u32,
    pub window: Duration,
}

impl RateLimitConfig {
    pub fn new(per_ip: u32, per_pubkey: u32, window: Duration) -> Self {
        Self {
            per_ip,
            per_pubkey,
            window,
        }
    }
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            per_ip: 120,
            per_pubkey: 60,
            window: Duration::from_secs(60),
        }
    }
}

#[derive(Debug, Clone)]
struct RateLimitCounter {
    count: u32,
    window_start: Instant,
}

#[derive(Debug)]
pub struct RateLimiter {
    config: RateLimitConfig,
    ip_counters: RwLock<HashMap<String, RateLimitCounter>>,
    pubkey_counters: RwLock<HashMap<String, RateLimitCounter>>,
}

impl RateLimiter {
    pub fn new(config: RateLimitConfig) -> Self {
        Self {
            config,
            ip_counters: RwLock::new(HashMap::new()),
            pubkey_counters: RwLock::new(HashMap::new()),
        }
    }

    pub fn check(&self, request: &AdmissionRequest) -> Option<AdmissionDecision> {
        if let Some(ip) = request.source_ip() {
            if self.hit_limit(&self.ip_counters, ip, self.config.per_ip) {
                return Some(AdmissionDecision::Reject {
                    reason: "rate limit exceeded for ip".to_string(),
                });
            }
        }

        if self.hit_limit(
            &self.pubkey_counters,
            &request.pubkey,
            self.config.per_pubkey,
        ) {
            return Some(AdmissionDecision::Reject {
                reason: "rate limit exceeded for pubkey".to_string(),
            });
        }

        None
    }

    fn hit_limit(
        &self,
        counters: &RwLock<HashMap<String, RateLimitCounter>>,
        key: &str,
        limit: u32,
    ) -> bool {
        if limit == 0 {
            return false;
        }

        let now = Instant::now();
        let mut counters = match counters.write() {
            Ok(counters) => counters,
            Err(poisoned) => poisoned.into_inner(),
        };
        let entry = counters.entry(key.to_string()).or_insert(RateLimitCounter {
            count: 0,
            window_start: now,
        });

        if now.duration_since(entry.window_start) >= self.config.window {
            entry.count = 0;
            entry.window_start = now;
        }

        if entry.count >= limit {
            return true;
        }

        entry.count += 1;
        false
    }
}

impl Default for RateLimiter {
    fn default() -> Self {
        Self::new(RateLimitConfig::default())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdmissionRequest {
    pub kind: u64,
    pub pubkey: String,
    pub event_id: String,
    pub tags: Vec<Vec<String>>,
    pub relay_url: Option<String>,
    pub source_ip: Option<String>,
}

impl AdmissionRequest {
    pub fn new(
        kind: u64,
        pubkey: impl Into<String>,
        event_id: impl Into<String>,
        tags: Vec<Vec<String>>,
        relay_url: Option<String>,
        source_ip: Option<String>,
    ) -> Result<Self, AdmissionRequestError> {
        let pubkey = pubkey.into();
        if pubkey.is_empty() {
            return Err(AdmissionRequestError::MissingField("pubkey"));
        }

        let event_id = event_id.into();
        if event_id.is_empty() {
            return Err(AdmissionRequestError::MissingField("event_id"));
        }

        if tags.iter().any(|tag| tag.is_empty()) {
            return Err(AdmissionRequestError::InvalidTag);
        }

        if relay_url
            .as_ref()
            .is_some_and(|value| value.trim().is_empty())
        {
            return Err(AdmissionRequestError::InvalidRelayUrl);
        }

        Ok(Self {
            kind,
            pubkey,
            event_id,
            tags,
            relay_url,
            source_ip,
        })
    }

    pub fn relay_host(&self) -> Option<&str> {
        self.relay_url.as_deref()
    }

    pub fn source_ip(&self) -> Option<&str> {
        self.source_ip.as_deref()
    }

    pub fn kind_u32(&self) -> Result<u32, AdmissionRequestError> {
        u32::try_from(self.kind).map_err(|_| AdmissionRequestError::InvalidKind(self.kind))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdmissionDecision {
    Accept,
    Reject { reason: String },
    RequiresRelatedEvents { filters: Vec<EventFilter> },
}

impl AdmissionDecision {
    pub fn reject(reason: impl Into<String>) -> Result<Self, AdmissionDecisionError> {
        let reason = reason.into();
        if reason.trim().is_empty() {
            return Err(AdmissionDecisionError::MissingReason);
        }
        Ok(Self::Reject { reason })
    }

    pub fn requires_related(filters: Vec<EventFilter>) -> Result<Self, AdmissionDecisionError> {
        if filters.is_empty() {
            return Err(AdmissionDecisionError::MissingFilters);
        }
        Ok(Self::RequiresRelatedEvents { filters })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdmissionRequestError {
    MissingField(&'static str),
    InvalidTag,
    InvalidKind(u64),
    InvalidRelayUrl,
}

impl std::fmt::Display for AdmissionRequestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AdmissionRequestError::MissingField(field) => {
                write!(f, "missing admission field {field}")
            }
            AdmissionRequestError::InvalidTag => write!(f, "invalid admission tag"),
            AdmissionRequestError::InvalidKind(kind) => {
                write!(f, "invalid admission kind {kind}")
            }
            AdmissionRequestError::InvalidRelayUrl => {
                write!(f, "invalid admission relay url")
            }
        }
    }
}

impl std::error::Error for AdmissionRequestError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdmissionDecisionError {
    MissingReason,
    MissingFilters,
}

impl std::fmt::Display for AdmissionDecisionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AdmissionDecisionError::MissingReason => {
                write!(f, "missing admission rejection reason")
            }
            AdmissionDecisionError::MissingFilters => {
                write!(f, "missing related event filters")
            }
        }
    }
}

impl std::error::Error for AdmissionDecisionError {}

pub fn evaluate_request(request: &AdmissionRequest) -> Result<AdmissionDecision, AdmissionError> {
    evaluate_request_with_admin_keys(request, &[])
}

pub fn evaluate_request_with_admin_keys(
    request: &AdmissionRequest,
    admin_keys: &[String],
) -> Result<AdmissionDecision, AdmissionError> {
    let kind = request.kind_u32().map_err(AdmissionError::Request)?;
    if kind == KIND_GITTREE_CONTROL.0 {
        if is_control_admin(&request.pubkey, admin_keys) {
            return Ok(AdmissionDecision::Accept);
        }
        return Ok(AdmissionDecision::Reject {
            reason: "control event not authorized".to_string(),
        });
    }
    let decision = gittree_core::evaluate_admission(
        kind,
        &request.pubkey,
        &request.event_id,
        &request.tags,
        request.relay_host(),
    )
    .map_err(AdmissionError::Core)?;

    Ok(match decision {
        gittree_core::AdmissionDecision::Accept => AdmissionDecision::Accept,
        gittree_core::AdmissionDecision::Reject { reason } => AdmissionDecision::Reject { reason },
        gittree_core::AdmissionDecision::RequiresRelatedEvents { filters } => {
            AdmissionDecision::RequiresRelatedEvents { filters }
        }
    })
}

pub async fn evaluate_request_with_storage<S>(
    request: &AdmissionRequest,
    storage: &S,
) -> Result<AdmissionDecision, AdmissionError>
where
    S: AnnouncementRepository + RelayCompatibilityRepository + StateRepository,
{
    evaluate_request_with_storage_mode(request, storage, RelayCompatibilityMode::Strict, &[]).await
}

pub async fn evaluate_request_with_storage_mode<S>(
    request: &AdmissionRequest,
    storage: &S,
    mode: RelayCompatibilityMode,
    admin_keys: &[String],
) -> Result<AdmissionDecision, AdmissionError>
where
    S: AnnouncementRepository + RelayCompatibilityRepository + StateRepository,
{
    let decision = evaluate_request_with_admin_keys(request, admin_keys)?;
    if let Some(relay_url) = request.relay_host() {
        let metrics = RelayCompatibilityMetrics::new();
        match storage.relay_compatibility(relay_url).await {
            Ok(Some(record)) if !record.compatible => {
                metrics.record(false);
                return match mode {
                    RelayCompatibilityMode::Strict => Ok(AdmissionDecision::Reject {
                        reason: format!("relay incompatible: {relay_url}"),
                    }),
                    RelayCompatibilityMode::Warn => {
                        warn!(relay_url = %relay_url, "relay incompatible; allowing");
                        Ok(decision.clone())
                    }
                    RelayCompatibilityMode::Allow => Ok(decision.clone()),
                };
            }
            Ok(Some(record)) => {
                metrics.record(record.compatible);
            }
            Ok(None) => {
                metrics.record(false);
                return match mode {
                    RelayCompatibilityMode::Strict => Ok(AdmissionDecision::Reject {
                        reason: format!("relay compatibility missing: {relay_url}"),
                    }),
                    RelayCompatibilityMode::Warn => {
                        warn!(relay_url = %relay_url, "relay compatibility missing; allowing");
                        Ok(decision.clone())
                    }
                    RelayCompatibilityMode::Allow => Ok(decision.clone()),
                };
            }
            Err(err) => {
                metrics.record(false);
                return match mode {
                    RelayCompatibilityMode::Strict => Ok(AdmissionDecision::Reject {
                        reason: format!("storage error: {err}"),
                    }),
                    RelayCompatibilityMode::Warn => {
                        warn!(relay_url = %relay_url, "storage error on compatibility; allowing");
                        Ok(decision.clone())
                    }
                    RelayCompatibilityMode::Allow => Ok(decision.clone()),
                };
            }
        }
    }
    let AdmissionDecision::RequiresRelatedEvents { filters } = decision else {
        return Ok(decision);
    };

    let address =
        repo_address_from_filters(&filters).or_else(|| repo_address_from_tags(&request.tags).ok());
    let Some(address) = address else {
        return Ok(AdmissionDecision::Reject {
            reason: "missing repo address for related event checks".to_string(),
        });
    };

    match repo_exists(storage, &address).await {
        Ok(true) => Ok(AdmissionDecision::RequiresRelatedEvents { filters }),
        Ok(false) => Ok(AdmissionDecision::Reject {
            reason: "repository not found for related event checks".to_string(),
        }),
        Err(err) => Ok(AdmissionDecision::Reject {
            reason: format!("storage error: {err}"),
        }),
    }
}

pub async fn evaluate_request_cached<S>(
    request: &AdmissionRequest,
    storage: &S,
    cache: &AdmissionCache,
) -> Result<AdmissionDecision, AdmissionError>
where
    S: AnnouncementRepository + RelayCompatibilityRepository + StateRepository,
{
    evaluate_request_cached_mode(request, storage, cache, RelayCompatibilityMode::Strict, &[]).await
}

pub async fn evaluate_request_cached_mode<S>(
    request: &AdmissionRequest,
    storage: &S,
    cache: &AdmissionCache,
    mode: RelayCompatibilityMode,
    admin_keys: &[String],
) -> Result<AdmissionDecision, AdmissionError>
where
    S: AnnouncementRepository + RelayCompatibilityRepository + StateRepository,
{
    let key = cache.cache_key(request);
    if let Some(cached) = cache.get(&key) {
        return Ok(cached);
    }

    let decision = evaluate_request_with_storage_mode(request, storage, mode, admin_keys).await?;
    cache.insert(key, decision.clone());
    Ok(decision)
}

fn is_control_admin(pubkey: &str, admin_keys: &[String]) -> bool {
    admin_keys
        .iter()
        .any(|key| key.eq_ignore_ascii_case(pubkey))
}

fn repo_address_from_tags(tags: &[Vec<String>]) -> Result<RepoAddress, CoreError> {
    for tag in tags {
        let Some((kind, rest)) = tag.split_first() else {
            continue;
        };
        if kind == "a" || kind == "A" {
            let Some(value) = rest.first() else {
                return Err(CoreError::InvalidTag {
                    tag: "a",
                    value: String::new(),
                });
            };
            return RepoAddress::parse(value);
        }
    }
    Err(CoreError::MissingField("a"))
}

fn repo_address_from_filters(filters: &[EventFilter]) -> Option<RepoAddress> {
    for filter in filters {
        if let Some(addresses) = filter.tags.get("a") {
            for value in addresses {
                if let Ok(address) = RepoAddress::parse(value) {
                    return Some(address);
                }
            }
        }
    }
    None
}

async fn repo_exists<S>(storage: &S, address: &RepoAddress) -> Result<bool, StorageError>
where
    S: AnnouncementRepository + StateRepository,
{
    let pubkey = hex::decode(&address.pubkey).map_err(|_| StorageError::InvalidHex {
        field: "pubkey",
        value: address.pubkey.clone(),
    })?;
    let announcement = storage
        .latest_announcement(&pubkey, &address.identifier)
        .await?;
    if announcement.is_some() {
        return Ok(true);
    }
    let state = storage.latest_state(&pubkey, &address.identifier).await?;
    Ok(state.is_some())
}

impl std::fmt::Display for AdmissionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AdmissionError::Config(err) => write!(f, "admission config error: {err}"),
            AdmissionError::Request(err) => write!(f, "admission request error: {err}"),
            AdmissionError::Core(err) => write!(f, "admission core error: {err}"),
            AdmissionError::ObservabilityConfig(err) => {
                write!(f, "admission observability config error: {err}")
            }
            AdmissionError::Observability(err) => {
                write!(f, "admission observability error: {err}")
            }
            AdmissionError::Storage(err) => write!(f, "admission storage error: {err}"),
            AdmissionError::Serve(err) => write!(f, "admission serve error: {err}"),
        }
    }
}

impl std::error::Error for AdmissionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            AdmissionError::Config(err) => Some(err),
            AdmissionError::Request(err) => Some(err),
            AdmissionError::Core(err) => Some(err),
            AdmissionError::ObservabilityConfig(err) => Some(err),
            AdmissionError::Observability(err) => Some(err),
            AdmissionError::Storage(err) => Some(err),
            AdmissionError::Serve(_) => None,
        }
    }
}

pub fn init_observability() -> Result<ObservabilityHandle, AdmissionError> {
    let config = gittree_observability::ObservabilityConfig::from_env("gittree-admission")
        .map_err(AdmissionError::ObservabilityConfig)?;
    let handle = gittree_observability::init(&config).map_err(AdmissionError::Observability)?;
    Ok(handle)
}

pub type AdmissionRepositories = CachedRepositories<PostgresRepositories>;

pub fn build_repositories(
    config: &AdmissionConfig,
) -> Result<AdmissionRepositories, AdmissionError> {
    let pool_options = config
        .storage
        .pool_options()
        .map_err(AdmissionError::Storage)?;
    let connect_options = config
        .storage
        .read_connect_options()
        .map_err(AdmissionError::Storage)?;
    let pool = pool_options.connect_lazy_with(connect_options);
    let repos = PostgresRepositories::new(pool);
    Ok(CachedRepositories::new(repos))
}

struct AdmissionAppState<R> {
    repositories: Arc<R>,
    cache: Arc<AdmissionCache>,
    compatibility: RelayCompatibilityMode,
    control_admin_keys: Vec<String>,
}

impl<R> Clone for AdmissionAppState<R> {
    fn clone(&self) -> Self {
        Self {
            repositories: Arc::clone(&self.repositories),
            cache: Arc::clone(&self.cache),
            compatibility: self.compatibility,
            control_admin_keys: self.control_admin_keys.clone(),
        }
    }
}

async fn serve_with<InitFn, InitOut, ServeFn, ServeFut>(
    config: AdmissionConfig,
    init_fn: InitFn,
    serve_fn: ServeFn,
) -> Result<(), AdmissionError>
where
    InitFn: FnOnce() -> Result<InitOut, AdmissionError>,
    ServeFn: FnOnce(tokio::net::TcpListener, Router) -> ServeFut,
    ServeFut: Future<Output = Result<(), std::io::Error>>,
{
    let _observability = init_fn()?;
    let repositories = build_repositories(&config)?;
    let cache = Arc::new(AdmissionCache::new(AdmissionCacheConfig::default()));
    let state = AdmissionAppState {
        repositories: Arc::new(repositories),
        cache,
        compatibility: config.compatibility.mode,
        control_admin_keys: config.control_admin_keys,
    };
    let router = build_router(state);
    let listener = tokio::net::TcpListener::bind(&config.bind)
        .await
        .map_err(|err| AdmissionError::Serve(err.to_string()))?;
    serve_fn(listener, router)
        .await
        .map_err(|err| AdmissionError::Serve(err.to_string()))?;
    Ok(())
}

pub async fn serve(config: AdmissionConfig) -> Result<(), AdmissionError> {
    serve_with(config, init_observability, run_axum_server).await
}

fn run_axum_server(
    listener: tokio::net::TcpListener,
    router: Router,
) -> impl Future<Output = Result<(), std::io::Error>> {
    async move { axum::serve(listener, router).await }
}

fn build_router<R>(state: AdmissionAppState<R>) -> Router
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
        .route("/decide", post(decide_handler))
        .with_state(state)
}

async fn health_handler() -> &'static str {
    "ok"
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdmissionRequestPayload {
    pub kind: u64,
    pub pubkey: String,
    pub event_id: String,
    pub tags: Vec<Vec<String>>,
    #[serde(default)]
    pub relay_url: Option<String>,
    #[serde(default)]
    pub source_ip: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum AdmissionDecisionPayload {
    Accept,
    Reject { reason: String },
    RequiresRelatedEvents { filters: Vec<AdmissionFilter> },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdmissionFilter {
    pub ids: Vec<String>,
    pub kinds: Vec<u32>,
    pub authors: Vec<String>,
    pub tags: BTreeMap<String, Vec<String>>,
    pub limit: Option<u64>,
}

impl From<AdmissionFilter> for EventFilter {
    fn from(filter: AdmissionFilter) -> Self {
        Self {
            ids: filter.ids,
            kinds: filter.kinds,
            authors: filter.authors,
            tags: filter.tags,
            limit: filter.limit,
        }
    }
}

impl From<&EventFilter> for AdmissionFilter {
    fn from(filter: &EventFilter) -> Self {
        Self {
            ids: filter.ids.clone(),
            kinds: filter.kinds.clone(),
            authors: filter.authors.clone(),
            tags: filter.tags.clone(),
            limit: filter.limit,
        }
    }
}

impl From<AdmissionDecision> for AdmissionDecisionPayload {
    fn from(decision: AdmissionDecision) -> Self {
        match decision {
            AdmissionDecision::Accept => AdmissionDecisionPayload::Accept,
            AdmissionDecision::Reject { reason } => AdmissionDecisionPayload::Reject { reason },
            AdmissionDecision::RequiresRelatedEvents { filters } => {
                AdmissionDecisionPayload::RequiresRelatedEvents {
                    filters: filters.iter().map(AdmissionFilter::from).collect(),
                }
            }
        }
    }
}

#[derive(Debug)]
enum AdmissionHttpError {
    BadRequest(String),
    Internal(String),
}

impl From<AdmissionError> for AdmissionHttpError {
    fn from(err: AdmissionError) -> Self {
        match err {
            AdmissionError::Request(err) => AdmissionHttpError::BadRequest(err.to_string()),
            AdmissionError::Core(err) => AdmissionHttpError::BadRequest(err.to_string()),
            AdmissionError::Storage(err) => AdmissionHttpError::Internal(err.to_string()),
            AdmissionError::Config(err) => AdmissionHttpError::Internal(err.to_string()),
            AdmissionError::ObservabilityConfig(err) => {
                AdmissionHttpError::Internal(err.to_string())
            }
            AdmissionError::Observability(err) => AdmissionHttpError::Internal(err.to_string()),
            AdmissionError::Serve(err) => AdmissionHttpError::Internal(err),
        }
    }
}

impl IntoResponse for AdmissionHttpError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            AdmissionHttpError::BadRequest(message) => (StatusCode::BAD_REQUEST, message),
            AdmissionHttpError::Internal(message) => (StatusCode::INTERNAL_SERVER_ERROR, message),
        };
        (status, message).into_response()
    }
}

async fn decide_handler<R>(
    State(state): State<AdmissionAppState<R>>,
    Json(payload): Json<AdmissionRequestPayload>,
) -> Result<Json<AdmissionDecisionPayload>, AdmissionHttpError>
where
    R: AnnouncementRepository + RelayCompatibilityRepository + StateRepository + Send + Sync,
{
    let request = AdmissionRequest::new(
        payload.kind,
        payload.pubkey,
        payload.event_id,
        payload.tags,
        payload.relay_url,
        payload.source_ip,
    )
    .map_err(|err| AdmissionHttpError::BadRequest(err.to_string()))?;
    let decision = evaluate_request_cached_mode(
        &request,
        state.repositories.as_ref(),
        state.cache.as_ref(),
        state.compatibility,
        &state.control_admin_keys,
    )
    .await
    .map_err(AdmissionHttpError::from)?;
    Ok(Json(decision.into()))
}

#[cfg(test)]
mod tests {
    use super::AdmissionCache;
    use super::AdmissionCacheConfig;
    use super::AdmissionConfig;
    use super::AdmissionConfigError;
    use super::AdmissionDecision;
    use super::AdmissionDecisionError;
    use super::AdmissionDecisionPayload;
    use super::AdmissionError;
    use super::AdmissionRequest;
    use super::AdmissionRequestError;
    use super::AdmissionRequestPayload;
    use super::RateLimitConfig;
    use super::RateLimiter;
    use super::RepoAddress;
    use super::StorageConfigError;
    use super::evaluate_request;
    use super::evaluate_request_cached;
    use super::evaluate_request_with_admin_keys;
    use super::evaluate_request_with_storage;
    use super::evaluate_request_with_storage_mode;
    use async_trait::async_trait;
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use axum::response::IntoResponse;
    use axum::routing::get;
    use gittree_config::{RelayCompatibilityConfig, RelayCompatibilityMode, ServicesConfig};
    use gittree_core::EventFilter;
    use gittree_core::RepoAnnouncement;
    use gittree_core::kinds::{
        KIND_GIT_PATCH, KIND_GIT_REPO_ANNOUNCEMENT, KIND_GIT_REPO_STATE, KIND_GITTREE_CONTROL,
    };
    use gittree_core::{RelayCapability, RelayCompatibilityReport, RepoState};
    use gittree_storage::{
        AnnouncementRepository, InMemoryRepositories, RelayCompatibilityRecord,
        RelayCompatibilityRepository, RepoAnnouncementRecord,
    };
    use gittree_storage::{RelayProbeMetadata, StateRepository, StorageConfig, StorageError};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tower::ServiceExt;

    #[test]
    fn config_loads_from_env() {
        with_env_var(
            super::ENV_STORAGE_READ_URL,
            "postgres://localhost/test",
            || {
                with_env_var(super::ENV_CONTROL_ADMIN_KEYS, "alpha, beta", || {
                    let config = AdmissionConfig::from_env().expect("config");
                    let services = ServicesConfig::from_env_validated().expect("services");
                    assert_eq!(config.bind, services.admission.bind);
                    assert_eq!(
                        config.control_admin_keys,
                        vec!["alpha".to_string(), "beta".to_string()]
                    );
                });
            },
        );
    }

    #[test]
    fn config_requires_storage_read_url() {
        with_env_removed(super::ENV_STORAGE_READ_URL, || {
            let err = AdmissionConfig::from_env().expect_err("missing read url");
            assert!(matches!(
                err,
                AdmissionConfigError::Storage(StorageConfigError::MissingEnv(
                    super::ENV_STORAGE_READ_URL
                ))
            ));
            assert!(err.to_string().contains("admission storage config error"));
            assert!(std::error::Error::source(&err).is_some());
        });
    }

    #[test]
    fn config_rejects_invalid_numeric_env_values() {
        with_env_var(
            super::ENV_STORAGE_READ_URL,
            "postgres://localhost/test",
            || {
                with_env_var(super::ENV_STORAGE_MAX_CONNECTIONS, "not-a-number", || {
                    let err = AdmissionConfig::from_env().expect_err("invalid max connections");
                    assert!(matches!(
                        err,
                        AdmissionConfigError::Storage(StorageConfigError::InvalidEnv {
                            key: super::ENV_STORAGE_MAX_CONNECTIONS,
                            ..
                        })
                    ));
                });

                with_env_var(super::ENV_STORAGE_IDLE_TIMEOUT_SECS, "NaN", || {
                    let err = AdmissionConfig::from_env().expect_err("invalid idle timeout");
                    assert!(matches!(
                        err,
                        AdmissionConfigError::Storage(StorageConfigError::InvalidEnv {
                            key: super::ENV_STORAGE_IDLE_TIMEOUT_SECS,
                            ..
                        })
                    ));
                });
            },
        );
    }

    #[test]
    fn config_ignores_empty_numeric_values_and_missing_admin_keys() {
        with_env_var(
            super::ENV_STORAGE_READ_URL,
            "postgres://localhost/test",
            || {
                with_env_var(super::ENV_STORAGE_MAX_CONNECTIONS, "   ", || {
                    with_env_var(super::ENV_STORAGE_IDLE_TIMEOUT_SECS, " ", || {
                        with_env_removed(super::ENV_CONTROL_ADMIN_KEYS, || {
                            let config = AdmissionConfig::from_env().expect("config");
                            assert_eq!(config.storage.max_connections, 10);
                            assert_eq!(config.storage.idle_timeout_secs, None);
                            assert!(config.control_admin_keys.is_empty());
                        });
                    });
                });
            },
        );
    }

    #[test]
    fn config_rejects_invalid_pool_bounds() {
        with_env_var(
            super::ENV_STORAGE_READ_URL,
            "postgres://localhost/test",
            || {
                with_env_var(super::ENV_STORAGE_MAX_CONNECTIONS, "1", || {
                    with_env_var(super::ENV_STORAGE_MIN_CONNECTIONS, "2", || {
                        let err = AdmissionConfig::from_env().expect_err("invalid pool bounds");
                        assert!(matches!(
                            err,
                            AdmissionConfigError::Storage(StorageConfigError::InvalidConfig(_))
                        ));
                    });
                });
            },
        );
    }

    #[test]
    fn config_error_display_and_source_cover_variants() {
        let config_err = AdmissionConfigError::Config(gittree_config::ConfigError::MissingEnv(
            "GITTREE_SERVICES_JSON",
        ));
        assert!(config_err.to_string().contains("admission config error"));
        assert!(std::error::Error::source(&config_err).is_some());

        let storage_err = AdmissionConfigError::Storage(StorageConfigError::InvalidConfig(
            "pool bounds invalid".to_string(),
        ));
        assert!(
            storage_err
                .to_string()
                .contains("admission storage config error")
        );
        assert!(std::error::Error::source(&storage_err).is_some());

        let invalid_env = StorageConfigError::InvalidEnv {
            key: "KEY",
            value: "bad".to_string(),
        };
        assert_eq!(invalid_env.to_string(), "invalid env KEY: bad");
    }

    #[tokio::test]
    async fn health_endpoint_returns_ok() {
        let repositories = std::sync::Arc::new(InMemoryRepositories::new());
        let cache = std::sync::Arc::new(AdmissionCache::new(AdmissionCacheConfig::default()));
        let app = super::build_router(super::AdmissionAppState {
            repositories,
            cache,
            compatibility: RelayCompatibilityMode::Strict,
            control_admin_keys: Vec::new(),
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
    async fn decide_endpoint_accepts_state_event() {
        let repositories = std::sync::Arc::new(InMemoryRepositories::new());
        let cache = std::sync::Arc::new(AdmissionCache::new(AdmissionCacheConfig::default()));
        let app = super::build_router(super::AdmissionAppState {
            repositories,
            cache,
            compatibility: RelayCompatibilityMode::Strict,
            control_admin_keys: Vec::new(),
        });
        let payload = AdmissionRequestPayload {
            kind: KIND_GIT_REPO_STATE.0 as u64,
            pubkey: "pubkey".to_string(),
            event_id: "event".to_string(),
            tags: Vec::new(),
            relay_url: None,
            source_ip: None,
        };
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/decide")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&payload).expect("body")))
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let decision: AdmissionDecisionPayload = serde_json::from_slice(&body).expect("decision");
        assert!(matches!(decision, AdmissionDecisionPayload::Accept));
    }

    #[tokio::test]
    async fn decide_endpoint_rejects_invalid_payload() {
        let repositories = Arc::new(InMemoryRepositories::new());
        let cache = Arc::new(AdmissionCache::new(AdmissionCacheConfig::default()));
        let app = super::build_router(super::AdmissionAppState {
            repositories,
            cache,
            compatibility: RelayCompatibilityMode::Strict,
            control_admin_keys: Vec::new(),
        });
        let payload = AdmissionRequestPayload {
            kind: KIND_GIT_REPO_STATE.0 as u64,
            pubkey: String::new(),
            event_id: "event".to_string(),
            tags: Vec::new(),
            relay_url: None,
            source_ip: None,
        };
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/decide")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&payload).expect("body")))
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), axum::http::StatusCode::BAD_REQUEST);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        assert!(String::from_utf8_lossy(&body).contains("missing admission field pubkey"));
    }

    #[test]
    fn request_rejects_missing_pubkey() {
        let err = AdmissionRequest::new(1, "", "event", vec![vec!["d".to_string()]], None, None)
            .unwrap_err();
        assert!(matches!(
            err,
            super::AdmissionRequestError::MissingField("pubkey")
        ));
    }

    #[test]
    fn request_rejects_missing_event_id() {
        let err = AdmissionRequest::new(1, "pubkey", "", vec![], None, None).unwrap_err();
        assert!(matches!(
            err,
            super::AdmissionRequestError::MissingField("event_id")
        ));
    }

    #[test]
    fn request_rejects_empty_tag() {
        let err =
            AdmissionRequest::new(1, "pubkey", "event", vec![vec![]], None, None).unwrap_err();
        assert!(matches!(err, super::AdmissionRequestError::InvalidTag));
    }

    #[test]
    fn request_rejects_empty_relay_url() {
        let err = AdmissionRequest::new(
            1,
            "pubkey",
            "event",
            vec![vec!["d".to_string(), "repo".to_string()]],
            Some("  ".to_string()),
            None,
        )
        .unwrap_err();
        assert!(matches!(err, super::AdmissionRequestError::InvalidRelayUrl));
    }

    #[test]
    fn request_accepts_valid_payload() {
        let request = AdmissionRequest::new(
            1,
            "pubkey",
            "event",
            vec![vec!["d".to_string(), "repo".to_string()]],
            Some("wss://relay.example".to_string()),
            None,
        )
        .expect("request");
        assert_eq!(request.relay_host(), Some("wss://relay.example"));
    }

    #[test]
    fn control_request_accepts_admin_key() {
        let request = AdmissionRequest::new(
            KIND_GITTREE_CONTROL.0 as u64,
            "adminpub",
            "event",
            Vec::new(),
            None,
            None,
        )
        .expect("request");
        let decision = evaluate_request_with_admin_keys(&request, &["adminpub".to_string()])
            .expect("decision");
        assert!(matches!(decision, AdmissionDecision::Accept));
    }

    #[test]
    fn control_request_rejects_unknown_key() {
        let request = AdmissionRequest::new(
            KIND_GITTREE_CONTROL.0 as u64,
            "otherpub",
            "event",
            Vec::new(),
            None,
            None,
        )
        .expect("request");
        let decision = evaluate_request_with_admin_keys(&request, &["adminpub".to_string()])
            .expect("decision");
        assert!(matches!(decision, AdmissionDecision::Reject { .. }));
    }

    #[test]
    fn decision_reject_requires_reason() {
        let err = AdmissionDecision::reject(" ").unwrap_err();
        assert!(matches!(err, super::AdmissionDecisionError::MissingReason));
    }

    #[test]
    fn decision_reject_accepts_reason() {
        let decision = AdmissionDecision::reject("denied").expect("decision");
        assert!(matches!(
            decision,
            AdmissionDecision::Reject { reason } if reason == "denied"
        ));
    }

    #[test]
    fn decision_requires_filters() {
        let err = AdmissionDecision::requires_related(Vec::new()).unwrap_err();
        assert!(matches!(err, super::AdmissionDecisionError::MissingFilters));
    }

    #[test]
    fn decision_accepts_related_filters() {
        let mut filter = EventFilter::new();
        filter.kinds = vec![1];
        let filters = vec![filter];
        let decision = AdmissionDecision::requires_related(filters.clone()).expect("decision");
        assert!(matches!(
            decision,
            AdmissionDecision::RequiresRelatedEvents { filters } if filters.len() == 1
        ));
    }

    #[test]
    fn request_and_decision_error_display_messages_are_stable() {
        assert_eq!(
            AdmissionRequestError::MissingField("pubkey").to_string(),
            "missing admission field pubkey"
        );
        assert_eq!(
            AdmissionRequestError::InvalidTag.to_string(),
            "invalid admission tag"
        );
        assert_eq!(
            AdmissionRequestError::InvalidKind(999).to_string(),
            "invalid admission kind 999"
        );
        assert_eq!(
            AdmissionRequestError::InvalidRelayUrl.to_string(),
            "invalid admission relay url"
        );
        assert_eq!(
            AdmissionDecisionError::MissingReason.to_string(),
            "missing admission rejection reason"
        );
        assert_eq!(
            AdmissionDecisionError::MissingFilters.to_string(),
            "missing related event filters"
        );
    }

    #[test]
    fn admission_error_display_and_source_are_stable() {
        let request = AdmissionError::Request(AdmissionRequestError::InvalidTag);
        assert_eq!(
            request.to_string(),
            "admission request error: invalid admission tag"
        );
        assert!(std::error::Error::source(&request).is_some());

        let core = AdmissionError::Core(gittree_core::CoreError::MissingField("a"));
        assert!(core.to_string().contains("admission core error"));
        assert!(std::error::Error::source(&core).is_some());

        let config = AdmissionError::Config(AdmissionConfigError::Storage(
            StorageConfigError::InvalidConfig("bad".to_string()),
        ));
        assert!(
            config
                .to_string()
                .contains("admission config error: admission storage config error")
        );
        assert!(std::error::Error::source(&config).is_some());

        let storage = AdmissionError::Storage(StorageError::Internal {
            message: "fail".to_string(),
        });
        assert!(storage.to_string().contains("admission storage error"));
        assert!(std::error::Error::source(&storage).is_some());

        let observability_config = AdmissionError::ObservabilityConfig(
            gittree_observability::ObservabilityConfigError::InvalidEnv {
                key: "GITTREE_LOG_JSON",
                value: "wat".to_string(),
            },
        );
        assert!(
            observability_config
                .to_string()
                .contains("admission observability config error")
        );
        assert!(std::error::Error::source(&observability_config).is_some());

        let observability = AdmissionError::Observability(
            gittree_observability::ObservabilityError::SubscriberInit("dup".to_string()),
        );
        assert!(
            observability
                .to_string()
                .contains("admission observability error")
        );
        assert!(std::error::Error::source(&observability).is_some());

        let serve = AdmissionError::Serve("bind failed".to_string());
        assert_eq!(serve.to_string(), "admission serve error: bind failed");
        assert!(std::error::Error::source(&serve).is_none());
    }

    #[test]
    fn admission_http_error_maps_status_codes() {
        let bad_request_response = super::AdmissionHttpError::from(AdmissionError::Request(
            AdmissionRequestError::InvalidTag,
        ))
        .into_response();
        assert_eq!(bad_request_response.status(), StatusCode::BAD_REQUEST);

        let internal_response =
            super::AdmissionHttpError::from(AdmissionError::Serve("bind failed".to_string()))
                .into_response();
        assert_eq!(
            internal_response.status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );

        let internal_from_core = super::AdmissionHttpError::from(AdmissionError::Core(
            gittree_core::CoreError::InvalidTag {
                tag: "a",
                value: "bad".to_string(),
            },
        ))
        .into_response();
        assert_eq!(internal_from_core.status(), StatusCode::BAD_REQUEST);

        let internal_from_storage =
            super::AdmissionHttpError::from(AdmissionError::Storage(StorageError::Internal {
                message: "boom".to_string(),
            }))
            .into_response();
        assert_eq!(
            internal_from_storage.status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );

        let internal_from_config = super::AdmissionHttpError::from(AdmissionError::Config(
            AdmissionConfigError::Storage(StorageConfigError::MissingEnv("MISSING")),
        ))
        .into_response();
        assert_eq!(
            internal_from_config.status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );

        let internal_from_obs_config =
            super::AdmissionHttpError::from(AdmissionError::ObservabilityConfig(
                gittree_observability::ObservabilityConfigError::InvalidEnv {
                    key: "GITTREE_LOG_JSON",
                    value: "invalid".to_string(),
                },
            ))
            .into_response();
        assert_eq!(
            internal_from_obs_config.status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );

        let internal_from_obs = super::AdmissionHttpError::from(AdmissionError::Observability(
            gittree_observability::ObservabilityError::SubscriberInit("dup".to_string()),
        ))
        .into_response();
        assert_eq!(
            internal_from_obs.status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }

    #[tokio::test]
    async fn build_repositories_covers_success_and_invalid_pool_settings() {
        let mut config = AdmissionConfig {
            bind: "127.0.0.1:0".to_string(),
            compatibility: RelayCompatibilityConfig::default(),
            storage: StorageConfig {
                read_connection: "postgres://user:pass@localhost:5432/gittree".to_string(),
                write_connection: None,
                max_connections: 4,
                min_connections: 2,
                idle_timeout_secs: None,
                max_lifetime_secs: None,
                application_name: None,
            },
            control_admin_keys: Vec::new(),
        };
        let _ = super::build_repositories(&config).expect("repositories");

        config.storage.max_connections = 1;
        config.storage.min_connections = 2;
        let err = super::build_repositories(&config).expect_err("invalid pool settings");
        assert!(matches!(err, AdmissionError::Storage(_)));
    }

    #[test]
    fn init_observability_reports_invalid_env() {
        with_env_var("GITTREE_LOG_JSON", "not-bool", || {
            let err = super::init_observability().expect_err("invalid observability env");
            assert!(matches!(err, AdmissionError::ObservabilityConfig(_)));
        });
    }

    #[test]
    fn serve_returns_bind_error_for_invalid_bind() {
        with_env_removed("GITTREE_LOG_JSON", || {
            let config = AdmissionConfig {
                bind: "invalid-bind".to_string(),
                compatibility: RelayCompatibilityConfig::default(),
                storage: StorageConfig {
                    read_connection: "postgres://user:pass@localhost:5432/gittree".to_string(),
                    write_connection: None,
                    max_connections: 4,
                    min_connections: 2,
                    idle_timeout_secs: None,
                    max_lifetime_secs: None,
                    application_name: None,
                },
                control_admin_keys: Vec::new(),
            };
            let runtime = tokio::runtime::Runtime::new().expect("runtime");
            let result = runtime.block_on(super::serve(config));
            assert!(matches!(
                result,
                Err(AdmissionError::Serve(_)) | Err(AdmissionError::Observability(_))
            ));
        });
    }

    #[tokio::test]
    async fn serve_with_returns_ok_when_server_finishes_cleanly() {
        let config = AdmissionConfig {
            bind: "127.0.0.1:0".to_string(),
            compatibility: RelayCompatibilityConfig::default(),
            storage: StorageConfig {
                read_connection: "postgres://user:pass@localhost:5432/gittree".to_string(),
                write_connection: None,
                max_connections: 4,
                min_connections: 2,
                idle_timeout_secs: None,
                max_lifetime_secs: None,
                application_name: None,
            },
            control_admin_keys: Vec::new(),
        };

        let result = super::serve_with(
            config,
            || Ok(()),
            |_listener, _router| async { Ok::<(), std::io::Error>(()) },
        )
        .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn serve_with_maps_server_errors() {
        let config = AdmissionConfig {
            bind: "127.0.0.1:0".to_string(),
            compatibility: RelayCompatibilityConfig::default(),
            storage: StorageConfig {
                read_connection: "postgres://user:pass@localhost:5432/gittree".to_string(),
                write_connection: None,
                max_connections: 4,
                min_connections: 2,
                idle_timeout_secs: None,
                max_lifetime_secs: None,
                application_name: None,
            },
            control_admin_keys: Vec::new(),
        };

        let err = super::serve_with(
            config,
            || Ok(()),
            |_listener, _router| async { Err(std::io::Error::other("boom")) },
        )
        .await
        .expect_err("server error");
        assert!(matches!(err, AdmissionError::Serve(_)));
    }

    #[tokio::test]
    async fn run_axum_server_can_start_and_be_aborted() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener");
        let addr = listener.local_addr().expect("addr");
        let app = axum::Router::new().route("/health", get(super::health_handler));

        let task = tokio::spawn(async move { super::run_axum_server(listener, app).await });
        tokio::time::sleep(Duration::from_millis(25)).await;

        let mut stream = tokio::net::TcpStream::connect(addr)
            .await
            .expect("connect health socket");
        stream
            .write_all(b"GET /health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .await
            .expect("write request");
        let mut response = Vec::new();
        stream
            .read_to_end(&mut response)
            .await
            .expect("read response");
        assert!(response.starts_with(b"HTTP/1.1 200"));

        task.abort();
        let join_result = task.await.expect_err("aborted");
        assert!(join_result.is_cancelled());
    }

    #[test]
    fn rate_limiter_rejects_ip_over_limit() {
        let limiter = RateLimiter::new(RateLimitConfig::new(1, 0, Duration::from_secs(60)));
        let request = AdmissionRequest::new(
            1,
            "pubkey",
            "event",
            Vec::new(),
            None,
            Some("127.0.0.1".to_string()),
        )
        .expect("request");

        assert!(limiter.check(&request).is_none());
        let decision = limiter.check(&request).expect("reject");
        assert!(matches!(
            decision,
            AdmissionDecision::Reject { reason } if reason.contains("ip")
        ));
    }

    #[test]
    fn rate_limiter_rejects_pubkey_over_limit() {
        let limiter = RateLimiter::new(RateLimitConfig::new(0, 1, Duration::from_secs(60)));
        let request =
            AdmissionRequest::new(1, "pubkey", "event", Vec::new(), None, None).expect("request");

        assert!(limiter.check(&request).is_none());
        let decision = limiter.check(&request).expect("reject");
        assert!(matches!(
            decision,
            AdmissionDecision::Reject { reason } if reason.contains("pubkey")
        ));
    }

    #[test]
    fn evaluate_request_accepts_state() {
        let request = AdmissionRequest::new(
            KIND_GIT_REPO_STATE.0 as u64,
            "pubkey",
            "event",
            Vec::new(),
            Some("relay".to_string()),
            None,
        )
        .expect("request");
        let decision = evaluate_request(&request).expect("decision");
        assert!(matches!(decision, AdmissionDecision::Accept));
    }

    #[test]
    fn evaluate_request_requires_related() {
        let request = AdmissionRequest::new(
            KIND_GIT_PATCH.0 as u64,
            "pubkey",
            "event",
            Vec::new(),
            Some("relay".to_string()),
            None,
        )
        .expect("request");
        let decision = evaluate_request(&request).expect("decision");
        assert!(matches!(
            decision,
            AdmissionDecision::RequiresRelatedEvents { .. }
        ));
    }

    #[test]
    fn evaluate_request_maps_core_reject_decision() {
        let announcement = sample_announcement("repo");
        let request = AdmissionRequest::new(
            KIND_GIT_REPO_ANNOUNCEMENT.0 as u64,
            "pubkey",
            "event",
            announcement.to_tags(),
            None,
            None,
        )
        .expect("request");
        let decision = evaluate_request(&request).expect("decision");
        assert!(matches!(
            decision,
            AdmissionDecision::Reject { reason } if reason.contains("missing relay host")
        ));
    }

    #[test]
    fn evaluate_request_rejects_invalid_kind() {
        let request = AdmissionRequest::new(
            u64::from(u32::MAX) + 1,
            "pubkey",
            "event",
            Vec::new(),
            Some("relay".to_string()),
            None,
        )
        .expect("request");
        let err = evaluate_request(&request).unwrap_err();
        assert!(matches!(err, super::AdmissionError::Request(_)));
    }

    fn hex_32(byte: u8) -> String {
        format!("{:02x}", byte).repeat(32)
    }

    fn sample_announcement(identifier: &str) -> RepoAnnouncement {
        RepoAnnouncement {
            identifier: identifier.to_string(),
            name: None,
            description: None,
            root_commit: None,
            clone: vec!["https://git.example/repo.git".to_string()],
            web: Vec::new(),
            relays: vec!["wss://relay.example".to_string()],
            blossoms: Vec::new(),
            hashtags: Vec::new(),
            maintainers: Vec::new(),
        }
    }

    fn sample_compat_record(relay_url: &str, compatible: bool) -> RelayCompatibilityRecord {
        let (supported, missing_required) = if compatible {
            (
                vec![RelayCapability::Nip01, RelayCapability::Nip34],
                Vec::new(),
            )
        } else {
            (vec![RelayCapability::Nip01], vec![RelayCapability::Nip34])
        };
        let report = RelayCompatibilityReport {
            relay_url: relay_url.to_string(),
            supported,
            missing_required,
            missing_optional: Vec::new(),
        };
        RelayCompatibilityRecord::new(&report, 0, &RelayProbeMetadata::default()).expect("record")
    }

    #[tokio::test]
    async fn storage_integration_rejects_missing_repo() {
        let storage = InMemoryRepositories::new();
        let pubkey = hex_32(0x11);
        let address = format!("30617:{pubkey}:repo");
        let request = AdmissionRequest::new(
            KIND_GIT_PATCH.0 as u64,
            "pubkey",
            "event",
            vec![vec!["a".to_string(), address]],
            Some("relay".to_string()),
            None,
        )
        .expect("request");

        let decision = evaluate_request_with_storage(&request, &storage)
            .await
            .expect("decision");
        assert!(matches!(decision, AdmissionDecision::Reject { .. }));
    }

    #[tokio::test]
    async fn storage_integration_rejects_missing_repo_without_relay_host() {
        let storage = InMemoryRepositories::new();
        let pubkey = hex_32(0x11);
        let address = format!("30617:{pubkey}:repo");
        let request = AdmissionRequest::new(
            KIND_GIT_PATCH.0 as u64,
            "pubkey",
            "event",
            vec![vec!["a".to_string(), address]],
            None,
            None,
        )
        .expect("request");

        let decision = evaluate_request_with_storage(&request, &storage)
            .await
            .expect("decision");
        assert!(matches!(
            decision,
            AdmissionDecision::Reject { reason }
                if reason.contains("repository not found for related event checks")
        ));
    }

    #[tokio::test]
    async fn storage_integration_keeps_related_when_repo_exists() {
        let storage = InMemoryRepositories::new();
        let pubkey = hex_32(0x11);
        let event_id = hex_32(0x22);
        let announcement = sample_announcement("repo");
        let record =
            RepoAnnouncementRecord::new(&event_id, &pubkey, 10, &announcement).expect("record");
        storage.insert_announcement(record).await.expect("insert");
        let compat_record = sample_compat_record("wss://relay.example", true);
        storage
            .upsert_relay_compatibility(compat_record)
            .await
            .expect("compat");

        let address = format!("30617:{pubkey}:repo");
        let request = AdmissionRequest::new(
            KIND_GIT_PATCH.0 as u64,
            "pubkey",
            "event",
            vec![vec!["a".to_string(), address]],
            Some("wss://relay.example".to_string()),
            None,
        )
        .expect("request");

        let decision = evaluate_request_with_storage(&request, &storage)
            .await
            .expect("decision");
        assert!(matches!(
            decision,
            AdmissionDecision::RequiresRelatedEvents { .. }
        ));
    }

    #[tokio::test]
    async fn storage_integration_rejects_incompatible_relay() {
        let storage = InMemoryRepositories::new();
        let record = sample_compat_record("wss://relay.example", false);
        storage
            .upsert_relay_compatibility(record)
            .await
            .expect("upsert");

        let pubkey = hex_32(0x11);
        let address = format!("30617:{pubkey}:repo");
        let request = AdmissionRequest::new(
            KIND_GIT_PATCH.0 as u64,
            "pubkey",
            "event",
            vec![vec!["a".to_string(), address]],
            Some("wss://relay.example".to_string()),
            None,
        )
        .expect("request");

        let decision = evaluate_request_with_storage(&request, &storage)
            .await
            .expect("decision");
        assert!(matches!(
            decision,
            AdmissionDecision::Reject { reason } if reason.contains("relay incompatible")
        ));
    }

    #[tokio::test]
    async fn storage_integration_warns_on_incompatible_relay() {
        let storage = InMemoryRepositories::new();
        let record = sample_compat_record("wss://relay.example", false);
        storage
            .upsert_relay_compatibility(record)
            .await
            .expect("upsert");

        let pubkey = hex_32(0x11);
        let address = format!("30617:{pubkey}:repo");
        let request = AdmissionRequest::new(
            KIND_GIT_PATCH.0 as u64,
            "pubkey",
            "event",
            vec![vec!["a".to_string(), address]],
            Some("wss://relay.example".to_string()),
            None,
        )
        .expect("request");

        let decision = evaluate_request_with_storage_mode(
            &request,
            &storage,
            RelayCompatibilityMode::Warn,
            &[],
        )
        .await
        .expect("decision");
        assert!(matches!(
            decision,
            AdmissionDecision::RequiresRelatedEvents { .. }
        ));
    }

    #[tokio::test]
    async fn storage_integration_allows_missing_compatibility_in_allow_mode() {
        let storage = InMemoryRepositories::new();
        let pubkey = hex_32(0x11);
        let address = format!("30617:{pubkey}:repo");
        let request = AdmissionRequest::new(
            KIND_GIT_PATCH.0 as u64,
            "pubkey",
            "event",
            vec![vec!["a".to_string(), address]],
            Some("wss://relay.example".to_string()),
            None,
        )
        .expect("request");

        let decision = evaluate_request_with_storage_mode(
            &request,
            &storage,
            RelayCompatibilityMode::Allow,
            &[],
        )
        .await
        .expect("decision");
        assert!(matches!(
            decision,
            AdmissionDecision::RequiresRelatedEvents { .. }
        ));
    }

    #[tokio::test]
    async fn storage_integration_allows_incompatible_relay_in_allow_mode() {
        let storage = InMemoryRepositories::new();
        let record = sample_compat_record("wss://relay.example", false);
        storage
            .upsert_relay_compatibility(record)
            .await
            .expect("upsert");

        let pubkey = hex_32(0x11);
        let address = format!("30617:{pubkey}:repo");
        let request = AdmissionRequest::new(
            KIND_GIT_PATCH.0 as u64,
            "pubkey",
            "event",
            vec![vec!["a".to_string(), address]],
            Some("wss://relay.example".to_string()),
            None,
        )
        .expect("request");

        let decision = evaluate_request_with_storage_mode(
            &request,
            &storage,
            RelayCompatibilityMode::Allow,
            &[],
        )
        .await
        .expect("decision");
        assert!(matches!(
            decision,
            AdmissionDecision::RequiresRelatedEvents { .. }
        ));
    }

    #[tokio::test]
    async fn storage_integration_warns_on_missing_compatibility() {
        let storage = InMemoryRepositories::new();
        let pubkey = hex_32(0x11);
        let address = format!("30617:{pubkey}:repo");
        let request = AdmissionRequest::new(
            KIND_GIT_PATCH.0 as u64,
            "pubkey",
            "event",
            vec![vec!["a".to_string(), address]],
            Some("wss://relay.example".to_string()),
            None,
        )
        .expect("request");

        let decision = evaluate_request_with_storage_mode(
            &request,
            &storage,
            RelayCompatibilityMode::Warn,
            &[],
        )
        .await
        .expect("decision");
        assert!(matches!(
            decision,
            AdmissionDecision::RequiresRelatedEvents { .. }
        ));
    }

    #[tokio::test]
    async fn storage_integration_rejects_missing_repo_address() {
        let storage = InMemoryRepositories::new();
        let request = AdmissionRequest::new(
            KIND_GIT_PATCH.0 as u64,
            "pubkey",
            "event",
            Vec::new(),
            None,
            None,
        )
        .expect("request");

        let decision = evaluate_request_with_storage(&request, &storage)
            .await
            .expect("decision");
        assert!(matches!(
            decision,
            AdmissionDecision::Reject { reason }
                if reason.contains("missing repo address for related event checks")
        ));
    }

    #[tokio::test]
    async fn storage_integration_rejects_when_repo_lookup_errors() {
        let storage = FailingStorage;
        let pubkey = hex_32(0x11);
        let address = format!("30617:{pubkey}:repo");
        let request = AdmissionRequest::new(
            KIND_GIT_PATCH.0 as u64,
            "pubkey",
            "event",
            vec![vec!["a".to_string(), address]],
            None,
            None,
        )
        .expect("request");

        let decision = evaluate_request_with_storage(&request, &storage)
            .await
            .expect("decision");
        assert!(matches!(
            decision,
            AdmissionDecision::Reject { reason } if reason.contains("storage error")
        ));
    }

    #[test]
    fn repo_address_helpers_cover_edge_cases() {
        let pubkey = hex_32(0x11);
        let valid = format!("30617:{pubkey}:repo");

        let address = super::repo_address_from_tags(&[
            vec!["x".to_string(), "ignored".to_string()],
            vec!["A".to_string(), valid.clone()],
        ])
        .expect("address");
        assert_eq!(address.identifier, "repo");

        let missing_value_err =
            super::repo_address_from_tags(&[vec!["a".to_string()]]).expect_err("missing value");
        assert!(matches!(
            missing_value_err,
            gittree_core::CoreError::InvalidTag { tag: "a", .. }
        ));

        let missing_err =
            super::repo_address_from_tags(&[vec!["d".to_string(), "repo".to_string()]])
                .expect_err("missing a tag");
        assert!(matches!(
            missing_err,
            gittree_core::CoreError::MissingField("a")
        ));

        let mut valid_filter = EventFilter::new();
        valid_filter.tags.insert("a".to_string(), vec![valid]);
        let mut invalid_filter = EventFilter::new();
        invalid_filter
            .tags
            .insert("a".to_string(), vec!["30617:nothex:repo".to_string()]);
        let resolved =
            super::repo_address_from_filters(&[invalid_filter, valid_filter]).expect("resolved");
        assert_eq!(resolved.identifier, "repo");

        let address_with_empty_tag = super::repo_address_from_tags(&[
            Vec::new(),
            vec!["a".to_string(), format!("30617:{}:repo", hex_32(0x22))],
        ])
        .expect("address with empty tag ignored");
        assert_eq!(address_with_empty_tag.identifier, "repo");
    }

    #[tokio::test]
    async fn repo_exists_covers_invalid_hex_and_state_lookup_paths() {
        let storage = InMemoryRepositories::new();
        let invalid = RepoAddress {
            pubkey: "nothex".to_string(),
            identifier: "repo".to_string(),
        };
        let err = super::repo_exists(&storage, &invalid)
            .await
            .expect_err("invalid pubkey");
        assert!(matches!(
            err,
            StorageError::InvalidHex {
                field: "pubkey",
                ..
            }
        ));

        let pubkey = hex_32(0x33);
        let address = RepoAddress {
            pubkey: pubkey.clone(),
            identifier: "repo".to_string(),
        };
        let exists = super::repo_exists(&storage, &address)
            .await
            .expect("repo exists");
        assert!(!exists);

        let announcement = sample_announcement("repo");
        let record = RepoAnnouncementRecord::new(&hex_32(0x34), &pubkey, 1, &announcement)
            .expect("announcement");
        storage.insert_announcement(record).await.expect("insert");
        let exists = super::repo_exists(&storage, &address)
            .await
            .expect("repo exists after insert");
        assert!(exists);
    }

    #[derive(Debug)]
    struct FailingStorage;

    #[async_trait]
    impl AnnouncementRepository for FailingStorage {
        async fn insert_announcement(
            &self,
            _record: RepoAnnouncementRecord,
        ) -> Result<(), StorageError> {
            Err(StorageError::Internal {
                message: "fail".to_string(),
            })
        }

        async fn list_announcements(
            &self,
            _pubkey: &[u8],
            _identifier: &str,
        ) -> Result<Vec<RepoAnnouncementRecord>, StorageError> {
            Err(StorageError::Internal {
                message: "fail".to_string(),
            })
        }

        async fn latest_announcement(
            &self,
            _pubkey: &[u8],
            _identifier: &str,
        ) -> Result<Option<RepoAnnouncementRecord>, StorageError> {
            Err(StorageError::Internal {
                message: "fail".to_string(),
            })
        }
    }

    #[async_trait]
    impl StateRepository for FailingStorage {
        async fn insert_state(
            &self,
            _record: gittree_storage::RepoStateRecord,
        ) -> Result<(), StorageError> {
            Err(StorageError::Internal {
                message: "fail".to_string(),
            })
        }

        async fn latest_state(
            &self,
            _pubkey: &[u8],
            _identifier: &str,
        ) -> Result<Option<gittree_storage::RepoStateRecord>, StorageError> {
            Err(StorageError::Internal {
                message: "fail".to_string(),
            })
        }
    }

    #[async_trait]
    impl RelayCompatibilityRepository for FailingStorage {
        async fn upsert_relay_compatibility(
            &self,
            _record: RelayCompatibilityRecord,
        ) -> Result<(), StorageError> {
            Err(StorageError::Internal {
                message: "fail".to_string(),
            })
        }

        async fn relay_compatibility(
            &self,
            _relay_url: &str,
        ) -> Result<Option<RelayCompatibilityRecord>, StorageError> {
            Err(StorageError::Internal {
                message: "fail".to_string(),
            })
        }
    }

    #[tokio::test]
    async fn failing_storage_trait_methods_return_internal_errors() {
        let storage = FailingStorage;
        let pubkey = vec![0u8; 32];
        let announcement = RepoAnnouncementRecord::new(
            &hex_32(0x41),
            &hex_32(0x42),
            1,
            &sample_announcement("repo"),
        )
        .expect("announcement");
        assert!(matches!(
            storage.insert_announcement(announcement).await,
            Err(StorageError::Internal { .. })
        ));
        assert!(matches!(
            storage.list_announcements(&pubkey, "repo").await,
            Err(StorageError::Internal { .. })
        ));
        assert!(matches!(
            storage.latest_announcement(&pubkey, "repo").await,
            Err(StorageError::Internal { .. })
        ));

        let mut state_map = std::collections::HashMap::new();
        state_map.insert("HEAD".to_string(), "ref: refs/heads/main".to_string());
        state_map.insert(
            "refs/heads/main".to_string(),
            "1111111111111111111111111111111111111111".to_string(),
        );
        let state = RepoState {
            identifier: "repo".to_string(),
            state: state_map,
        };
        let state = gittree_storage::RepoStateRecord::new(&hex_32(0x43), &hex_32(0x42), 1, &state)
            .expect("state");
        assert!(matches!(
            storage.insert_state(state).await,
            Err(StorageError::Internal { .. })
        ));
        assert!(matches!(
            storage.latest_state(&pubkey, "repo").await,
            Err(StorageError::Internal { .. })
        ));

        let compat = sample_compat_record("wss://relay.example", true);
        assert!(matches!(
            storage.upsert_relay_compatibility(compat).await,
            Err(StorageError::Internal { .. })
        ));
    }

    #[tokio::test]
    async fn storage_integration_rejects_on_error() {
        let storage = FailingStorage;
        let pubkey = hex_32(0x11);
        let address = format!("30617:{pubkey}:repo");
        let request = AdmissionRequest::new(
            KIND_GIT_PATCH.0 as u64,
            "pubkey",
            "event",
            vec![vec!["a".to_string(), address]],
            Some("relay".to_string()),
            None,
        )
        .expect("request");

        let decision = evaluate_request_with_storage(&request, &storage)
            .await
            .expect("decision");
        assert!(matches!(
            decision,
            AdmissionDecision::Reject { reason } if reason.contains("storage error")
        ));
    }

    #[tokio::test]
    async fn storage_integration_warns_on_compatibility_error() {
        let storage = FailingStorage;
        let pubkey = hex_32(0x11);
        let address = format!("30617:{pubkey}:repo");
        let request = AdmissionRequest::new(
            KIND_GIT_PATCH.0 as u64,
            "pubkey",
            "event",
            vec![vec!["a".to_string(), address]],
            Some("wss://relay.example".to_string()),
            None,
        )
        .expect("request");

        let decision = evaluate_request_with_storage_mode(
            &request,
            &storage,
            RelayCompatibilityMode::Warn,
            &[],
        )
        .await
        .expect("decision");
        assert!(matches!(
            decision,
            AdmissionDecision::RequiresRelatedEvents { .. }
        ));
    }

    #[tokio::test]
    async fn storage_integration_allows_on_compatibility_error() {
        let storage = FailingStorage;
        let pubkey = hex_32(0x11);
        let address = format!("30617:{pubkey}:repo");
        let request = AdmissionRequest::new(
            KIND_GIT_PATCH.0 as u64,
            "pubkey",
            "event",
            vec![vec!["a".to_string(), address]],
            Some("wss://relay.example".to_string()),
            None,
        )
        .expect("request");

        let decision = evaluate_request_with_storage_mode(
            &request,
            &storage,
            RelayCompatibilityMode::Allow,
            &[],
        )
        .await
        .expect("decision");
        assert!(matches!(
            decision,
            AdmissionDecision::RequiresRelatedEvents { .. }
        ));
    }

    #[derive(Debug, Default)]
    struct CountingStorage {
        inner: InMemoryRepositories,
        calls: AtomicUsize,
    }

    #[async_trait]
    impl AnnouncementRepository for CountingStorage {
        async fn insert_announcement(
            &self,
            record: RepoAnnouncementRecord,
        ) -> Result<(), StorageError> {
            self.inner.insert_announcement(record).await
        }

        async fn list_announcements(
            &self,
            pubkey: &[u8],
            identifier: &str,
        ) -> Result<Vec<RepoAnnouncementRecord>, StorageError> {
            self.inner.list_announcements(pubkey, identifier).await
        }

        async fn latest_announcement(
            &self,
            pubkey: &[u8],
            identifier: &str,
        ) -> Result<Option<RepoAnnouncementRecord>, StorageError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.inner.latest_announcement(pubkey, identifier).await
        }
    }

    #[async_trait]
    impl StateRepository for CountingStorage {
        async fn insert_state(
            &self,
            record: gittree_storage::RepoStateRecord,
        ) -> Result<(), StorageError> {
            self.inner.insert_state(record).await
        }

        async fn latest_state(
            &self,
            pubkey: &[u8],
            identifier: &str,
        ) -> Result<Option<gittree_storage::RepoStateRecord>, StorageError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.inner.latest_state(pubkey, identifier).await
        }
    }

    #[async_trait]
    impl RelayCompatibilityRepository for CountingStorage {
        async fn upsert_relay_compatibility(
            &self,
            record: RelayCompatibilityRecord,
        ) -> Result<(), StorageError> {
            self.inner.upsert_relay_compatibility(record).await
        }

        async fn relay_compatibility(
            &self,
            relay_url: &str,
        ) -> Result<Option<RelayCompatibilityRecord>, StorageError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.inner.relay_compatibility(relay_url).await
        }
    }

    #[tokio::test]
    async fn counting_storage_trait_methods_cover_passthrough_paths() {
        let storage = CountingStorage::default();
        let pubkey_hex = hex_32(0x51);
        let pubkey_bytes = hex::decode(&pubkey_hex).expect("pubkey bytes");
        let announcement = RepoAnnouncementRecord::new(
            &hex_32(0x52),
            &pubkey_hex,
            1,
            &sample_announcement("repo"),
        )
        .expect("announcement");
        storage
            .insert_announcement(announcement.clone())
            .await
            .expect("insert announcement");
        let announcements = storage
            .list_announcements(&pubkey_bytes, "repo")
            .await
            .expect("list announcements");
        assert_eq!(announcements.len(), 1);

        let mut state_map = std::collections::HashMap::new();
        state_map.insert("HEAD".to_string(), "ref: refs/heads/main".to_string());
        state_map.insert(
            "refs/heads/main".to_string(),
            "1111111111111111111111111111111111111111".to_string(),
        );
        let state = RepoState {
            identifier: "repo".to_string(),
            state: state_map,
        };
        let state = gittree_storage::RepoStateRecord::new(&hex_32(0x53), &pubkey_hex, 1, &state)
            .expect("state");
        storage.insert_state(state).await.expect("insert state");
        let _ = storage
            .latest_state(&pubkey_bytes, "repo")
            .await
            .expect("latest state");
        assert!(storage.calls.load(Ordering::SeqCst) > 0);
    }

    #[test]
    fn admission_filter_and_decision_payload_conversion_cover_all_variants() {
        let mut tags = std::collections::BTreeMap::new();
        tags.insert("a".to_string(), vec!["30617:11:repo".to_string()]);
        let admission_filter = super::AdmissionFilter {
            ids: vec!["id".to_string()],
            kinds: vec![1],
            authors: vec!["author".to_string()],
            tags: tags.clone(),
            limit: Some(10),
        };
        let event_filter: EventFilter = admission_filter.clone().into();
        assert_eq!(event_filter.ids, vec!["id".to_string()]);
        let round_trip = super::AdmissionFilter::from(&event_filter);
        assert_eq!(round_trip.tags, tags);

        assert!(matches!(
            super::AdmissionDecisionPayload::from(AdmissionDecision::Accept),
            super::AdmissionDecisionPayload::Accept
        ));
        assert!(matches!(
            super::AdmissionDecisionPayload::from(AdmissionDecision::Reject {
                reason: "nope".to_string()
            }),
            super::AdmissionDecisionPayload::Reject { .. }
        ));
        assert!(matches!(
            super::AdmissionDecisionPayload::from(AdmissionDecision::RequiresRelatedEvents {
                filters: vec![event_filter]
            }),
            super::AdmissionDecisionPayload::RequiresRelatedEvents { .. }
        ));
    }

    #[test]
    fn rate_limiter_hit_limit_resets_after_window_rollover() {
        let limiter = RateLimiter::new(RateLimitConfig::new(1, 1, Duration::from_secs(60)));
        {
            let mut counters = limiter
                .pubkey_counters
                .write()
                .expect("pubkey counter lock");
            counters.insert(
                "pubkey".to_string(),
                super::RateLimitCounter {
                    count: 1,
                    window_start: std::time::Instant::now() - Duration::from_secs(120),
                },
            );
        }
        assert!(!limiter.hit_limit(&limiter.pubkey_counters, "pubkey", 1));
        assert!(limiter.hit_limit(&limiter.pubkey_counters, "pubkey", 1));
    }

    #[test]
    fn admission_cache_evict_if_needed_clears_when_disabled() {
        let cache = AdmissionCache::new(AdmissionCacheConfig {
            ttl: None,
            max_entries: 0,
        });
        let mut map = std::collections::HashMap::new();
        map.insert(
            "repo".to_string(),
            super::AdmissionCacheEntry {
                value: AdmissionDecision::Accept,
                stored_at: std::time::Instant::now(),
            },
        );
        cache.evict_if_needed(&mut map);
        assert!(map.is_empty());
    }

    #[test]
    fn restore_env_var_covers_some_and_none_paths() {
        with_env_scope(|| {
            restore_env_var("GITTREE_ADMISSION_TEST_ENV", Some("value".to_string()));
            assert_eq!(
                std::env::var("GITTREE_ADMISSION_TEST_ENV").ok().as_deref(),
                Some("value")
            );
            restore_env_var("GITTREE_ADMISSION_TEST_ENV", None);
            assert!(std::env::var("GITTREE_ADMISSION_TEST_ENV").is_err());
        });
    }

    #[tokio::test]
    async fn cache_returns_cached_decision() {
        let storage = CountingStorage::default();
        let cache = AdmissionCache::default();
        let pubkey = hex_32(0x44);
        let event_id = hex_32(0x55);
        let announcement = sample_announcement("repo");
        let record =
            RepoAnnouncementRecord::new(&event_id, &pubkey, 10, &announcement).expect("record");
        storage.insert_announcement(record).await.expect("insert");
        let compat_record = sample_compat_record("wss://relay.example", true);
        storage
            .upsert_relay_compatibility(compat_record)
            .await
            .expect("compat");

        let address = format!("30617:{pubkey}:repo");
        let request = AdmissionRequest::new(
            KIND_GIT_PATCH.0 as u64,
            "pubkey",
            "event",
            vec![vec!["a".to_string(), address]],
            Some("wss://relay.example".to_string()),
            None,
        )
        .expect("request");

        let _ = evaluate_request_cached(&request, &storage, &cache)
            .await
            .expect("first");
        let _ = evaluate_request_cached(&request, &storage, &cache)
            .await
            .expect("second");

        assert_eq!(storage.calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn cache_respects_ttl() {
        let storage = CountingStorage::default();
        let cache = AdmissionCache::new(AdmissionCacheConfig::new(Some(Duration::from_secs(0)), 8));
        let pubkey = hex_32(0x66);
        let event_id = hex_32(0x77);
        let announcement = sample_announcement("repo");
        let record =
            RepoAnnouncementRecord::new(&event_id, &pubkey, 10, &announcement).expect("record");
        storage.insert_announcement(record).await.expect("insert");
        let compat_record = sample_compat_record("wss://relay.example", true);
        storage
            .upsert_relay_compatibility(compat_record)
            .await
            .expect("compat");

        let address = format!("30617:{pubkey}:repo");
        let request = AdmissionRequest::new(
            KIND_GIT_PATCH.0 as u64,
            "pubkey",
            "event",
            vec![vec!["a".to_string(), address]],
            Some("wss://relay.example".to_string()),
            None,
        )
        .expect("request");

        let _ = evaluate_request_cached(&request, &storage, &cache)
            .await
            .expect("first");
        let _ = evaluate_request_cached(&request, &storage, &cache)
            .await
            .expect("second");

        assert!(storage.calls.load(Ordering::SeqCst) >= 4);
    }

    #[test]
    fn cache_without_ttl_returns_fresh_entry() {
        let cache = AdmissionCache::new(AdmissionCacheConfig::new(None, 2));
        let key = "decision-key".to_string();
        cache.insert(key.clone(), AdmissionDecision::Accept);
        assert_eq!(cache.get(&key), Some(AdmissionDecision::Accept));
    }

    #[test]
    fn cache_max_entries_zero_disables_storage() {
        let cache =
            AdmissionCache::new(AdmissionCacheConfig::new(Some(Duration::from_secs(30)), 0));
        let key = "decision-key".to_string();
        cache.insert(key.clone(), AdmissionDecision::Accept);
        assert!(cache.get(&key).is_none());
    }

    #[test]
    fn cache_evicts_oldest_entry_when_capacity_exceeded() {
        let cache =
            AdmissionCache::new(AdmissionCacheConfig::new(Some(Duration::from_secs(30)), 1));
        cache.insert(
            "old".to_string(),
            AdmissionDecision::Reject {
                reason: "old".to_string(),
            },
        );
        cache.insert("new".to_string(), AdmissionDecision::Accept);

        assert!(cache.get("old").is_none());
        assert_eq!(cache.get("new"), Some(AdmissionDecision::Accept));
    }

    #[test]
    fn rate_limit_defaults_are_stable() {
        let config = RateLimitConfig::default();
        assert_eq!(config.per_ip, 120);
        assert_eq!(config.per_pubkey, 60);
        assert_eq!(config.window, Duration::from_secs(60));

        let limiter = RateLimiter::default();
        let request =
            AdmissionRequest::new(1, "pubkey", "event", Vec::new(), None, None).expect("request");
        assert!(limiter.check(&request).is_none());
    }

    #[test]
    fn rate_limiter_recovers_from_poisoned_lock() {
        let limiter = Arc::new(RateLimiter::new(RateLimitConfig::new(
            2,
            0,
            Duration::from_secs(60),
        )));
        let poison = Arc::clone(&limiter);
        let _ = std::thread::spawn(move || {
            let _guard = poison.ip_counters.write().expect("lock");
            panic!("poison lock");
        })
        .join();

        let request = AdmissionRequest::new(
            1,
            "pubkey",
            "event",
            Vec::new(),
            None,
            Some("127.0.0.1".to_string()),
        )
        .expect("request");
        assert!(limiter.check(&request).is_none());
    }

    fn with_env_var<F: FnOnce()>(key: &str, value: &str, f: F) {
        with_env_scope(|| {
            let previous = std::env::var(key).ok();
            unsafe {
                std::env::set_var(key, value);
            }
            f();
            restore_env_var(key, previous);
        });
    }

    fn with_env_removed<F: FnOnce()>(key: &str, f: F) {
        with_env_scope(|| {
            let previous = std::env::var(key).ok();
            unsafe {
                std::env::remove_var(key);
            }
            f();
            restore_env_var(key, previous);
        });
    }

    fn with_env_scope<F: FnOnce()>(f: F) {
        struct EnvDepthGuard;

        impl Drop for EnvDepthGuard {
            fn drop(&mut self) {
                ENV_SCOPE_DEPTH.with(|depth| {
                    depth.set(depth.get().saturating_sub(1));
                });
            }
        }

        static ENV_LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
        thread_local! {
            static ENV_SCOPE_DEPTH: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
        }

        let outermost = ENV_SCOPE_DEPTH.with(|depth| {
            let current = depth.get();
            depth.set(current + 1);
            current == 0
        });
        let _depth_guard = EnvDepthGuard;

        if outermost {
            let lock = ENV_LOCK.get_or_init(|| std::sync::Mutex::new(()));
            let _env_guard = lock.lock().expect("env lock");
            f();
        } else {
            f();
        }
    }

    fn restore_env_var(key: &str, previous: Option<String>) {
        if let Some(value) = previous {
            unsafe {
                std::env::set_var(key, value);
            }
        } else {
            unsafe {
                std::env::remove_var(key);
            }
        }
    }
}
