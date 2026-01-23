use crate::{
    AnnouncementRepository, RepoAnnouncementRecord, RepoStateRecord, StateRepository, StorageError,
};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::RwLock;

#[derive(Debug, Default)]
struct CacheStore {
    announcements: RwLock<HashMap<String, Vec<RepoAnnouncementRecord>>>,
    latest_announcements: RwLock<HashMap<String, RepoAnnouncementRecord>>,
    latest_states: RwLock<HashMap<String, RepoStateRecord>>,
}

#[derive(Debug)]
pub struct CachedRepositories<R> {
    inner: R,
    cache: CacheStore,
}

impl<R> CachedRepositories<R> {
    pub fn new(inner: R) -> Self {
        Self {
            inner,
            cache: CacheStore::default(),
        }
    }

    fn key(pubkey: &[u8], identifier: &str) -> String {
        format!("{}:{}", hex::encode(pubkey), identifier)
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
        self.inner.insert_announcement(record.clone()).await?;
        let key = Self::key(&record.pubkey, &record.identifier);

        let mut lists = self
            .cache
            .announcements
            .write()
            .map_err(|_| StorageError::Internal {
                message: "announcement cache poisoned".to_string(),
            })?;
        if let Some(existing) = lists.get_mut(&key) {
            existing.push(record.clone());
        }
        drop(lists);

        let mut latest =
            self.cache
                .latest_announcements
                .write()
                .map_err(|_| StorageError::Internal {
                    message: "announcement cache poisoned".to_string(),
                })?;
        match latest.get(&key) {
            Some(current) if current.created_at > record.created_at => {}
            _ => {
                latest.insert(key, record);
            }
        }

        Ok(())
    }

    async fn list_announcements(
        &self,
        pubkey: &[u8],
        identifier: &str,
    ) -> Result<Vec<RepoAnnouncementRecord>, StorageError> {
        let key = Self::key(pubkey, identifier);
        let cached = {
            let lists = self
                .cache
                .announcements
                .read()
                .map_err(|_| StorageError::Internal {
                    message: "announcement cache poisoned".to_string(),
                })?;
            lists.get(&key).cloned()
        };
        if let Some(cached) = cached {
            return Ok(cached);
        }

        let records = self.inner.list_announcements(pubkey, identifier).await?;

        let mut lists = self
            .cache
            .announcements
            .write()
            .map_err(|_| StorageError::Internal {
                message: "announcement cache poisoned".to_string(),
            })?;
        lists.insert(key.clone(), records.clone());
        drop(lists);

        let mut latest =
            self.cache
                .latest_announcements
                .write()
                .map_err(|_| StorageError::Internal {
                    message: "announcement cache poisoned".to_string(),
                })?;
        if records.is_empty() {
            latest.remove(&key);
        } else if let Some(record) = records
            .iter()
            .max_by_key(|record| record.created_at)
            .cloned()
        {
            latest.insert(key, record);
        }

        Ok(records)
    }

    async fn latest_announcement(
        &self,
        pubkey: &[u8],
        identifier: &str,
    ) -> Result<Option<RepoAnnouncementRecord>, StorageError> {
        let key = Self::key(pubkey, identifier);
        let cached = {
            let latest =
                self.cache
                    .latest_announcements
                    .read()
                    .map_err(|_| StorageError::Internal {
                        message: "announcement cache poisoned".to_string(),
                    })?;
            latest.get(&key).cloned()
        };
        if let Some(cached) = cached {
            return Ok(Some(cached));
        }

        let record = self.inner.latest_announcement(pubkey, identifier).await?;
        if let Some(record) = record.clone() {
            let mut latest =
                self.cache
                    .latest_announcements
                    .write()
                    .map_err(|_| StorageError::Internal {
                        message: "announcement cache poisoned".to_string(),
                    })?;
            latest.insert(key, record);
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
        self.inner.insert_state(record.clone()).await?;
        let key = Self::key(&record.pubkey, &record.identifier);
        let mut latest = self
            .cache
            .latest_states
            .write()
            .map_err(|_| StorageError::Internal {
                message: "state cache poisoned".to_string(),
            })?;
        match latest.get(&key) {
            Some(current) if current.created_at > record.created_at => {}
            _ => {
                latest.insert(key, record);
            }
        }
        Ok(())
    }

    async fn latest_state(
        &self,
        pubkey: &[u8],
        identifier: &str,
    ) -> Result<Option<RepoStateRecord>, StorageError> {
        let key = Self::key(pubkey, identifier);
        let cached = {
            let latest = self
                .cache
                .latest_states
                .read()
                .map_err(|_| StorageError::Internal {
                    message: "state cache poisoned".to_string(),
                })?;
            latest.get(&key).cloned()
        };
        if let Some(cached) = cached {
            return Ok(Some(cached));
        }

        let record = self.inner.latest_state(pubkey, identifier).await?;
        if let Some(record) = record.clone() {
            let mut latest =
                self.cache
                    .latest_states
                    .write()
                    .map_err(|_| StorageError::Internal {
                        message: "state cache poisoned".to_string(),
                    })?;
            latest.insert(key, record);
        }

        Ok(record)
    }
}

#[cfg(test)]
mod tests {
    use super::CachedRepositories;
    use crate::{
        AnnouncementRepository, RepoAnnouncementRecord, RepoStateRecord, StateRepository,
        StorageError,
    };
    use async_trait::async_trait;
    use gittree_core::{RepoAnnouncement, RepoState};
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, RwLock};

    #[derive(Debug, Default)]
    struct CountingRepo {
        announcements: RwLock<HashMap<String, Vec<RepoAnnouncementRecord>>>,
        states: RwLock<HashMap<String, Vec<RepoStateRecord>>>,
        list_calls: AtomicUsize,
        latest_announcement_calls: AtomicUsize,
        latest_state_calls: AtomicUsize,
    }

    impl CountingRepo {
        fn new() -> Self {
            Self::default()
        }

        fn key(pubkey: &[u8], identifier: &str) -> String {
            format!("{}:{}", hex::encode(pubkey), identifier)
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
}
