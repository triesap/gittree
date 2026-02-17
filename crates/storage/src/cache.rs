use crate::{
    AnnouncementRepository, RelayCompatibilityRecord, RelayCompatibilityRepository,
    RelayPublishJob, RelayPublishRepository, RelayPublishRequest, RepoAnnouncementRecord,
    RepoStateRecord, StateRepository, StorageError,
};
use async_trait::async_trait;
use std::collections::HashMap;
use std::hash::Hash;
use std::sync::{RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::time::{Duration, Instant};
use time::OffsetDateTime;

#[derive(Debug, Clone)]
pub struct CacheConfig {
    pub ttl: Option<Duration>,
    pub max_entries: usize,
}

impl CacheConfig {
    pub fn new(ttl: Option<Duration>, max_entries: usize) -> Self {
        Self { ttl, max_entries }
    }

    pub fn disabled() -> Self {
        Self {
            ttl: None,
            max_entries: 0,
        }
    }
}

impl Default for CacheConfig {
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

#[derive(Debug, Default)]
struct CacheStore {
    announcements: RwLock<HashMap<String, CacheEntry<Vec<RepoAnnouncementRecord>>>>,
    latest_announcements: RwLock<HashMap<String, CacheEntry<RepoAnnouncementRecord>>>,
    latest_states: RwLock<HashMap<String, CacheEntry<RepoStateRecord>>>,
    relay_compatibility: RwLock<HashMap<String, CacheEntry<RelayCompatibilityRecord>>>,
}

#[derive(Debug)]
pub struct CachedRepositories<R> {
    inner: R,
    cache: CacheStore,
    config: CacheConfig,
}

impl<R> CachedRepositories<R> {
    pub fn new(inner: R) -> Self {
        Self::with_config(inner, CacheConfig::default())
    }

    pub fn with_config(inner: R, config: CacheConfig) -> Self {
        Self {
            inner,
            cache: CacheStore::default(),
            config,
        }
    }

    fn key(pubkey: &[u8], identifier: &str) -> String {
        format!("{}:{}", hex::encode(pubkey), identifier)
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
        K: Clone + Eq + Hash,
    {
        let max_entries = self.config.max_entries;
        if max_entries == 0 {
            map.clear();
            return;
        }

        while map.len() > max_entries {
            let oldest = map
                .iter()
                .min_by_key(|(_, entry)| entry.stored_at)
                .expect("cache map non-empty when eviction needed")
                .0
                .clone();
            map.remove(&oldest);
        }
    }
}

fn cache_read<'a, T>(
    lock: &'a RwLock<T>,
    message: &'static str,
) -> Result<RwLockReadGuard<'a, T>, StorageError> {
    match lock.read() {
        Ok(guard) => Ok(guard),
        Err(_) => Err(StorageError::Internal {
            message: message.to_string(),
        }),
    }
}

fn cache_write<'a, T>(
    lock: &'a RwLock<T>,
    message: &'static str,
) -> Result<RwLockWriteGuard<'a, T>, StorageError> {
    match lock.write() {
        Ok(guard) => Ok(guard),
        Err(_) => Err(StorageError::Internal {
            message: message.to_string(),
        }),
    }
}

#[async_trait]
impl<R> AnnouncementRepository for CachedRepositories<R>
where
    R: AnnouncementRepository + StateRepository,
{
    async fn insert_announcement(
        &self,
        record: RepoAnnouncementRecord,
    ) -> Result<(), StorageError> {
        if !self.cache_enabled() {
            return self.inner.insert_announcement(record).await;
        }

        self.inner.insert_announcement(record.clone()).await?;
        let key = Self::key(&record.pubkey, &record.identifier);
        let now = Instant::now();

        let mut lists = cache_write(&self.cache.announcements, "announcement cache poisoned")?;
        if let Some(entry) = lists.get_mut(&key) {
            if self.is_fresh(entry) {
                entry.value.push(record.clone());
                entry.stored_at = now;
            } else {
                lists.remove(&key);
            }
        }
        drop(lists);

        let mut latest = cache_write(
            &self.cache.latest_announcements,
            "announcement cache poisoned",
        )?;
        let should_update = match latest.get(&key) {
            Some(entry) if self.is_fresh(entry) && entry.value.created_at > record.created_at => {
                false
            }
            _ => true,
        };
        if should_update {
            latest.insert(
                key,
                CacheEntry {
                    value: record,
                    stored_at: now,
                },
            );
            self.evict_if_needed(&mut latest);
        }

        Ok(())
    }

    async fn list_announcements(
        &self,
        pubkey: &[u8],
        identifier: &str,
    ) -> Result<Vec<RepoAnnouncementRecord>, StorageError> {
        if !self.cache_enabled() {
            return self.inner.list_announcements(pubkey, identifier).await;
        }

        let key = Self::key(pubkey, identifier);
        let (cached, stale) = {
            let lists = cache_read(&self.cache.announcements, "announcement cache poisoned")?;
            match lists.get(&key) {
                Some(entry) if self.is_fresh(entry) => (Some(entry.value.clone()), false),
                Some(_) => (None, true),
                None => (None, false),
            }
        };
        if stale {
            let mut lists = cache_write(&self.cache.announcements, "announcement cache poisoned")?;
            lists.remove(&key);
        }
        if let Some(cached) = cached {
            return Ok(cached);
        }

        let records = self.inner.list_announcements(pubkey, identifier).await?;
        let now = Instant::now();

        let mut lists = cache_write(&self.cache.announcements, "announcement cache poisoned")?;
        lists.insert(
            key.clone(),
            CacheEntry {
                value: records.clone(),
                stored_at: now,
            },
        );
        self.evict_if_needed(&mut lists);
        drop(lists);

        let mut latest = cache_write(
            &self.cache.latest_announcements,
            "announcement cache poisoned",
        )?;
        if records.is_empty() {
            latest.remove(&key);
        } else if let Some(record) = records
            .iter()
            .max_by_key(|record| record.created_at)
            .cloned()
        {
            latest.insert(
                key,
                CacheEntry {
                    value: record,
                    stored_at: now,
                },
            );
            self.evict_if_needed(&mut latest);
        }

        Ok(records)
    }

    async fn latest_announcement(
        &self,
        pubkey: &[u8],
        identifier: &str,
    ) -> Result<Option<RepoAnnouncementRecord>, StorageError> {
        if !self.cache_enabled() {
            return self.inner.latest_announcement(pubkey, identifier).await;
        }

        let key = Self::key(pubkey, identifier);
        let (cached, stale) = {
            let latest = cache_read(
                &self.cache.latest_announcements,
                "announcement cache poisoned",
            )?;
            match latest.get(&key) {
                Some(entry) if self.is_fresh(entry) => (Some(entry.value.clone()), false),
                Some(_) => (None, true),
                None => (None, false),
            }
        };
        if stale {
            let mut latest = cache_write(
                &self.cache.latest_announcements,
                "announcement cache poisoned",
            )?;
            latest.remove(&key);
        }
        if let Some(cached) = cached {
            return Ok(Some(cached));
        }

        let record = self.inner.latest_announcement(pubkey, identifier).await?;
        if let Some(record) = record.clone() {
            let now = Instant::now();
            let mut latest = cache_write(
                &self.cache.latest_announcements,
                "announcement cache poisoned",
            )?;
            latest.insert(
                key,
                CacheEntry {
                    value: record,
                    stored_at: now,
                },
            );
            self.evict_if_needed(&mut latest);
        }

        Ok(record)
    }
}

