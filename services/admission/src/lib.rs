use gittree_config::{ConfigError, ServicesConfig};
use gittree_core::nip34_common::RepoAddress;
use gittree_core::{CoreError, EventFilter};
use gittree_observability::ObservabilityError;
use gittree_storage::{AnnouncementRepository, StateRepository, StorageError};
use std::collections::HashMap;
use std::hash::Hash;
use std::sync::RwLock;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdmissionConfig {
    pub bind: String,
}

impl AdmissionConfig {
    pub fn from_env() -> Result<Self, ConfigError> {
        let services = ServicesConfig::from_env_validated()?;
        Ok(Self {
            bind: services.admission.bind,
        })
    }
}

#[derive(Debug)]
pub enum AdmissionError {
    Config(ConfigError),
    Request(AdmissionRequestError),
    Core(CoreError),
    Observability(ObservabilityError),
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
    let kind = request.kind_u32().map_err(AdmissionError::Request)?;
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
    S: AnnouncementRepository + StateRepository,
{
    let decision = evaluate_request(request)?;
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
    S: AnnouncementRepository + StateRepository,
{
    let key = cache.cache_key(request);
    if let Some(cached) = cache.get(&key) {
        return Ok(cached);
    }

    let decision = evaluate_request_with_storage(request, storage).await?;
    cache.insert(key, decision.clone());
    Ok(decision)
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
            AdmissionError::Observability(err) => {
                write!(f, "admission observability error: {err}")
            }
        }
    }
}

impl std::error::Error for AdmissionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            AdmissionError::Config(err) => Some(err),
            AdmissionError::Request(err) => Some(err),
            AdmissionError::Core(err) => Some(err),
            AdmissionError::Observability(err) => Some(err),
        }
    }
}

pub fn init_observability() -> Result<(), AdmissionError> {
    let config = gittree_observability::ObservabilityConfig {
        service_name: "gittree-admission".to_string(),
        ..gittree_observability::ObservabilityConfig::default()
    };
    gittree_observability::init(&config).map_err(AdmissionError::Observability)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::AdmissionCache;
    use super::AdmissionCacheConfig;
    use super::AdmissionConfig;
    use super::AdmissionDecision;
    use super::AdmissionRequest;
    use super::RateLimitConfig;
    use super::RateLimiter;
    use super::evaluate_request;
    use super::evaluate_request_cached;
    use super::evaluate_request_with_storage;
    use async_trait::async_trait;
    use gittree_config::ServicesConfig;
    use gittree_core::EventFilter;
    use gittree_core::RepoAnnouncement;
    use gittree_core::kinds::{KIND_GIT_PATCH, KIND_GIT_REPO_STATE};
    use gittree_storage::{AnnouncementRepository, InMemoryRepositories, RepoAnnouncementRecord};
    use gittree_storage::{StateRepository, StorageError};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    #[test]
    fn config_loads_from_env() {
        let config = AdmissionConfig::from_env().expect("config");
        let services = ServicesConfig::from_env_validated().expect("services");
        assert_eq!(config.bind, services.admission.bind);
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
    fn decision_reject_requires_reason() {
        let err = AdmissionDecision::reject(" ").unwrap_err();
        assert!(matches!(err, super::AdmissionDecisionError::MissingReason));
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
    async fn storage_integration_keeps_related_when_repo_exists() {
        let storage = InMemoryRepositories::new();
        let pubkey = hex_32(0x11);
        let event_id = hex_32(0x22);
        let announcement = sample_announcement("repo");
        let record =
            RepoAnnouncementRecord::new(&event_id, &pubkey, 10, &announcement).expect("record");
        storage.insert_announcement(record).await.expect("insert");

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
            AdmissionDecision::RequiresRelatedEvents { .. }
        ));
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

        let _ = evaluate_request_cached(&request, &storage, &cache)
            .await
            .expect("first");
        let _ = evaluate_request_cached(&request, &storage, &cache)
            .await
            .expect("second");

        assert_eq!(storage.calls.load(Ordering::SeqCst), 1);
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

        let _ = evaluate_request_cached(&request, &storage, &cache)
            .await
            .expect("first");
        let _ = evaluate_request_cached(&request, &storage, &cache)
            .await
            .expect("second");

        assert!(storage.calls.load(Ordering::SeqCst) >= 2);
    }
}
