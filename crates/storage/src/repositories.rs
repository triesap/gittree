use crate::{RepoAnnouncementRecord, RepoStateRecord, StorageError};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::RwLock;

#[async_trait]
pub trait AnnouncementRepository: Send + Sync {
    async fn insert_announcement(&self, record: RepoAnnouncementRecord)
    -> Result<(), StorageError>;
    async fn list_announcements(
        &self,
        pubkey: &[u8],
        identifier: &str,
    ) -> Result<Vec<RepoAnnouncementRecord>, StorageError>;
    async fn latest_announcement(
        &self,
        pubkey: &[u8],
        identifier: &str,
    ) -> Result<Option<RepoAnnouncementRecord>, StorageError>;
}

#[async_trait]
pub trait StateRepository: Send + Sync {
    async fn insert_state(&self, record: RepoStateRecord) -> Result<(), StorageError>;
    async fn latest_state(
        &self,
        pubkey: &[u8],
        identifier: &str,
    ) -> Result<Option<RepoStateRecord>, StorageError>;
}

#[derive(Debug, Default)]
pub struct InMemoryRepositories {
    announcements: RwLock<HashMap<String, Vec<RepoAnnouncementRecord>>>,
    states: RwLock<HashMap<String, Vec<RepoStateRecord>>>,
}

impl InMemoryRepositories {
    pub fn new() -> Self {
        Self::default()
    }

    fn key(pubkey: &[u8], identifier: &str) -> String {
        format!("{}:{}", hex::encode(pubkey), identifier)
    }
}

#[async_trait]
impl AnnouncementRepository for InMemoryRepositories {
    async fn insert_announcement(
        &self,
        record: RepoAnnouncementRecord,
    ) -> Result<(), StorageError> {
        let key = Self::key(&record.pubkey, &record.identifier);
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
        let key = Self::key(pubkey, identifier);
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
        let mut records = self.list_announcements(pubkey, identifier).await?;
        records.sort_by_key(|record| record.created_at);
        Ok(records.pop())
    }
}

#[async_trait]
impl StateRepository for InMemoryRepositories {
    async fn insert_state(&self, record: RepoStateRecord) -> Result<(), StorageError> {
        let key = Self::key(&record.pubkey, &record.identifier);
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
        let key = Self::key(pubkey, identifier);
        let map = self.states.read().map_err(|_| StorageError::Internal {
            message: "state store poisoned".to_string(),
        })?;
        let mut records = map.get(&key).cloned().unwrap_or_default();
        records.sort_by_key(|record| record.created_at);
        Ok(records.pop())
    }
}

#[cfg(test)]
mod tests {
    use super::{AnnouncementRepository, InMemoryRepositories, StateRepository};
    use crate::{RepoAnnouncementRecord, RepoStateRecord};
    use gittree_core::{RepoAnnouncement, RepoState};
    use std::collections::HashMap;

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
    async fn in_memory_lists_announcements() {
        let store = InMemoryRepositories::new();
        let record = RepoAnnouncementRecord::new(
            &hex_32(0x11),
            &hex_32(0x22),
            100,
            &sample_announcement("repo"),
        )
        .expect("record");

        store
            .insert_announcement(record.clone())
            .await
            .expect("insert");
        let results = store
            .list_announcements(&record.pubkey, "repo")
            .await
            .expect("list");
        assert_eq!(results, vec![record]);
    }

    #[tokio::test]
    async fn in_memory_returns_latest_announcement() {
        let store = InMemoryRepositories::new();
        let announcement = sample_announcement("repo");

        let older = RepoAnnouncementRecord::new(&hex_32(0x11), &hex_32(0x22), 10, &announcement)
            .expect("older");
        let newer = RepoAnnouncementRecord::new(&hex_32(0x11), &hex_32(0x22), 20, &announcement)
            .expect("newer");

        store.insert_announcement(older).await.expect("insert");
        store
            .insert_announcement(newer.clone())
            .await
            .expect("insert");

        let latest = store
            .latest_announcement(&newer.pubkey, "repo")
            .await
            .expect("latest");
        assert_eq!(latest, Some(newer));
    }

    #[tokio::test]
    async fn in_memory_returns_latest_state() {
        let store = InMemoryRepositories::new();
        let state = sample_state("repo");

        let older = RepoStateRecord::new(&hex_32(0x11), &hex_32(0x22), 10, &state).expect("older");
        let newer = RepoStateRecord::new(&hex_32(0x11), &hex_32(0x22), 20, &state).expect("newer");

        store.insert_state(older).await.expect("insert");
        store.insert_state(newer.clone()).await.expect("insert");

        let latest = store
            .latest_state(&newer.pubkey, "repo")
            .await
            .expect("latest");
        assert_eq!(latest, Some(newer));
    }
}