#[async_trait]
impl<R> StateRepository for CachedRepositories<R>
where
    R: AnnouncementRepository + StateRepository,
{
    async fn insert_state(&self, record: RepoStateRecord) -> Result<(), StorageError> {
        if !self.cache_enabled() {
            return self.inner.insert_state(record).await;
        }

        self.inner.insert_state(record.clone()).await?;
        let key = Self::key(&record.pubkey, &record.identifier);
        let now = Instant::now();
        let mut latest = cache_write(&self.cache.latest_states, "state cache poisoned")?;
        let should_update = match latest.get(&key) {
            Some(entry) if self.is_fresh(entry) && entry.value.created_at > record.created_at => {
                false
            }
            _ => true,
        };
        if should_update {
            latest.insert(
                key,
                CacheEntry {
                    value: record,
                    stored_at: now,
                },
            );
            self.evict_if_needed(&mut latest);
        }
        Ok(())
    }

    async fn latest_state(
        &self,
        pubkey: &[u8],
        identifier: &str,
    ) -> Result<Option<RepoStateRecord>, StorageError> {
        if !self.cache_enabled() {
            return self.inner.latest_state(pubkey, identifier).await;
        }

        let key = Self::key(pubkey, identifier);
        let (cached, stale) = {
            let latest = cache_read(&self.cache.latest_states, "state cache poisoned")?;
            match latest.get(&key) {
                Some(entry) if self.is_fresh(entry) => (Some(entry.value.clone()), false),
                Some(_) => (None, true),
                None => (None, false),
            }
        };
        if stale {
            let mut latest = cache_write(&self.cache.latest_states, "state cache poisoned")?;
            latest.remove(&key);
        }
        if let Some(cached) = cached {
            return Ok(Some(cached));
        }

        let record = self.inner.latest_state(pubkey, identifier).await?;
        if let Some(record) = record.clone() {
            let now = Instant::now();
            let mut latest = cache_write(&self.cache.latest_states, "state cache poisoned")?;
            latest.insert(
                key,
                CacheEntry {
                    value: record,
                    stored_at: now,
                },
            );
            self.evict_if_needed(&mut latest);
        }

        Ok(record)
    }
}

#[async_trait]
impl<R> RelayCompatibilityRepository for CachedRepositories<R>
where
    R: RelayCompatibilityRepository,
{
    async fn upsert_relay_compatibility(
        &self,
        record: RelayCompatibilityRecord,
    ) -> Result<(), StorageError> {
        if !self.cache_enabled() {
            return self.inner.upsert_relay_compatibility(record).await;
        }

        self.inner
            .upsert_relay_compatibility(record.clone())
            .await?;
        let key = record.relay_url.clone();
        let now = Instant::now();
        let mut entries = cache_write(
            &self.cache.relay_compatibility,
            "relay compatibility cache poisoned",
        )?;
        entries.insert(
            key,
            CacheEntry {
                value: record,
                stored_at: now,
            },
        );
        self.evict_if_needed(&mut entries);
        Ok(())
    }

    async fn relay_compatibility(
        &self,
        relay_url: &str,
    ) -> Result<Option<RelayCompatibilityRecord>, StorageError> {
        if !self.cache_enabled() {
            return self.inner.relay_compatibility(relay_url).await;
        }

        let (cached, stale) = {
            let entries = cache_read(
                &self.cache.relay_compatibility,
                "relay compatibility cache poisoned",
            )?;
            match entries.get(relay_url) {
                Some(entry) if self.is_fresh(entry) => (Some(entry.value.clone()), false),
                Some(_) => (None, true),
                None => (None, false),
            }
        };
        if stale {
            let mut entries = cache_write(
                &self.cache.relay_compatibility,
                "relay compatibility cache poisoned",
            )?;
            entries.remove(relay_url);
        }
        if let Some(cached) = cached {
            return Ok(Some(cached));
        }

        let record = self.inner.relay_compatibility(relay_url).await?;
        if let Some(record) = record.clone() {
            let now = Instant::now();
            let mut entries = cache_write(
                &self.cache.relay_compatibility,
                "relay compatibility cache poisoned",
            )?;
            entries.insert(
                relay_url.to_string(),
                CacheEntry {
                    value: record,
                    stored_at: now,
                },
            );
            self.evict_if_needed(&mut entries);
        }

        Ok(record)
    }
}

