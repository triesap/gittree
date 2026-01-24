use crate::{
    RelayCompatibilityRecord, RepoAnnouncementRecord, RepoMappingRecord, RepoStateRecord,
    StorageError,
};
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

#[async_trait]
pub trait RepoMappingRepository: Send + Sync {
    async fn upsert_mapping(&self, record: RepoMappingRecord) -> Result<(), StorageError>;
    async fn mapping_by_forgejo(
        &self,
        owner: &str,
        repo: &str,
    ) -> Result<Option<RepoMappingRecord>, StorageError>;
    async fn mapping_by_repo(
        &self,
        pubkey: &[u8],
        identifier: &str,
    ) -> Result<Option<RepoMappingRecord>, StorageError>;
}

#[async_trait]
pub trait RelayCompatibilityRepository: Send + Sync {
    async fn upsert_relay_compatibility(
        &self,
        record: RelayCompatibilityRecord,
    ) -> Result<(), StorageError>;
    async fn relay_compatibility(
        &self,
        relay_url: &str,
    ) -> Result<Option<RelayCompatibilityRecord>, StorageError>;
}

#[derive(Debug, Default)]
pub struct InMemoryRepositories {
    announcements: RwLock<HashMap<String, Vec<RepoAnnouncementRecord>>>,
    states: RwLock<HashMap<String, Vec<RepoStateRecord>>>,
    mappings_by_forgejo: RwLock<HashMap<String, RepoMappingRecord>>,
    mappings_by_repo: RwLock<HashMap<String, RepoMappingRecord>>,
    relay_compatibility: RwLock<HashMap<String, RelayCompatibilityRecord>>,
}

impl InMemoryRepositories {
    pub fn new() -> Self {
        Self::default()
    }

    fn key(pubkey: &[u8], identifier: &str) -> String {
        format!("{}:{}", hex::encode(pubkey), identifier)
    }

    fn forgejo_key(owner: &str, repo: &str) -> String {
        format!("{owner}/{repo}")
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

#[async_trait]
impl RepoMappingRepository for InMemoryRepositories {
    async fn upsert_mapping(&self, record: RepoMappingRecord) -> Result<(), StorageError> {
        let forgejo_key = Self::forgejo_key(&record.forgejo_owner, &record.forgejo_repo);
        let repo_key = Self::key(&record.pubkey, &record.identifier);
        let mut forgejo_map =
            self.mappings_by_forgejo
                .write()
                .map_err(|_| StorageError::Internal {
                    message: "forgejo mapping store poisoned".to_string(),
                })?;
        let mut repo_map =
            self.mappings_by_repo
                .write()
                .map_err(|_| StorageError::Internal {
                    message: "repo mapping store poisoned".to_string(),
                })?;
        forgejo_map.insert(forgejo_key, record.clone());
        repo_map.insert(repo_key, record);
        Ok(())
    }

    async fn mapping_by_forgejo(
        &self,
        owner: &str,
        repo: &str,
    ) -> Result<Option<RepoMappingRecord>, StorageError> {
        let key = Self::forgejo_key(owner, repo);
        let map =
            self.mappings_by_forgejo
                .read()
                .map_err(|_| StorageError::Internal {
                    message: "forgejo mapping store poisoned".to_string(),
                })?;
        Ok(map.get(&key).cloned())
    }

    async fn mapping_by_repo(
        &self,
        pubkey: &[u8],
        identifier: &str,
    ) -> Result<Option<RepoMappingRecord>, StorageError> {
        let key = Self::key(pubkey, identifier);
        let map =
            self.mappings_by_repo
                .read()
                .map_err(|_| StorageError::Internal {
                    message: "repo mapping store poisoned".to_string(),
                })?;
        Ok(map.get(&key).cloned())
    }
}

#[async_trait]
impl RelayCompatibilityRepository for InMemoryRepositories {
    async fn upsert_relay_compatibility(
        &self,
        record: RelayCompatibilityRecord,
    ) -> Result<(), StorageError> {
        let key = record.relay_url.clone();
        let mut map =
            self.relay_compatibility
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
        let map =
            self.relay_compatibility
                .read()
                .map_err(|_| StorageError::Internal {
                    message: "relay compatibility store poisoned".to_string(),
                })?;
        Ok(map.get(relay_url).cloned())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AnnouncementRepository, InMemoryRepositories, RelayCompatibilityRepository,
        RepoMappingRepository, StateRepository,
    };
    use crate::{RelayCompatibilityRecord, RepoAnnouncementRecord, RepoMappingRecord, RepoStateRecord};
    use gittree_core::{RelayCapability, RelayCompatibilityReport, RepoAnnouncement, RepoState};
    use gittree_core::RepoMapping;
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

    fn sample_mapping(identifier: &str) -> RepoMappingRecord {
        let mapping =
            RepoMapping::new("owner", "repo", hex_32(0x11), identifier).expect("mapping");
        RepoMappingRecord::new(&mapping).expect("record")
    }

    fn sample_compat_report() -> RelayCompatibilityReport {
        RelayCompatibilityReport {
            relay_url: "wss://relay.example".to_string(),
            supported: vec![RelayCapability::Nip01, RelayCapability::Nip34],
            missing_required: Vec::new(),
            missing_optional: Vec::new(),
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

    #[tokio::test]
    async fn in_memory_returns_mapping_by_forgejo() {
        let store = InMemoryRepositories::new();
        let record = sample_mapping("repo");
        store.upsert_mapping(record.clone()).await.expect("upsert");
        let found = store
            .mapping_by_forgejo("owner", "repo")
            .await
            .expect("lookup");
        assert_eq!(found, Some(record));
    }

    #[tokio::test]
    async fn in_memory_returns_mapping_by_repo() {
        let store = InMemoryRepositories::new();
        let record = sample_mapping("repo");
        store.upsert_mapping(record.clone()).await.expect("upsert");
        let found = store
            .mapping_by_repo(&record.pubkey, &record.identifier)
            .await
            .expect("lookup");
        assert_eq!(found, Some(record));
    }

    #[tokio::test]
    async fn in_memory_upserts_relay_compatibility() {
        let store = InMemoryRepositories::new();
        let report = sample_compat_report();
        let record = RelayCompatibilityRecord::new(&report, 42).expect("record");

        store
            .upsert_relay_compatibility(record.clone())
            .await
            .expect("upsert");
        let found = store
            .relay_compatibility(&report.relay_url)
            .await
            .expect("lookup");
        assert_eq!(found, Some(record));
    }
}