#[async_trait]
impl<R> RelayPublishRepository for CachedRepositories<R>
where
    R: RelayPublishRepository,
{
    async fn enqueue_relay_publish(
        &self,
        request: RelayPublishRequest,
    ) -> Result<(), StorageError> {
        self.inner.enqueue_relay_publish(request).await
    }

    async fn claim_relay_publish(
        &self,
        now: OffsetDateTime,
    ) -> Result<Option<RelayPublishJob>, StorageError> {
        self.inner.claim_relay_publish(now).await
    }

    async fn mark_relay_publish_succeeded(&self, id: i64) -> Result<(), StorageError> {
        self.inner.mark_relay_publish_succeeded(id).await
    }

    async fn mark_relay_publish_failed(
        &self,
        id: i64,
        error: &str,
        retry_at: OffsetDateTime,
    ) -> Result<(), StorageError> {
        self.inner
            .mark_relay_publish_failed(id, error, retry_at)
            .await
    }

    async fn pending_relay_publishes(
        &self,
        pubkey: &[u8],
        identifier: &str,
        kind: u32,
    ) -> Result<i64, StorageError> {
        self.inner
            .pending_relay_publishes(pubkey, identifier, kind)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::{CacheConfig, CacheEntry, CachedRepositories};
    use crate::{
        AnnouncementRepository, RelayCompatibilityRecord, RelayCompatibilityRepository,
        RelayProbeMetadata, RelayPublishJob, RelayPublishRepository, RelayPublishRequest,
        RepoAnnouncementRecord, RepoStateRecord, StateRepository, StorageError,
    };
    use async_trait::async_trait;
    use gittree_core::{RelayCapability, RelayCompatibilityReport, RepoAnnouncement, RepoState};
    use std::collections::HashMap;
    use std::panic::AssertUnwindSafe;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, RwLock};
    use std::time::{Duration, Instant};
    use time::OffsetDateTime;
    use tokio::sync::Notify;

    #[derive(Debug, Default)]
    struct CountingRepo {
        announcements: RwLock<HashMap<String, Vec<RepoAnnouncementRecord>>>,
        states: RwLock<HashMap<String, Vec<RepoStateRecord>>>,
        relay_compatibility: RwLock<HashMap<String, RelayCompatibilityRecord>>,
        list_calls: AtomicUsize,
        latest_announcement_calls: AtomicUsize,
        latest_state_calls: AtomicUsize,
        relay_compatibility_calls: AtomicUsize,
    }

    #[derive(Debug, Default)]
    struct RelayPublishProbeRepo {
        enqueue_calls: AtomicUsize,
        claim_calls: AtomicUsize,
        mark_success_calls: AtomicUsize,
        mark_failed_calls: AtomicUsize,
        pending_calls: AtomicUsize,
    }

    #[derive(Debug, Clone, Default)]
    struct AsyncGate {
        started: Arc<Notify>,
        proceed: Arc<Notify>,
    }

    impl AsyncGate {
        async fn wait_started(&self) {
            self.started.notified().await;
        }

        fn allow(&self) {
            self.proceed.notify_one();
        }

        async fn pause(&self) {
            self.started.notify_one();
            self.proceed.notified().await;
        }
    }

    #[derive(Debug, Default)]
    struct PoisonRepo {
        list_announcements_result: Vec<RepoAnnouncementRecord>,
        latest_announcement_result: Option<RepoAnnouncementRecord>,
        latest_state_result: Option<RepoStateRecord>,
        relay_compatibility_result: Option<RelayCompatibilityRecord>,
        list_announcements_gate: Option<AsyncGate>,
        latest_announcement_gate: Option<AsyncGate>,
        latest_state_gate: Option<AsyncGate>,
        relay_compatibility_gate: Option<AsyncGate>,
    }

    impl CountingRepo {
        fn new() -> Self {
            Self::default()
        }

        fn key(pubkey: &[u8], identifier: &str) -> String {
            format!("{}:{}", hex::encode(pubkey), identifier)
        }

        fn relay_key(relay_url: &str) -> String {
            relay_url.to_string()
        }
    }

    #[async_trait]
    impl AnnouncementRepository for Arc<CountingRepo> {
        async fn insert_announcement(
            &self,
            record: RepoAnnouncementRecord,
        ) -> Result<(), StorageError> {
            let key = CountingRepo::key(&record.pubkey, &record.identifier);
            let mut map = self
                .announcements
                .write()
                .map_err(|_| StorageError::Internal {
                    message: "announcement store poisoned".to_string(),
                })?;
            map.entry(key).or_default().push(record);
            Ok(())
        }

        async fn list_announcements(
            &self,
            pubkey: &[u8],
            identifier: &str,
        ) -> Result<Vec<RepoAnnouncementRecord>, StorageError> {
            self.list_calls.fetch_add(1, Ordering::SeqCst);
            let key = CountingRepo::key(pubkey, identifier);
            let map = self
                .announcements
                .read()
                .map_err(|_| StorageError::Internal {
                    message: "announcement store poisoned".to_string(),
                })?;
            Ok(map.get(&key).cloned().unwrap_or_default())
        }

        async fn latest_announcement(
            &self,
            pubkey: &[u8],
            identifier: &str,
        ) -> Result<Option<RepoAnnouncementRecord>, StorageError> {
            self.latest_announcement_calls
                .fetch_add(1, Ordering::SeqCst);
            let mut records = self.list_announcements(pubkey, identifier).await?;
            records.sort_by_key(|record| record.created_at);
            Ok(records.pop())
        }
    }

    #[async_trait]
    impl StateRepository for Arc<CountingRepo> {
        async fn insert_state(&self, record: RepoStateRecord) -> Result<(), StorageError> {
            let key = CountingRepo::key(&record.pubkey, &record.identifier);
            let mut map = self.states.write().map_err(|_| StorageError::Internal {
                message: "state store poisoned".to_string(),
            })?;
            map.entry(key).or_default().push(record);
            Ok(())
        }

        async fn latest_state(
            &self,
            pubkey: &[u8],
            identifier: &str,
        ) -> Result<Option<RepoStateRecord>, StorageError> {
            self.latest_state_calls.fetch_add(1, Ordering::SeqCst);
            let key = CountingRepo::key(pubkey, identifier);
            let map = self.states.read().map_err(|_| StorageError::Internal {
                message: "state store poisoned".to_string(),
            })?;
            let mut records = map.get(&key).cloned().unwrap_or_default();
            records.sort_by_key(|record| record.created_at);
            Ok(records.pop())
        }
    }

    #[async_trait]
    impl RelayCompatibilityRepository for Arc<CountingRepo> {
        async fn upsert_relay_compatibility(
            &self,
            record: RelayCompatibilityRecord,
        ) -> Result<(), StorageError> {
            let key = CountingRepo::relay_key(&record.relay_url);
            let mut map = self
                .relay_compatibility
                .write()
                .map_err(|_| StorageError::Internal {
                    message: "relay compatibility store poisoned".to_string(),
                })?;
            map.insert(key, record);
            Ok(())
        }

        async fn relay_compatibility(
            &self,
            relay_url: &str,
        ) -> Result<Option<RelayCompatibilityRecord>, StorageError> {
            self.relay_compatibility_calls
                .fetch_add(1, Ordering::SeqCst);
            let map = self
                .relay_compatibility
                .read()
                .map_err(|_| StorageError::Internal {
                    message: "relay compatibility store poisoned".to_string(),
                })?;
            Ok(map.get(relay_url).cloned())
        }
    }

    #[async_trait]
    impl RelayPublishRepository for Arc<RelayPublishProbeRepo> {
        async fn enqueue_relay_publish(
            &self,
            _request: RelayPublishRequest,
        ) -> Result<(), StorageError> {
            self.enqueue_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        async fn claim_relay_publish(
            &self,
            now: OffsetDateTime,
        ) -> Result<Option<RelayPublishJob>, StorageError> {
            self.claim_calls.fetch_add(1, Ordering::SeqCst);
            Ok(Some(RelayPublishJob {
                id: 1,
                relay_url: "wss://relay.example".to_string(),
                event_id: vec![0x11; 32],
                pubkey: vec![0x22; 32],
                created_at: 1,
                kind: 1,
                tags: vec![vec!["d".to_string(), "demo".to_string()]],
                content: String::new(),
                sig: vec![0x33; 64],
                forgejo_owner: "alice".to_string(),
                forgejo_repo: "demo".to_string(),
                identifier: "demo".to_string(),
                attempt_count: 1,
                publish_after: now,
            }))
        }

        async fn mark_relay_publish_succeeded(&self, _id: i64) -> Result<(), StorageError> {
            self.mark_success_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        async fn mark_relay_publish_failed(
            &self,
            _id: i64,
            _error: &str,
            _retry_at: OffsetDateTime,
        ) -> Result<(), StorageError> {
            self.mark_failed_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        async fn pending_relay_publishes(
            &self,
            _pubkey: &[u8],
            _identifier: &str,
            _kind: u32,
        ) -> Result<i64, StorageError> {
            self.pending_calls.fetch_add(1, Ordering::SeqCst);
            Ok(7)
        }
    }

    #[async_trait]
    impl AnnouncementRepository for Arc<PoisonRepo> {
        async fn insert_announcement(
            &self,
            _record: RepoAnnouncementRecord,
        ) -> Result<(), StorageError> {
            Ok(())
        }

        async fn list_announcements(
            &self,
            _pubkey: &[u8],
            _identifier: &str,
        ) -> Result<Vec<RepoAnnouncementRecord>, StorageError> {
            if let Some(gate) = &self.list_announcements_gate {
                gate.pause().await;
            }
            Ok(self.list_announcements_result.clone())
        }

        async fn latest_announcement(
            &self,
            _pubkey: &[u8],
            _identifier: &str,
        ) -> Result<Option<RepoAnnouncementRecord>, StorageError> {
            if let Some(gate) = &self.latest_announcement_gate {
                gate.pause().await;
            }
            Ok(self.latest_announcement_result.clone())
        }
    }

    #[async_trait]
    impl StateRepository for Arc<PoisonRepo> {
        async fn insert_state(&self, _record: RepoStateRecord) -> Result<(), StorageError> {
            Ok(())
        }

        async fn latest_state(
            &self,
            _pubkey: &[u8],
            _identifier: &str,
        ) -> Result<Option<RepoStateRecord>, StorageError> {
            if let Some(gate) = &self.latest_state_gate {
                gate.pause().await;
            }
            Ok(self.latest_state_result.clone())
        }
    }

    #[async_trait]
    impl RelayCompatibilityRepository for Arc<PoisonRepo> {
        async fn upsert_relay_compatibility(
            &self,
            _record: RelayCompatibilityRecord,
        ) -> Result<(), StorageError> {
            Ok(())
        }

        async fn relay_compatibility(
            &self,
            _relay_url: &str,
        ) -> Result<Option<RelayCompatibilityRecord>, StorageError> {
            if let Some(gate) = &self.relay_compatibility_gate {
                gate.pause().await;
            }
            Ok(self.relay_compatibility_result.clone())
        }
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

    fn sample_state(identifier: &str) -> RepoState {
        let mut state = HashMap::new();
        state.insert("HEAD".to_string(), "ref: refs/heads/main".to_string());
        state.insert(
            "refs/heads/main".to_string(),
            "0123456789abcdef0123456789abcdef01234567".to_string(),
        );
        RepoState {
            identifier: identifier.to_string(),
            state,
        }
    }

    fn sample_relay_compatibility(relay_url: &str) -> RelayCompatibilityRecord {
        let report = RelayCompatibilityReport {
            relay_url: relay_url.to_string(),
            supported: vec![RelayCapability::Nip01, RelayCapability::Nip34],
            missing_required: Vec::new(),
            missing_optional: Vec::new(),
        };
        RelayCompatibilityRecord::new(&report, 10, &RelayProbeMetadata::default()).expect("record")
    }

    fn poison_lock<T>(lock: &RwLock<T>) {
        let _ = std::panic::catch_unwind(AssertUnwindSafe(|| {
            let _guard = lock.write().expect("lock");
            panic!("poison lock");
        }));
    }

    fn assert_internal_message(err: StorageError, expected: &str) {
        match err {
            StorageError::Internal { message } => assert_eq!(message, expected),
            other => panic!("expected internal error, got {other:?}"),
        }
    }

    #[test]
    fn cache_lock_helpers_map_poison_to_internal_error() {
        let lock = RwLock::new(1u8);
        let _guard = super::cache_read(&lock, "ok").expect("read lock ok");
        drop(_guard);
        let _write_guard = super::cache_write(&lock, "ok").expect("write lock ok");
        drop(_write_guard);

        poison_lock(&lock);

        let err =
            super::cache_read(&lock, "cache poisoned").expect_err("poisoned read lock must fail");
        assert_internal_message(err, "cache poisoned");

        let err =
            super::cache_write(&lock, "cache poisoned").expect_err("poisoned write lock must fail");
        assert_internal_message(err, "cache poisoned");
    }

    #[tokio::test]
    async fn poison_repo_insert_methods_are_noops() {
        let repo = Arc::new(PoisonRepo::default());

        let announcement = RepoAnnouncementRecord::new(
            &hex_32(0x11),
            &hex_32(0x22),
            10,
            &sample_announcement("repo"),
        )
        .expect("announcement");
        repo.insert_announcement(announcement)
            .await
            .expect("insert announcement");

        let state = RepoStateRecord::new(&hex_32(0x33), &hex_32(0x44), 10, &sample_state("repo"))
            .expect("state");
        repo.insert_state(state).await.expect("insert state");

        let relay_compat = sample_relay_compatibility("wss://relay.example");
        repo.upsert_relay_compatibility(relay_compat)
            .await
            .expect("upsert relay compatibility");
    }

    #[tokio::test]
    async fn list_announcements_uses_cache() {
        let inner = Arc::new(CountingRepo::new());
        let cached = CachedRepositories::new(inner.clone());
        let record = RepoAnnouncementRecord::new(
            &hex_32(0x11),
            &hex_32(0x22),
            10,
            &sample_announcement("repo"),
        )
        .expect("record");

        cached.insert_announcement(record).await.expect("insert");

        cached
            .list_announcements(&hex::decode(hex_32(0x22)).expect("pubkey"), "repo")
            .await
            .expect("first");
        cached
            .list_announcements(&hex::decode(hex_32(0x22)).expect("pubkey"), "repo")
            .await
            .expect("second");

        assert_eq!(inner.list_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn list_announcements_ttl_expires() {
        let inner = Arc::new(CountingRepo::new());
        let config = CacheConfig::new(Some(Duration::from_secs(0)), 16);
        let cached = CachedRepositories::with_config(inner.clone(), config);
        let record = RepoAnnouncementRecord::new(
            &hex_32(0x11),
            &hex_32(0x22),
            10,
            &sample_announcement("repo"),
        )
        .expect("record");

        inner
            .insert_announcement(record.clone())
            .await
            .expect("insert");

        cached
            .list_announcements(&record.pubkey, "repo")
            .await
            .expect("first");
        cached
            .list_announcements(&record.pubkey, "repo")
            .await
            .expect("second");

        assert_eq!(inner.list_calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn latest_announcement_uses_cache() {
        let inner = Arc::new(CountingRepo::new());
        let cached = CachedRepositories::new(inner.clone());
        let record = RepoAnnouncementRecord::new(
            &hex_32(0x11),
            &hex_32(0x22),
            10,
            &sample_announcement("repo"),
        )
        .expect("record");

        inner
            .insert_announcement(record.clone())
            .await
            .expect("insert");

        cached
            .latest_announcement(&record.pubkey, "repo")
            .await
            .expect("first");
        cached
            .latest_announcement(&record.pubkey, "repo")
            .await
            .expect("second");

        assert_eq!(inner.latest_announcement_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn latest_announcement_eviction_respects_max_entries() {
        let inner = Arc::new(CountingRepo::new());
        let config = CacheConfig::new(None, 1);
        let cached = CachedRepositories::with_config(inner.clone(), config);

        let record_a = RepoAnnouncementRecord::new(
            &hex_32(0x11),
            &hex_32(0x22),
            10,
            &sample_announcement("repo-a"),
        )
        .expect("record a");
        let record_b = RepoAnnouncementRecord::new(
            &hex_32(0x11),
            &hex_32(0x33),
            20,
            &sample_announcement("repo-b"),
        )
        .expect("record b");

        inner
            .insert_announcement(record_a.clone())
            .await
            .expect("insert a");
        inner
            .insert_announcement(record_b.clone())
            .await
            .expect("insert b");

        cached
            .latest_announcement(&record_a.pubkey, "repo-a")
            .await
            .expect("first a");
        cached
            .latest_announcement(&record_b.pubkey, "repo-b")
            .await
            .expect("first b");
        cached
            .latest_announcement(&record_a.pubkey, "repo-a")
            .await
            .expect("second a");

        assert_eq!(inner.latest_announcement_calls.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn insert_state_updates_latest_cache() {
        let inner = Arc::new(CountingRepo::new());
        let cached = CachedRepositories::new(inner.clone());
        let record = RepoStateRecord::new(&hex_32(0x33), &hex_32(0x44), 10, &sample_state("repo"))
            .expect("record");

        cached.insert_state(record).await.expect("insert");
        cached
            .latest_state(&hex::decode(hex_32(0x44)).expect("pubkey"), "repo")
            .await
            .expect("latest");

        assert_eq!(inner.latest_state_calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn relay_compatibility_uses_cache() {
        let inner = Arc::new(CountingRepo::new());
        let cached = CachedRepositories::new(inner.clone());
        let record = sample_relay_compatibility("wss://relay.example");

        inner
            .upsert_relay_compatibility(record)
            .await
            .expect("insert");

        cached
            .relay_compatibility("wss://relay.example")
            .await
            .expect("first");
        cached
            .relay_compatibility("wss://relay.example")
            .await
            .expect("second");

        assert_eq!(inner.relay_compatibility_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn relay_compatibility_ttl_expires() {
        let inner = Arc::new(CountingRepo::new());
        let config = CacheConfig::new(Some(Duration::from_secs(0)), 16);
        let cached = CachedRepositories::with_config(inner.clone(), config);
        let record = sample_relay_compatibility("wss://relay.example");

        inner
            .upsert_relay_compatibility(record)
            .await
            .expect("insert");

        cached
            .relay_compatibility("wss://relay.example")
            .await
            .expect("first");
        cached
            .relay_compatibility("wss://relay.example")
            .await
            .expect("second");

        assert_eq!(inner.relay_compatibility_calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn cache_disabled_bypasses_announcement_and_compatibility_cache() {
        let inner = Arc::new(CountingRepo::new());
        let cached = CachedRepositories::with_config(inner.clone(), CacheConfig::disabled());
        let record = RepoAnnouncementRecord::new(
            &hex_32(0x11),
            &hex_32(0x22),
            10,
            &sample_announcement("repo"),
        )
        .expect("record");
        inner
            .insert_announcement(record.clone())
            .await
            .expect("insert");
        inner
            .upsert_relay_compatibility(sample_relay_compatibility("wss://relay.example"))
            .await
            .expect("compat");

        cached
            .list_announcements(&record.pubkey, "repo")
            .await
            .expect("list 1");
        cached
            .list_announcements(&record.pubkey, "repo")
            .await
            .expect("list 2");
        cached
            .latest_announcement(&record.pubkey, "repo")
            .await
            .expect("latest 1");
        cached
            .latest_announcement(&record.pubkey, "repo")
            .await
            .expect("latest 2");
        cached
            .relay_compatibility("wss://relay.example")
            .await
            .expect("compat 1");
        cached
            .relay_compatibility("wss://relay.example")
            .await
            .expect("compat 2");

        // `latest_announcement` in the probe uses `list_announcements` internally.
        assert_eq!(inner.list_calls.load(Ordering::SeqCst), 4);
        assert_eq!(inner.latest_announcement_calls.load(Ordering::SeqCst), 2);
        assert_eq!(inner.relay_compatibility_calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn latest_state_ttl_expires() {
        let inner = Arc::new(CountingRepo::new());
        let config = CacheConfig::new(Some(Duration::from_secs(0)), 16);
        let cached = CachedRepositories::with_config(inner.clone(), config);
        let record = RepoStateRecord::new(&hex_32(0x33), &hex_32(0x44), 10, &sample_state("repo"))
            .expect("record");
        inner.insert_state(record.clone()).await.expect("insert");

        cached
            .latest_state(&record.pubkey, "repo")
            .await
            .expect("first");
        cached
            .latest_state(&record.pubkey, "repo")
            .await
            .expect("second");

        assert_eq!(inner.latest_state_calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn relay_publish_methods_delegate_to_inner() {
        let inner = Arc::new(RelayPublishProbeRepo::default());
        let cached = CachedRepositories::new(inner.clone());

        let request = RelayPublishRequest {
            relay_url: "wss://relay.example".to_string(),
            event_id: "11".repeat(32),
            pubkey: "22".repeat(32),
            created_at: 1,
            kind: 1,
            tags: vec![vec!["d".to_string(), "demo".to_string()]],
            content: String::new(),
            sig: "33".repeat(64),
            forgejo_owner: "alice".to_string(),
            forgejo_repo: "demo".to_string(),
            identifier: "demo".to_string(),
        };

        cached
            .enqueue_relay_publish(request)
            .await
            .expect("enqueue");
        let job = cached
            .claim_relay_publish(OffsetDateTime::UNIX_EPOCH)
            .await
            .expect("claim")
            .expect("job");
        cached
            .mark_relay_publish_succeeded(job.id)
            .await
            .expect("success");
        cached
            .mark_relay_publish_failed(
                job.id,
                "error",
                OffsetDateTime::UNIX_EPOCH + time::Duration::minutes(1),
            )
            .await
            .expect("failed");
        let pending = cached
            .pending_relay_publishes(&[0x22; 32], "demo", 1)
            .await
            .expect("pending");

        assert_eq!(pending, 7);
        assert_eq!(inner.enqueue_calls.load(Ordering::SeqCst), 1);
        assert_eq!(inner.claim_calls.load(Ordering::SeqCst), 1);
        assert_eq!(inner.mark_success_calls.load(Ordering::SeqCst), 1);
        assert_eq!(inner.mark_failed_calls.load(Ordering::SeqCst), 1);
        assert_eq!(inner.pending_calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn cache_helpers_cover_none_ttl_and_zero_max_entries() {
        let cached = CachedRepositories::with_config(
            Arc::new(CountingRepo::new()),
            CacheConfig::new(None, 0),
        );
        let mut map: HashMap<String, CacheEntry<i32>> = HashMap::new();
        map.insert(
            "k".to_string(),
            CacheEntry {
                value: 1,
                stored_at: Instant::now(),
            },
        );
        cached.evict_if_needed(&mut map);
        assert!(map.is_empty());

        let fresh = CacheEntry {
            value: 2,
            stored_at: Instant::now(),
        };
        assert!(cached.is_fresh(&fresh));

        let bounded = CachedRepositories::with_config(
            Arc::new(CountingRepo::new()),
            CacheConfig::new(Some(Duration::from_secs(1)), 1),
        );
        let now = Instant::now();
        let mut bounded_map: HashMap<String, CacheEntry<i32>> = HashMap::new();
        bounded_map.insert(
            "old".to_string(),
            CacheEntry {
                value: 1,
                stored_at: now - Duration::from_secs(2),
            },
        );
        bounded_map.insert(
            "new".to_string(),
            CacheEntry {
                value: 2,
                stored_at: now,
            },
        );
        bounded.evict_if_needed(&mut bounded_map);
        assert_eq!(bounded_map.len(), 1);
        assert!(bounded.is_fresh(&CacheEntry {
            value: 3,
            stored_at: now,
        }));
    }

    fn exercise_helper_instantiations<R>(cached: &CachedRepositories<Arc<R>>) {
        let announcement = RepoAnnouncementRecord::new(
            &hex_32(0x11),
            &hex_32(0x22),
            10,
            &sample_announcement("repo"),
        )
        .expect("announcement");
        let state = RepoStateRecord::new(&hex_32(0x33), &hex_32(0x44), 10, &sample_state("repo"))
            .expect("state");
        let relay_compat = sample_relay_compatibility("wss://relay.example");

        let now = Instant::now();

        let mut announcements: HashMap<String, CacheEntry<RepoAnnouncementRecord>> = HashMap::new();
        announcements.insert(
            "old".to_string(),
            CacheEntry {
                value: announcement.clone(),
                stored_at: now - Duration::from_secs(2),
            },
        );
        announcements.insert(
            "new".to_string(),
            CacheEntry {
                value: announcement.clone(),
                stored_at: now,
            },
        );
        cached.evict_if_needed(&mut announcements);
        assert_eq!(announcements.len(), 1);

        let mut states: HashMap<String, CacheEntry<RepoStateRecord>> = HashMap::new();
        states.insert(
            "old".to_string(),
            CacheEntry {
                value: state.clone(),
                stored_at: now - Duration::from_secs(2),
            },
        );
        states.insert(
            "new".to_string(),
            CacheEntry {
                value: state.clone(),
                stored_at: now,
            },
        );
        cached.evict_if_needed(&mut states);
        assert_eq!(states.len(), 1);

        let mut compat: HashMap<String, CacheEntry<RelayCompatibilityRecord>> = HashMap::new();
        compat.insert(
            "old".to_string(),
            CacheEntry {
                value: relay_compat.clone(),
                stored_at: now - Duration::from_secs(2),
            },
        );
        compat.insert(
            "new".to_string(),
            CacheEntry {
                value: relay_compat.clone(),
                stored_at: now,
            },
        );
        cached.evict_if_needed(&mut compat);
        assert_eq!(compat.len(), 1);

        let mut list_map: HashMap<String, CacheEntry<Vec<RepoAnnouncementRecord>>> = HashMap::new();
        list_map.insert(
            "old".to_string(),
            CacheEntry {
                value: vec![announcement.clone()],
                stored_at: now - Duration::from_secs(2),
            },
        );
        list_map.insert(
            "new".to_string(),
            CacheEntry {
                value: vec![announcement.clone()],
                stored_at: now,
            },
        );
        cached.evict_if_needed(&mut list_map);
        assert_eq!(list_map.len(), 1);

        let mut integer_map: HashMap<String, CacheEntry<i64>> = HashMap::new();
        integer_map.insert(
            "old".to_string(),
            CacheEntry {
                value: 1,
                stored_at: now - Duration::from_secs(2),
            },
        );
        integer_map.insert(
            "new".to_string(),
            CacheEntry {
                value: 2,
                stored_at: now,
            },
        );
        cached.evict_if_needed(&mut integer_map);
        assert_eq!(integer_map.len(), 1);

        assert!(cached.is_fresh(&CacheEntry {
            value: announcement.clone(),
            stored_at: now,
        }));
        assert!(cached.is_fresh(&CacheEntry {
            value: state,
            stored_at: now,
        }));
        assert!(cached.is_fresh(&CacheEntry {
            value: relay_compat,
            stored_at: now,
        }));
        assert!(cached.is_fresh(&CacheEntry {
            value: vec![announcement],
            stored_at: now,
        }));
    }

    #[test]
    fn cache_helpers_cover_generic_instantiations_for_backing_repos() {
        let counting = CachedRepositories::with_config(
            Arc::new(CountingRepo::new()),
            CacheConfig::new(Some(Duration::from_secs(1)), 1),
        );
        exercise_helper_instantiations(&counting);

        let poison = CachedRepositories::with_config(
            Arc::new(PoisonRepo::default()),
            CacheConfig::new(Some(Duration::from_secs(1)), 1),
        );
        exercise_helper_instantiations(&poison);
    }

    #[test]
    fn cache_helpers_cover_integer_evict_for_counting_repo() {
        let evict_i64: fn(
            &CachedRepositories<Arc<CountingRepo>>,
            &mut HashMap<String, CacheEntry<i64>>,
        ) = CachedRepositories::<Arc<CountingRepo>>::evict_if_needed::<String, i64>;
        let fresh_i64: fn(&CachedRepositories<Arc<CountingRepo>>, &CacheEntry<i64>) -> bool =
            CachedRepositories::<Arc<CountingRepo>>::is_fresh::<i64>;

        let cached_zero = CachedRepositories::with_config(
            Arc::new(CountingRepo::new()),
            CacheConfig::new(Some(Duration::from_secs(1)), 0),
        );
        let mut zero_map: HashMap<String, CacheEntry<i64>> = HashMap::new();
        zero_map.insert(
            "k".to_string(),
            CacheEntry {
                value: 1,
                stored_at: Instant::now(),
            },
        );
        evict_i64(&cached_zero, &mut zero_map);
        assert!(zero_map.is_empty());

        let cached = CachedRepositories::with_config(
            Arc::new(CountingRepo::new()),
            CacheConfig::new(Some(Duration::from_secs(1)), 1),
        );
        let now = Instant::now();
        let mut map: HashMap<String, CacheEntry<i64>> = HashMap::new();
        map.insert(
            "old".to_string(),
            CacheEntry {
                value: 1,
                stored_at: now - Duration::from_secs(2),
            },
        );
        map.insert(
            "new".to_string(),
            CacheEntry {
                value: 2,
                stored_at: now,
            },
        );
        evict_i64(&cached, &mut map);
        assert_eq!(map.len(), 1);
        assert!(fresh_i64(
            &cached,
            &CacheEntry {
                value: 2_i64,
                stored_at: now,
            }
        ));
    }

    #[test]
    fn cache_helpers_cover_cache_read_with_announcement_map_type() {
        let lock: RwLock<HashMap<String, CacheEntry<RepoAnnouncementRecord>>> =
            RwLock::new(HashMap::new());
        let guard = super::cache_read(&lock, "announcement cache poisoned")
            .expect("read lock for announcement cache map");
        assert!(guard.is_empty());
    }

    #[test]
    fn cache_helpers_cover_cache_read_error_with_announcement_map_type() {
        let lock: RwLock<HashMap<String, CacheEntry<RepoAnnouncementRecord>>> =
            RwLock::new(HashMap::new());
        poison_lock(&lock);
        let err = super::cache_read(&lock, "announcement cache poisoned")
            .expect_err("poisoned announcement cache read lock must fail");
        assert_internal_message(err, "announcement cache poisoned");
    }

    #[tokio::test]
    async fn cache_reports_poisoned_announcement_cache() {
        let inner = Arc::new(CountingRepo::new());
        let cached = CachedRepositories::new(inner.clone());
        let record = RepoAnnouncementRecord::new(
            &hex_32(0x11),
            &hex_32(0x22),
            10,
            &sample_announcement("repo"),
        )
        .expect("record");

        poison_lock(&cached.cache.announcements);
        let insert_err = cached
            .insert_announcement(record.clone())
            .await
            .unwrap_err();
        assert_internal_message(insert_err, "announcement cache poisoned");
        let list_err = cached
            .list_announcements(&record.pubkey, "repo")
            .await
            .unwrap_err();
        assert_internal_message(list_err, "announcement cache poisoned");

        let latest_poisoned = CachedRepositories::new(inner.clone());
        poison_lock(&latest_poisoned.cache.latest_announcements);
        let latest_insert_err = latest_poisoned
            .insert_announcement(record)
            .await
            .unwrap_err();
        assert_internal_message(latest_insert_err, "announcement cache poisoned");
    }

    #[tokio::test]
    async fn cache_reports_poisoned_state_cache() {
        let inner = Arc::new(CountingRepo::new());
        let cached = CachedRepositories::new(inner);
        let record = RepoStateRecord::new(&hex_32(0x33), &hex_32(0x44), 10, &sample_state("repo"))
            .expect("record");

        poison_lock(&cached.cache.latest_states);
        let insert_err = cached.insert_state(record.clone()).await.unwrap_err();
        assert_internal_message(insert_err, "state cache poisoned");
        let latest_err = cached
            .latest_state(&record.pubkey, "repo")
            .await
            .unwrap_err();
        assert_internal_message(latest_err, "state cache poisoned");
    }

    #[tokio::test]
    async fn cache_reports_poisoned_relay_compatibility_cache() {
        let inner = Arc::new(CountingRepo::new());
        let cached = CachedRepositories::new(inner);
        let record = sample_relay_compatibility("wss://relay.example");

        poison_lock(&cached.cache.relay_compatibility);
        let upsert_err = cached
            .upsert_relay_compatibility(record.clone())
            .await
            .unwrap_err();
        assert_internal_message(upsert_err, "relay compatibility cache poisoned");
        let read_err = cached
            .relay_compatibility(&record.relay_url)
            .await
            .unwrap_err();
        assert_internal_message(read_err, "relay compatibility cache poisoned");
    }

    #[tokio::test]
    async fn counting_repo_reports_poisoned_internal_stores() {
        let announcements = Arc::new(CountingRepo::new());
        let announcement = RepoAnnouncementRecord::new(
            &hex_32(0x11),
            &hex_32(0x22),
            10,
            &sample_announcement("repo"),
        )
        .expect("announcement");
        poison_lock(&announcements.announcements);
        let insert_announcement_err = announcements
            .insert_announcement(announcement.clone())
            .await
            .unwrap_err();
        assert_internal_message(insert_announcement_err, "announcement store poisoned");
        let list_announcement_err = announcements
            .list_announcements(&announcement.pubkey, "repo")
            .await
            .unwrap_err();
        assert_internal_message(list_announcement_err, "announcement store poisoned");

        let states = Arc::new(CountingRepo::new());
        let state = RepoStateRecord::new(&hex_32(0x33), &hex_32(0x44), 10, &sample_state("repo"))
            .expect("state");
        poison_lock(&states.states);
        let insert_state_err = states.insert_state(state.clone()).await.unwrap_err();
        assert_internal_message(insert_state_err, "state store poisoned");
        let latest_state_err = states
            .latest_state(&state.pubkey, "repo")
            .await
            .unwrap_err();
        assert_internal_message(latest_state_err, "state store poisoned");

        let relay = Arc::new(CountingRepo::new());
        let compatibility = sample_relay_compatibility("wss://relay.example");
        poison_lock(&relay.relay_compatibility);
        let upsert_compat_err = relay
            .upsert_relay_compatibility(compatibility.clone())
            .await
            .unwrap_err();
        assert_internal_message(upsert_compat_err, "relay compatibility store poisoned");
        let read_compat_err = relay
            .relay_compatibility(&compatibility.relay_url)
            .await
            .unwrap_err();
        assert_internal_message(read_compat_err, "relay compatibility store poisoned");
    }

    #[test]
    #[should_panic(expected = "expected internal error")]
    fn assert_internal_message_panics_for_non_internal_error() {
        assert_internal_message(
            StorageError::InvalidField {
                field: "field",
                value: "value".to_string(),
            },
            "unused",
        );
    }

    #[tokio::test]
    async fn list_announcements_reports_poisoned_refresh_write_lock() {
        let gate = AsyncGate::default();
        let record = RepoAnnouncementRecord::new(
            &hex_32(0x11),
            &hex_32(0x22),
            10,
            &sample_announcement("repo"),
        )
        .expect("record");
        let inner = Arc::new(PoisonRepo {
            list_announcements_result: vec![record.clone()],
            latest_announcement_result: Some(record.clone()),
            list_announcements_gate: Some(gate.clone()),
            ..PoisonRepo::default()
        });
        let cached = Arc::new(CachedRepositories::new(inner));
        let pubkey = record.pubkey.clone();
        let cached_task = {
            let cached = Arc::clone(&cached);
            tokio::spawn(async move { cached.list_announcements(&pubkey, "repo").await })
        };

        gate.wait_started().await;
        poison_lock(&cached.cache.announcements);
        gate.allow();

        let err = cached_task
            .await
            .expect("task")
            .expect_err("poisoned refresh write lock must fail");
        assert_internal_message(err, "announcement cache poisoned");
    }

    #[tokio::test]
    async fn list_announcements_reports_poisoned_latest_refresh_write_lock() {
        let gate = AsyncGate::default();
        let record = RepoAnnouncementRecord::new(
            &hex_32(0x11),
            &hex_32(0x22),
            10,
            &sample_announcement("repo"),
        )
        .expect("record");
        let inner = Arc::new(PoisonRepo {
            list_announcements_result: vec![record.clone()],
            latest_announcement_result: Some(record.clone()),
            list_announcements_gate: Some(gate.clone()),
            ..PoisonRepo::default()
        });
        let cached = Arc::new(CachedRepositories::new(inner));
        let pubkey = record.pubkey.clone();
        let cached_task = {
            let cached = Arc::clone(&cached);
            tokio::spawn(async move { cached.list_announcements(&pubkey, "repo").await })
        };

        gate.wait_started().await;
        poison_lock(&cached.cache.latest_announcements);
        gate.allow();

        let err = cached_task
            .await
            .expect("task")
            .expect_err("poisoned latest refresh write lock must fail");
        assert_internal_message(err, "announcement cache poisoned");
    }

    #[tokio::test]
    async fn latest_announcement_reports_poisoned_refresh_write_lock() {
        let gate = AsyncGate::default();
        let record = RepoAnnouncementRecord::new(
            &hex_32(0x11),
            &hex_32(0x22),
            10,
            &sample_announcement("repo"),
        )
        .expect("record");
        let inner = Arc::new(PoisonRepo {
            latest_announcement_result: Some(record.clone()),
            latest_announcement_gate: Some(gate.clone()),
            ..PoisonRepo::default()
        });
        let cached = Arc::new(CachedRepositories::new(inner));
        let pubkey = record.pubkey.clone();
        let cached_task = {
            let cached = Arc::clone(&cached);
            tokio::spawn(async move { cached.latest_announcement(&pubkey, "repo").await })
        };

        gate.wait_started().await;
        poison_lock(&cached.cache.latest_announcements);
        gate.allow();

        let err = cached_task
            .await
            .expect("task")
            .expect_err("poisoned latest announcement refresh write lock must fail");
        assert_internal_message(err, "announcement cache poisoned");
    }

    #[tokio::test]
    async fn latest_state_reports_poisoned_refresh_write_lock() {
        let gate = AsyncGate::default();
        let record = RepoStateRecord::new(&hex_32(0x33), &hex_32(0x44), 10, &sample_state("repo"))
            .expect("record");
        let inner = Arc::new(PoisonRepo {
            latest_state_result: Some(record.clone()),
            latest_state_gate: Some(gate.clone()),
            ..PoisonRepo::default()
        });
        let cached = Arc::new(CachedRepositories::new(inner));
        let pubkey = record.pubkey.clone();
        let cached_task = {
            let cached = Arc::clone(&cached);
            tokio::spawn(async move { cached.latest_state(&pubkey, "repo").await })
        };

        gate.wait_started().await;
        poison_lock(&cached.cache.latest_states);
        gate.allow();

        let err = cached_task
            .await
            .expect("task")
            .expect_err("poisoned latest state refresh write lock must fail");
        assert_internal_message(err, "state cache poisoned");
    }

    #[tokio::test]
    async fn relay_compatibility_reports_poisoned_refresh_write_lock() {
        let gate = AsyncGate::default();
        let record = sample_relay_compatibility("wss://relay.example");
        let inner = Arc::new(PoisonRepo {
            relay_compatibility_result: Some(record.clone()),
            relay_compatibility_gate: Some(gate.clone()),
            ..PoisonRepo::default()
        });
        let cached = Arc::new(CachedRepositories::new(inner));
        let cached_task = {
            let cached = Arc::clone(&cached);
            tokio::spawn(async move { cached.relay_compatibility("wss://relay.example").await })
        };

        gate.wait_started().await;
        poison_lock(&cached.cache.relay_compatibility);
        gate.allow();

        let err = cached_task
            .await
            .expect("task")
            .expect_err("poisoned relay compatibility refresh write lock must fail");
        assert_internal_message(err, "relay compatibility cache poisoned");
    }
}
