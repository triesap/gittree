use crate::{
    AccountRecord, EventQuery, EventRecord, ProfileRecord, RelayCompatibilityRecord,
    RelayInviteRecord, RelayMembershipRecord, RelayPublishJob, RelayPublishRequest,
    RelayPublishStatus, RelayTenantRecord, RepoAnnouncementRecord, RepoMappingRecord,
    RepoStateRecord, StorageError,
};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::RwLock;
use std::sync::atomic::{AtomicI64, Ordering};
use time::OffsetDateTime;

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
    async fn list_mappings(&self) -> Result<Vec<RepoMappingRecord>, StorageError>;
}

#[async_trait]
pub trait AccountRepository: Send + Sync {
    async fn upsert_account(&self, record: AccountRecord) -> Result<(), StorageError>;
    async fn account_by_pubkey(&self, pubkey: &[u8])
    -> Result<Option<AccountRecord>, StorageError>;
    async fn account_by_username(
        &self,
        username: &str,
    ) -> Result<Option<AccountRecord>, StorageError>;
}

#[async_trait]
pub trait ProfileRepository: Send + Sync {
    async fn upsert_profile(&self, record: ProfileRecord) -> Result<(), StorageError>;
    async fn profile_by_pubkey(&self, pubkey: &[u8])
    -> Result<Option<ProfileRecord>, StorageError>;
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

#[async_trait]
pub trait RelayTenantRepository: Send + Sync {
    async fn upsert_tenant(&self, record: RelayTenantRecord) -> Result<(), StorageError>;
    async fn tenant_by_id(
        &self,
        tenant_id: &str,
    ) -> Result<Option<RelayTenantRecord>, StorageError>;
    async fn tenant_by_host(&self, host: &str) -> Result<Option<RelayTenantRecord>, StorageError>;
    async fn list_tenants(&self) -> Result<Vec<RelayTenantRecord>, StorageError>;
}

#[async_trait]
pub trait RelayMembershipRepository: Send + Sync {
    async fn upsert_membership(&self, record: RelayMembershipRecord) -> Result<(), StorageError>;
    async fn membership_by_pubkey(
        &self,
        tenant_id: &str,
        pubkey: &[u8],
    ) -> Result<Option<RelayMembershipRecord>, StorageError>;
    async fn list_memberships(
        &self,
        tenant_id: &str,
    ) -> Result<Vec<RelayMembershipRecord>, StorageError>;
    async fn remove_membership(&self, tenant_id: &str, pubkey: &[u8])
    -> Result<bool, StorageError>;
    async fn insert_invite(&self, record: RelayInviteRecord) -> Result<(), StorageError>;
    async fn invite_by_code(
        &self,
        tenant_id: &str,
        invite_code: &str,
    ) -> Result<Option<RelayInviteRecord>, StorageError>;
    async fn delete_invite(&self, tenant_id: &str, invite_code: &str) -> Result<(), StorageError>;
}

#[async_trait]
pub trait EventRepository: Send + Sync {
    async fn insert_event(&self, record: EventRecord) -> Result<(), StorageError>;
    async fn get_event(
        &self,
        tenant_id: &str,
        event_id: &[u8],
    ) -> Result<Option<EventRecord>, StorageError>;
    async fn delete_event(&self, tenant_id: &str, event_id: &[u8]) -> Result<bool, StorageError>;
    async fn query_events(&self, query: &EventQuery) -> Result<Vec<EventRecord>, StorageError>;
}

#[async_trait]
pub trait RelayPublishRepository: Send + Sync {
    async fn enqueue_relay_publish(&self, request: RelayPublishRequest)
    -> Result<(), StorageError>;
    async fn claim_relay_publish(
        &self,
        now: OffsetDateTime,
    ) -> Result<Option<RelayPublishJob>, StorageError>;
    async fn mark_relay_publish_succeeded(&self, id: i64) -> Result<(), StorageError>;
    async fn mark_relay_publish_failed(
        &self,
        id: i64,
        error: &str,
        retry_at: OffsetDateTime,
    ) -> Result<(), StorageError>;
    async fn pending_relay_publishes(
        &self,
        pubkey: &[u8],
        identifier: &str,
        kind: u32,
    ) -> Result<i64, StorageError>;
}

#[derive(Debug, Clone)]
struct OutboxEntry {
    job: RelayPublishJob,
    status: RelayPublishStatus,
    last_error: Option<String>,
}

#[derive(Debug, Default)]
pub struct InMemoryRepositories {
    announcements: RwLock<HashMap<String, Vec<RepoAnnouncementRecord>>>,
    states: RwLock<HashMap<String, Vec<RepoStateRecord>>>,
    mappings_by_forgejo: RwLock<HashMap<String, RepoMappingRecord>>,
    mappings_by_repo: RwLock<HashMap<String, RepoMappingRecord>>,
    accounts_by_pubkey: RwLock<HashMap<String, AccountRecord>>,
    accounts_by_username: RwLock<HashMap<String, AccountRecord>>,
    profiles_by_pubkey: RwLock<HashMap<String, ProfileRecord>>,
    relay_compatibility: RwLock<HashMap<String, RelayCompatibilityRecord>>,
    tenants_by_id: RwLock<HashMap<String, RelayTenantRecord>>,
    tenants_by_host: RwLock<HashMap<String, String>>,
    memberships: RwLock<HashMap<String, RelayMembershipRecord>>,
    invites: RwLock<HashMap<String, RelayInviteRecord>>,
    events: RwLock<HashMap<String, EventRecord>>,
    outbox: RwLock<HashMap<i64, OutboxEntry>>,
    outbox_seq: AtomicI64,
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

    fn event_key(event_id: &[u8]) -> String {
        hex::encode(event_id)
    }

    fn account_key(pubkey: &[u8]) -> String {
        hex::encode(pubkey)
    }

    fn profile_key(pubkey: &[u8]) -> String {
        Self::account_key(pubkey)
    }

    fn tenant_key(tenant_id: &str) -> String {
        tenant_id.to_string()
    }

    fn membership_key(tenant_id: &str, pubkey: &[u8]) -> String {
        format!("{tenant_id}:{}", hex::encode(pubkey))
    }

    fn invite_key(invite_code: &str) -> String {
        invite_code.to_string()
    }

    fn next_outbox_id(&self) -> i64 {
        self.outbox_seq.fetch_add(1, Ordering::SeqCst) + 1
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
        let mut repo_map = self
            .mappings_by_repo
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
        let map = self
            .mappings_by_forgejo
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
        let map = self
            .mappings_by_repo
            .read()
            .map_err(|_| StorageError::Internal {
                message: "repo mapping store poisoned".to_string(),
            })?;
        Ok(map.get(&key).cloned())
    }

    async fn list_mappings(&self) -> Result<Vec<RepoMappingRecord>, StorageError> {
        let map = self
            .mappings_by_repo
            .read()
            .map_err(|_| StorageError::Internal {
                message: "repo mapping store poisoned".to_string(),
            })?;
        let mut records: Vec<RepoMappingRecord> = map.values().cloned().collect();
        records.sort_by(|a, b| a.forgejo_full_name().cmp(&b.forgejo_full_name()));
        Ok(records)
    }
}

#[async_trait]
impl AccountRepository for InMemoryRepositories {
    async fn upsert_account(&self, record: AccountRecord) -> Result<(), StorageError> {
        let key = Self::account_key(&record.pubkey);
        let mut by_pubkey =
            self.accounts_by_pubkey
                .write()
                .map_err(|_| StorageError::Internal {
                    message: "account pubkey store poisoned".to_string(),
                })?;
        let mut by_username =
            self.accounts_by_username
                .write()
                .map_err(|_| StorageError::Internal {
                    message: "account username store poisoned".to_string(),
                })?;

        if let Some(existing) = by_username.get(&record.forgejo_username) {
            if existing.pubkey != record.pubkey {
                return Err(StorageError::Internal {
                    message: "account username already exists".to_string(),
                });
            }
        }

        if let Some(existing) = by_pubkey.get(&key) {
            if existing.forgejo_username != record.forgejo_username {
                by_username.remove(&existing.forgejo_username);
            }
        }

        by_pubkey.insert(key, record.clone());
        by_username.insert(record.forgejo_username.clone(), record);
        Ok(())
    }

    async fn account_by_pubkey(
        &self,
        pubkey: &[u8],
    ) -> Result<Option<AccountRecord>, StorageError> {
        let key = Self::account_key(pubkey);
        let map = self
            .accounts_by_pubkey
            .read()
            .map_err(|_| StorageError::Internal {
                message: "account pubkey store poisoned".to_string(),
            })?;
        Ok(map.get(&key).cloned())
    }

    async fn account_by_username(
        &self,
        username: &str,
    ) -> Result<Option<AccountRecord>, StorageError> {
        let map = self
            .accounts_by_username
            .read()
            .map_err(|_| StorageError::Internal {
                message: "account username store poisoned".to_string(),
            })?;
        Ok(map.get(username).cloned())
    }
}

#[async_trait]
impl ProfileRepository for InMemoryRepositories {
    async fn upsert_profile(&self, record: ProfileRecord) -> Result<(), StorageError> {
        let key = Self::profile_key(&record.pubkey);
        let mut profiles = self
            .profiles_by_pubkey
            .write()
            .map_err(|_| StorageError::Internal {
                message: "profile store poisoned".to_string(),
            })?;
        profiles.insert(key, record);
        Ok(())
    }

    async fn profile_by_pubkey(
        &self,
        pubkey: &[u8],
    ) -> Result<Option<ProfileRecord>, StorageError> {
        let key = Self::profile_key(pubkey);
        let profiles = self
            .profiles_by_pubkey
            .read()
            .map_err(|_| StorageError::Internal {
                message: "profile store poisoned".to_string(),
            })?;
        Ok(profiles.get(&key).cloned())
    }
}

#[async_trait]
impl RelayCompatibilityRepository for InMemoryRepositories {
    async fn upsert_relay_compatibility(
        &self,
        record: RelayCompatibilityRecord,
    ) -> Result<(), StorageError> {
        let key = record.relay_url.clone();
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
impl RelayTenantRepository for InMemoryRepositories {
    async fn upsert_tenant(&self, record: RelayTenantRecord) -> Result<(), StorageError> {
        let mut by_id = self
            .tenants_by_id
            .write()
            .map_err(|_| StorageError::Internal {
                message: "tenant store poisoned".to_string(),
            })?;
        let mut by_host = self
            .tenants_by_host
            .write()
            .map_err(|_| StorageError::Internal {
                message: "tenant store poisoned".to_string(),
            })?;
        let key = Self::tenant_key(&record.id);
        by_host.insert(record.host.clone(), record.id.clone());
        by_id.insert(key, record);
        Ok(())
    }

    async fn tenant_by_id(
        &self,
        tenant_id: &str,
    ) -> Result<Option<RelayTenantRecord>, StorageError> {
        let by_id = self
            .tenants_by_id
            .read()
            .map_err(|_| StorageError::Internal {
                message: "tenant store poisoned".to_string(),
            })?;
        Ok(by_id.get(tenant_id).cloned())
    }

    async fn tenant_by_host(&self, host: &str) -> Result<Option<RelayTenantRecord>, StorageError> {
        let by_host = self
            .tenants_by_host
            .read()
            .map_err(|_| StorageError::Internal {
                message: "tenant store poisoned".to_string(),
            })?;
        let Some(tenant_id) = by_host.get(host) else {
            return Ok(None);
        };
        let by_id = self
            .tenants_by_id
            .read()
            .map_err(|_| StorageError::Internal {
                message: "tenant store poisoned".to_string(),
            })?;
        Ok(by_id.get(tenant_id).cloned())
    }

    async fn list_tenants(&self) -> Result<Vec<RelayTenantRecord>, StorageError> {
        let by_id = self
            .tenants_by_id
            .read()
            .map_err(|_| StorageError::Internal {
                message: "tenant store poisoned".to_string(),
            })?;
        Ok(by_id.values().cloned().collect())
    }
}

#[async_trait]
impl RelayMembershipRepository for InMemoryRepositories {
    async fn upsert_membership(&self, record: RelayMembershipRecord) -> Result<(), StorageError> {
        let key = Self::membership_key(&record.tenant_id, &record.pubkey);
        let mut map = self
            .memberships
            .write()
            .map_err(|_| StorageError::Internal {
                message: "membership store poisoned".to_string(),
            })?;
        map.insert(key, record);
        Ok(())
    }

    async fn membership_by_pubkey(
        &self,
        tenant_id: &str,
        pubkey: &[u8],
    ) -> Result<Option<RelayMembershipRecord>, StorageError> {
        let key = Self::membership_key(tenant_id, pubkey);
        let map = self
            .memberships
            .read()
            .map_err(|_| StorageError::Internal {
                message: "membership store poisoned".to_string(),
            })?;
        Ok(map.get(&key).cloned())
    }

    async fn list_memberships(
        &self,
        tenant_id: &str,
    ) -> Result<Vec<RelayMembershipRecord>, StorageError> {
        let map = self
            .memberships
            .read()
            .map_err(|_| StorageError::Internal {
                message: "membership store poisoned".to_string(),
            })?;
        Ok(map
            .values()
            .filter(|record| record.tenant_id == tenant_id)
            .cloned()
            .collect())
    }

    async fn remove_membership(
        &self,
        tenant_id: &str,
        pubkey: &[u8],
    ) -> Result<bool, StorageError> {
        let key = Self::membership_key(tenant_id, pubkey);
        let mut map = self
            .memberships
            .write()
            .map_err(|_| StorageError::Internal {
                message: "membership store poisoned".to_string(),
            })?;
        Ok(map.remove(&key).is_some())
    }

    async fn insert_invite(&self, record: RelayInviteRecord) -> Result<(), StorageError> {
        let key = Self::invite_key(&record.invite_code);
        let mut map = self.invites.write().map_err(|_| StorageError::Internal {
            message: "invite store poisoned".to_string(),
        })?;
        map.insert(key, record);
        Ok(())
    }

    async fn invite_by_code(
        &self,
        tenant_id: &str,
        invite_code: &str,
    ) -> Result<Option<RelayInviteRecord>, StorageError> {
        let map = self.invites.read().map_err(|_| StorageError::Internal {
            message: "invite store poisoned".to_string(),
        })?;
        Ok(map
            .get(invite_code)
            .filter(|record| record.tenant_id == tenant_id)
            .cloned())
    }

    async fn delete_invite(&self, tenant_id: &str, invite_code: &str) -> Result<(), StorageError> {
        let mut map = self.invites.write().map_err(|_| StorageError::Internal {
            message: "invite store poisoned".to_string(),
        })?;
        if map
            .get(invite_code)
            .is_some_and(|record| record.tenant_id == tenant_id)
        {
            map.remove(invite_code);
        }
        Ok(())
    }
}

#[async_trait]
impl EventRepository for InMemoryRepositories {
    async fn insert_event(&self, record: EventRecord) -> Result<(), StorageError> {
        let key = Self::event_key(&record.id);
        let mut map = self.events.write().map_err(|_| StorageError::Internal {
            message: "event store poisoned".to_string(),
        })?;
        map.entry(key).or_insert(record);
        Ok(())
    }

    async fn get_event(
        &self,
        tenant_id: &str,
        event_id: &[u8],
    ) -> Result<Option<EventRecord>, StorageError> {
        let key = Self::event_key(event_id);
        let map = self.events.read().map_err(|_| StorageError::Internal {
            message: "event store poisoned".to_string(),
        })?;
        Ok(map
            .get(&key)
            .filter(|record| record.tenant_id == tenant_id)
            .cloned())
    }

    async fn delete_event(&self, tenant_id: &str, event_id: &[u8]) -> Result<bool, StorageError> {
        let key = Self::event_key(event_id);
        let mut map = self.events.write().map_err(|_| StorageError::Internal {
            message: "event store poisoned".to_string(),
        })?;
        Ok(match map.get(&key) {
            Some(record) if record.tenant_id == tenant_id => map.remove(&key).is_some(),
            _ => false,
        })
    }

    async fn query_events(&self, query: &EventQuery) -> Result<Vec<EventRecord>, StorageError> {
        let map = self.events.read().map_err(|_| StorageError::Internal {
            message: "event store poisoned".to_string(),
        })?;

        let mut tag_filters: HashMap<&str, Vec<&str>> = HashMap::new();
        for tag in &query.tags {
            tag_filters
                .entry(tag.name.as_str())
                .or_default()
                .push(tag.value.as_str());
        }

        let mut results: Vec<EventRecord> = map
            .values()
            .filter(|event| {
                if let Some(tenant_id) = &query.tenant_id {
                    if event.tenant_id != *tenant_id {
                        return false;
                    }
                }

                if !query.ids.is_empty()
                    && !query.ids.iter().any(|id| match hex::decode(id) {
                        Ok(bytes) => bytes == event.id,
                        Err(_) => false,
                    })
                {
                    return false;
                }

                if !query.authors.is_empty()
                    && !query
                        .authors
                        .iter()
                        .any(|author| match hex::decode(author) {
                            Ok(bytes) => bytes == event.pubkey,
                            Err(_) => false,
                        })
                {
                    return false;
                }

                if !query.kinds.is_empty() && !query.kinds.contains(&event.kind) {
                    return false;
                }

                if let Some(since) = query.since {
                    if event.created_at < since {
                        return false;
                    }
                }

                if let Some(until) = query.until {
                    if event.created_at > until {
                        return false;
                    }
                }

                if !tag_filters.is_empty() {
                    let mut by_name: HashMap<&str, Vec<&str>> = HashMap::new();
                    for tag in &event.tags {
                        by_name
                            .entry(tag.name.as_str())
                            .or_default()
                            .push(tag.value.as_str());
                    }
                    for (name, wanted) in &tag_filters {
                        let Some(values) = by_name.get(name) else {
                            return false;
                        };
                        if !values.iter().any(|value| wanted.contains(value)) {
                            return false;
                        }
                    }
                }

                true
            })
            .cloned()
            .collect();

        results.sort_by(|left, right| {
            right
                .created_at
                .cmp(&left.created_at)
                .then_with(|| left.id.cmp(&right.id))
        });

        if let Some(limit) = query.limit {
            results.truncate(limit as usize);
        }

        Ok(results)
    }
}

#[async_trait]
impl RelayPublishRepository for InMemoryRepositories {
    async fn enqueue_relay_publish(
        &self,
        request: RelayPublishRequest,
    ) -> Result<(), StorageError> {
        let entry = request.decode()?;
        let id = self.next_outbox_id();
        let job = RelayPublishJob {
            id,
            relay_url: entry.relay_url,
            event_id: entry.event_id,
            pubkey: entry.pubkey,
            created_at: entry.created_at,
            kind: entry.kind,
            tags: entry.tags,
            content: entry.content,
            sig: entry.sig,
            forgejo_owner: entry.forgejo_owner,
            forgejo_repo: entry.forgejo_repo,
            identifier: entry.identifier,
            attempt_count: 0,
            publish_after: OffsetDateTime::now_utc(),
        };
        let mut outbox = self.outbox.write().map_err(|_| StorageError::Internal {
            message: "outbox store poisoned".to_string(),
        })?;
        outbox.insert(
            id,
            OutboxEntry {
                job,
                status: RelayPublishStatus::Pending,
                last_error: None,
            },
        );
        Ok(())
    }

    async fn claim_relay_publish(
        &self,
        now: OffsetDateTime,
    ) -> Result<Option<RelayPublishJob>, StorageError> {
        let mut outbox = self.outbox.write().map_err(|_| StorageError::Internal {
            message: "outbox store poisoned".to_string(),
        })?;
        let mut selected: Option<i64> = None;
        for (id, entry) in outbox.iter() {
            if entry.status != RelayPublishStatus::Pending {
                continue;
            }
            if entry.job.publish_after > now {
                continue;
            }
            selected = match selected {
                Some(current) if current <= *id => Some(current),
                _ => Some(*id),
            };
        }
        let Some(id) = selected else {
            return Ok(None);
        };
        let entry = outbox.get_mut(&id).expect("entry");
        entry.status = RelayPublishStatus::Publishing;
        entry.job.attempt_count += 1;
        entry.job.publish_after = now;
        Ok(Some(entry.job.clone()))
    }

    async fn mark_relay_publish_succeeded(&self, id: i64) -> Result<(), StorageError> {
        let mut outbox = self.outbox.write().map_err(|_| StorageError::Internal {
            message: "outbox store poisoned".to_string(),
        })?;
        let entry = outbox.get_mut(&id).ok_or_else(|| StorageError::Internal {
            message: "outbox entry not found".to_string(),
        })?;
        entry.status = RelayPublishStatus::Published;
        entry.last_error = None;
        Ok(())
    }

    async fn mark_relay_publish_failed(
        &self,
        id: i64,
        error: &str,
        retry_at: OffsetDateTime,
    ) -> Result<(), StorageError> {
        let mut outbox = self.outbox.write().map_err(|_| StorageError::Internal {
            message: "outbox store poisoned".to_string(),
        })?;
        let entry = outbox.get_mut(&id).ok_or_else(|| StorageError::Internal {
            message: "outbox entry not found".to_string(),
        })?;
        entry.status = RelayPublishStatus::Pending;
        entry.job.publish_after = retry_at;
        entry.last_error = Some(error.to_string());
        Ok(())
    }

    async fn pending_relay_publishes(
        &self,
        pubkey: &[u8],
        identifier: &str,
        kind: u32,
    ) -> Result<i64, StorageError> {
        let outbox = self.outbox.read().map_err(|_| StorageError::Internal {
            message: "outbox store poisoned".to_string(),
        })?;
        let count = outbox
            .values()
            .filter(|entry| {
                entry.status != RelayPublishStatus::Published
                    && entry.job.kind == kind
                    && entry.job.identifier == identifier
                    && entry.job.pubkey == pubkey
            })
            .count();
        Ok(count as i64)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AccountRepository, AnnouncementRepository, EventQuery, EventRecord, EventRepository,
        InMemoryRepositories, ProfileRepository, RelayCompatibilityRepository,
        RelayMembershipRepository, RelayPublishRepository, RelayTenantRepository,
        RepoMappingRepository, StateRepository,
    };
    use crate::{
        AccountRecord, ProfileRecord, ProfileVisibility, RelayCompatibilityRecord,
        RelayInviteRecord, RelayMembershipRecord, RelayProbeMetadata, RelayPublishRequest,
        RelayTenantRecord, RepoAnnouncementRecord, RepoMappingRecord, RepoStateRecord,
        StorageError, TagRecord,
    };
    use gittree_core::RepoMapping;
    use gittree_core::{RelayCapability, RelayCompatibilityReport, RepoAnnouncement, RepoState};
    use std::collections::HashMap;
    use std::panic::AssertUnwindSafe;
    use std::sync::RwLock;
    use time::OffsetDateTime;

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
        let mapping = RepoMapping::new("owner", "repo", hex_32(0x11), identifier).expect("mapping");
        RepoMappingRecord::new(&mapping).expect("record")
    }

    fn sample_account(username: &str, pubkey_byte: u8) -> AccountRecord {
        AccountRecord::new(&hex_32(pubkey_byte), username).expect("account")
    }

    fn sample_profile(pubkey_byte: u8) -> ProfileRecord {
        ProfileRecord::new(
            &hex_32(pubkey_byte),
            Some("Ada".to_string()),
            Some("Builder".to_string()),
            None,
            None,
            None,
            ProfileVisibility::Private,
            10,
            20,
        )
        .expect("profile")
    }

    fn sample_compat_report() -> RelayCompatibilityReport {
        RelayCompatibilityReport {
            relay_url: "wss://relay.example".to_string(),
            supported: vec![RelayCapability::Nip01, RelayCapability::Nip34],
            missing_required: Vec::new(),
            missing_optional: Vec::new(),
        }
    }

    fn sample_tenant(host: &str) -> RelayTenantRecord {
        RelayTenantRecord::new(
            host,
            host,
            &hex_32(0x44),
            vec![1, 2, 3],
            vec![4, 5, 6],
            "kid",
            Some("Tenant".to_string()),
            None,
            None,
            None,
            None,
            true,
            false,
            false,
            1,
            1,
        )
        .expect("tenant")
    }

    fn sample_membership(tenant_id: &str, pubkey_byte: u8) -> RelayMembershipRecord {
        RelayMembershipRecord::new(tenant_id, &hex_32(pubkey_byte), "member", "active", 1, 1)
            .expect("membership")
    }

    fn sample_invite(tenant_id: &str, code: &str) -> RelayInviteRecord {
        RelayInviteRecord::new(tenant_id, code, "member", &hex_32(0x55), None, None, 1)
            .expect("invite")
    }

    fn event_record(event_id: &str, pubkey: &str, created_at: i64) -> EventRecord {
        EventRecord::new(
            "default",
            event_id,
            pubkey,
            created_at,
            1,
            "content".to_string(),
            &format!("{pubkey}{pubkey}"),
            vec![vec!["e".to_string(), "tag".to_string()]],
        )
        .expect("event record")
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

    fn sample_publish_request() -> RelayPublishRequest {
        RelayPublishRequest {
            relay_url: "wss://relay.example".to_string(),
            event_id: "11".repeat(32),
            pubkey: "22".repeat(32),
            created_at: 1,
            kind: 1,
            tags: vec![vec!["d".to_string(), "repo".to_string()]],
            content: String::new(),
            sig: "33".repeat(64),
            forgejo_owner: "owner".to_string(),
            forgejo_repo: "repo".to_string(),
            identifier: "repo".to_string(),
        }
    }

    #[test]
    fn assert_internal_message_panics_for_non_internal_errors() {
        let result = std::panic::catch_unwind(|| {
            assert_internal_message(
                StorageError::InvalidField {
                    field: "tenant_id",
                    value: "empty".to_string(),
                },
                "unused",
            );
        });
        assert!(result.is_err());
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
    async fn in_memory_lists_mappings_sorted() {
        let store = InMemoryRepositories::new();
        let mapping_a = RepoMapping::new("owner", "alpha", hex_32(0x11), "alpha").expect("mapping");
        let mapping_b = RepoMapping::new("owner", "beta", hex_32(0x22), "beta").expect("mapping");
        let record_a = RepoMappingRecord::new(&mapping_a).expect("record");
        let record_b = RepoMappingRecord::new(&mapping_b).expect("record");

        store
            .upsert_mapping(record_b.clone())
            .await
            .expect("upsert");
        store
            .upsert_mapping(record_a.clone())
            .await
            .expect("upsert");

        let records = store.list_mappings().await.expect("list");
        assert_eq!(records, vec![record_a, record_b]);
    }

    #[tokio::test]
    async fn in_memory_returns_account_by_pubkey() {
        let store = InMemoryRepositories::new();
        let record = sample_account("alice", 0x11);
        store.upsert_account(record.clone()).await.expect("upsert");
        let found = store
            .account_by_pubkey(&record.pubkey)
            .await
            .expect("lookup");
        assert_eq!(found, Some(record));
    }

    #[tokio::test]
    async fn in_memory_returns_account_by_username() {
        let store = InMemoryRepositories::new();
        let record = sample_account("alice", 0x11);
        store.upsert_account(record.clone()).await.expect("upsert");
        let found = store.account_by_username("alice").await.expect("lookup");
        assert_eq!(found, Some(record));
    }

    #[tokio::test]
    async fn in_memory_rejects_account_username_collision() {
        let store = InMemoryRepositories::new();
        store
            .upsert_account(sample_account("alice", 0x11))
            .await
            .expect("first");
        let err = store
            .upsert_account(sample_account("alice", 0x22))
            .await
            .unwrap_err();
        assert!(matches!(err, crate::StorageError::Internal { .. }));
    }

    #[tokio::test]
    async fn in_memory_returns_profile_by_pubkey() {
        let store = InMemoryRepositories::new();
        let record = sample_profile(0x33);
        store.upsert_profile(record.clone()).await.expect("upsert");
        let found = store
            .profile_by_pubkey(&record.pubkey)
            .await
            .expect("lookup");
        assert_eq!(found, Some(record));
    }

    #[tokio::test]
    async fn in_memory_upserts_relay_compatibility() {
        let store = InMemoryRepositories::new();
        let report = sample_compat_report();
        let record = RelayCompatibilityRecord::new(&report, 42, &RelayProbeMetadata::default())
            .expect("record");

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

    #[tokio::test]
    async fn in_memory_tenant_membership_and_invite_flows() {
        let store = InMemoryRepositories::new();
        let tenant = sample_tenant("relay.local");
        store.upsert_tenant(tenant.clone()).await.expect("tenant");

        let looked_up = store.tenant_by_host("relay.local").await.expect("lookup");
        assert_eq!(looked_up, Some(tenant.clone()));
        assert!(
            store
                .tenant_by_host("missing.local")
                .await
                .expect("lookup")
                .is_none()
        );
        assert_eq!(store.list_tenants().await.expect("tenants").len(), 1);

        let member = sample_membership(&tenant.id, 0x33);
        store
            .upsert_membership(member.clone())
            .await
            .expect("membership");
        let listed = store.list_memberships(&tenant.id).await.expect("list");
        assert_eq!(listed, vec![member.clone()]);
        let removed = store
            .remove_membership(&tenant.id, &member.pubkey)
            .await
            .expect("remove");
        assert!(removed);
        let removed_again = store
            .remove_membership(&tenant.id, &member.pubkey)
            .await
            .expect("remove");
        assert!(!removed_again);

        let invite = sample_invite(&tenant.id, "code-1");
        store.insert_invite(invite.clone()).await.expect("invite");
        let found = store
            .invite_by_code(&tenant.id, "code-1")
            .await
            .expect("lookup");
        assert_eq!(found, Some(invite));
        store
            .delete_invite(&tenant.id, "code-1")
            .await
            .expect("delete");
        assert!(
            store
                .invite_by_code(&tenant.id, "code-1")
                .await
                .expect("lookup")
                .is_none()
        );
    }

    #[tokio::test]
    async fn in_memory_membership_and_invite_are_tenant_scoped() {
        let store = InMemoryRepositories::new();
        let tenant_a = sample_tenant("relay-a.local");
        let tenant_b = sample_tenant("relay-b.local");
        store
            .upsert_tenant(tenant_a.clone())
            .await
            .expect("tenant a");
        store
            .upsert_tenant(tenant_b.clone())
            .await
            .expect("tenant b");

        let member = sample_membership(&tenant_a.id, 0x70);
        store
            .upsert_membership(member.clone())
            .await
            .expect("membership");

        assert!(
            store
                .membership_by_pubkey(&tenant_b.id, &member.pubkey)
                .await
                .expect("lookup")
                .is_none()
        );
        let removed_wrong_tenant = store
            .remove_membership(&tenant_b.id, &member.pubkey)
            .await
            .expect("remove");
        assert!(!removed_wrong_tenant);

        let invite = sample_invite(&tenant_a.id, "tenant-scoped-code");
        store.insert_invite(invite.clone()).await.expect("invite");
        assert!(
            store
                .invite_by_code(&tenant_b.id, &invite.invite_code)
                .await
                .expect("lookup")
                .is_none()
        );
        store
            .delete_invite(&tenant_b.id, &invite.invite_code)
            .await
            .expect("delete");
        let still_present = store
            .invite_by_code(&tenant_a.id, &invite.invite_code)
            .await
            .expect("lookup");
        assert_eq!(still_present, Some(invite));
    }

    #[tokio::test]
    async fn in_memory_outbox_claims_and_marks_success() {
        let store = InMemoryRepositories::new();
        let request = sample_publish_request();
        store.enqueue_relay_publish(request).await.expect("enqueue");
        let job = store
            .claim_relay_publish(OffsetDateTime::now_utc())
            .await
            .expect("claim")
            .expect("job");
        assert_eq!(job.relay_url, "wss://relay.example");
        store
            .mark_relay_publish_succeeded(job.id)
            .await
            .expect("mark");
        let remaining = store
            .pending_relay_publishes(&job.pubkey, &job.identifier, job.kind)
            .await
            .expect("count");
        assert_eq!(remaining, 0);
    }

    #[tokio::test]
    async fn in_memory_outbox_marks_failed_and_reclaims_after_retry() {
        let store = InMemoryRepositories::new();
        let mut request = sample_publish_request();
        request.tags = Vec::new();
        store.enqueue_relay_publish(request).await.expect("enqueue");
        let now = OffsetDateTime::now_utc();
        let first = store
            .claim_relay_publish(now)
            .await
            .expect("claim")
            .expect("job");
        let retry_at = now + time::Duration::seconds(60);
        store
            .mark_relay_publish_failed(first.id, "retry", retry_at)
            .await
            .expect("failed");
        let none_early = store
            .claim_relay_publish(now + time::Duration::seconds(30))
            .await
            .expect("claim");
        assert!(none_early.is_none());
        let second = store
            .claim_relay_publish(now + time::Duration::seconds(90))
            .await
            .expect("claim")
            .expect("job");
        assert_eq!(second.id, first.id);
        assert_eq!(second.attempt_count, 2);
    }

    #[tokio::test]
    async fn in_memory_event_repo_inserts_and_reads() {
        let store = InMemoryRepositories::new();
        let event_id = "11".repeat(32);
        let pubkey = "22".repeat(32);
        let record = event_record(&event_id, &pubkey, 1);

        store.insert_event(record.clone()).await.expect("insert");
        let fetched = store
            .get_event("default", &hex::decode(event_id).expect("id"))
            .await
            .expect("get")
            .expect("record");
        assert_eq!(fetched, record);
    }

    #[tokio::test]
    async fn in_memory_event_repo_filters_tags_and_kinds() {
        let store = InMemoryRepositories::new();
        let event_id = "aa".repeat(32);
        let pubkey = "bb".repeat(32);
        let mut record = event_record(&event_id, &pubkey, 5);
        record.kind = 30000;
        record.tags = vec![
            TagRecord {
                name: "d".to_string(),
                value: "repo".to_string(),
            },
            TagRecord {
                name: "e".to_string(),
                value: "tag".to_string(),
            },
        ];

        store.insert_event(record.clone()).await.expect("insert");

        let query = EventQuery {
            kinds: vec![30000],
            tags: vec![TagRecord {
                name: "d".to_string(),
                value: "repo".to_string(),
            }],
            ..EventQuery::default()
        };

        let results = store.query_events(&query).await.expect("query");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], record);
    }

    #[tokio::test]
    async fn in_memory_event_repo_applies_limits_and_invalid_id_filters() {
        let store = InMemoryRepositories::new();
        let first_id = "aa".repeat(32);
        let second_id = "bb".repeat(32);
        let pubkey = "cc".repeat(32);
        let first = event_record(&first_id, &pubkey, 1);
        let second = event_record(&second_id, &pubkey, 2);
        store.insert_event(first.clone()).await.expect("insert");
        store.insert_event(second.clone()).await.expect("insert");

        let limited = store
            .query_events(&EventQuery {
                limit: Some(1),
                ..EventQuery::default()
            })
            .await
            .expect("query");
        assert_eq!(limited, vec![second.clone()]);

        let none = store
            .query_events(&EventQuery {
                ids: vec!["not-hex".to_string()],
                ..EventQuery::default()
            })
            .await
            .expect("query");
        assert!(none.is_empty());

        let since_filtered = store
            .query_events(&EventQuery {
                since: Some(2),
                ..EventQuery::default()
            })
            .await
            .expect("query");
        assert_eq!(since_filtered, vec![second]);
    }

    #[tokio::test]
    async fn in_memory_event_repo_filters_tenant_ids_and_valid_hex_refs() {
        let store = InMemoryRepositories::new();
        let event_id = "aa".repeat(32);
        let author = "bb".repeat(32);

        let mut tenant_a = event_record(&event_id, &author, 2);
        tenant_a.tenant_id = "tenant-a".to_string();
        let mut tenant_b = event_record(&"cc".repeat(32), &author, 1);
        tenant_b.tenant_id = "tenant-b".to_string();

        store
            .insert_event(tenant_a.clone())
            .await
            .expect("insert tenant a");
        store.insert_event(tenant_b).await.expect("insert tenant b");

        let query = EventQuery {
            tenant_id: Some("tenant-a".to_string()),
            ids: vec![event_id],
            authors: vec![author],
            ..EventQuery::default()
        };

        let results = store.query_events(&query).await.expect("query");
        assert_eq!(results, vec![tenant_a]);
    }

    #[tokio::test]
    async fn in_memory_event_repo_applies_until_and_tie_break_sorting() {
        let store = InMemoryRepositories::new();
        let author = "cc".repeat(32);

        let mut first = event_record(&"aa".repeat(32), &author, 10);
        first.tenant_id = "tenant-a".to_string();
        let mut second = event_record(&"bb".repeat(32), &author, 10);
        second.tenant_id = "tenant-a".to_string();
        let mut newer = event_record(&"dd".repeat(32), &author, 11);
        newer.tenant_id = "tenant-a".to_string();

        store
            .insert_event(second.clone())
            .await
            .expect("insert second");
        store
            .insert_event(first.clone())
            .await
            .expect("insert first");
        store.insert_event(newer).await.expect("insert newer");

        let results = store
            .query_events(&EventQuery {
                tenant_id: Some("tenant-a".to_string()),
                until: Some(10),
                ..EventQuery::default()
            })
            .await
            .expect("query");

        assert_eq!(results, vec![first, second]);
    }

    #[tokio::test]
    async fn in_memory_event_repo_rejects_invalid_authors_kinds_and_tag_filters() {
        let store = InMemoryRepositories::new();
        let event_id = "aa".repeat(32);
        let author = "bb".repeat(32);
        let mut record = event_record(&event_id, &author, 5);
        record.kind = 30000;
        record.tags = vec![
            TagRecord {
                name: "d".to_string(),
                value: "repo".to_string(),
            },
            TagRecord {
                name: "e".to_string(),
                value: "tag".to_string(),
            },
        ];
        store.insert_event(record).await.expect("insert");

        let invalid_author = store
            .query_events(&EventQuery {
                authors: vec!["not-hex".to_string()],
                ..EventQuery::default()
            })
            .await
            .expect("query invalid author");
        assert!(invalid_author.is_empty());

        let kind_mismatch = store
            .query_events(&EventQuery {
                kinds: vec![1],
                ..EventQuery::default()
            })
            .await
            .expect("query kind mismatch");
        assert!(kind_mismatch.is_empty());

        let missing_tag_name = store
            .query_events(&EventQuery {
                tags: vec![TagRecord {
                    name: "p".to_string(),
                    value: "alice".to_string(),
                }],
                ..EventQuery::default()
            })
            .await
            .expect("query missing tag name");
        assert!(missing_tag_name.is_empty());

        let missing_tag_value = store
            .query_events(&EventQuery {
                tags: vec![TagRecord {
                    name: "d".to_string(),
                    value: "other".to_string(),
                }],
                ..EventQuery::default()
            })
            .await
            .expect("query missing tag value");
        assert!(missing_tag_value.is_empty());
    }

    #[tokio::test]
    async fn in_memory_outbox_missing_entries_return_internal_error() {
        let store = InMemoryRepositories::new();
        assert!(
            store
                .claim_relay_publish(OffsetDateTime::now_utc())
                .await
                .expect("claim")
                .is_none()
        );

        let missing_succeeded = store.mark_relay_publish_succeeded(42).await.unwrap_err();
        assert_internal_message(missing_succeeded, "outbox entry not found");

        let missing_failed = store
            .mark_relay_publish_failed(42, "missing", OffsetDateTime::now_utc())
            .await
            .unwrap_err();
        assert_internal_message(missing_failed, "outbox entry not found");
    }

    #[tokio::test]
    async fn in_memory_pending_relay_publishes_filters_kind_identifier_and_pubkey() {
        let store = InMemoryRepositories::new();
        store
            .enqueue_relay_publish(sample_publish_request())
            .await
            .expect("enqueue");

        assert_eq!(
            store
                .pending_relay_publishes(&hex::decode(hex_32(0x22)).expect("pubkey"), "repo", 2)
                .await
                .expect("count"),
            0
        );
        assert_eq!(
            store
                .pending_relay_publishes(
                    &hex::decode(hex_32(0x22)).expect("pubkey"),
                    "other-repo",
                    1
                )
                .await
                .expect("count"),
            0
        );
        assert_eq!(
            store
                .pending_relay_publishes(&hex::decode(hex_32(0x99)).expect("pubkey"), "repo", 1)
                .await
                .expect("count"),
            0
        );
        assert_eq!(
            store
                .pending_relay_publishes(&hex::decode(hex_32(0x22)).expect("pubkey"), "repo", 1)
                .await
                .expect("count"),
            1
        );
    }

    #[tokio::test]
    async fn in_memory_reports_poisoned_announcement_and_state_stores() {
        let store = InMemoryRepositories::new();
        let announcement = RepoAnnouncementRecord::new(
            &hex_32(0x11),
            &hex_32(0x22),
            10,
            &sample_announcement("repo"),
        )
        .expect("announcement");
        poison_lock(&store.announcements);
        let insert_err = store
            .insert_announcement(announcement.clone())
            .await
            .unwrap_err();
        assert_internal_message(insert_err, "announcement store poisoned");
        let list_err = store
            .list_announcements(&announcement.pubkey, "repo")
            .await
            .unwrap_err();
        assert_internal_message(list_err, "announcement store poisoned");

        let state_store = InMemoryRepositories::new();
        let state = RepoStateRecord::new(&hex_32(0x11), &hex_32(0x22), 10, &sample_state("repo"))
            .expect("state");
        poison_lock(&state_store.states);
        let insert_state_err = state_store.insert_state(state.clone()).await.unwrap_err();
        assert_internal_message(insert_state_err, "state store poisoned");
        let latest_state_err = state_store
            .latest_state(&state.pubkey, "repo")
            .await
            .unwrap_err();
        assert_internal_message(latest_state_err, "state store poisoned");
    }

    #[tokio::test]
    async fn in_memory_reports_poisoned_mapping_and_account_stores() {
        let mapping = sample_mapping("repo");

        let forgejo_store = InMemoryRepositories::new();
        poison_lock(&forgejo_store.mappings_by_forgejo);
        let forgejo_upsert_err = forgejo_store
            .upsert_mapping(mapping.clone())
            .await
            .unwrap_err();
        assert_internal_message(forgejo_upsert_err, "forgejo mapping store poisoned");
        let forgejo_read_err = forgejo_store
            .mapping_by_forgejo("owner", "repo")
            .await
            .unwrap_err();
        assert_internal_message(forgejo_read_err, "forgejo mapping store poisoned");

        let repo_store = InMemoryRepositories::new();
        poison_lock(&repo_store.mappings_by_repo);
        let repo_upsert_err = repo_store
            .upsert_mapping(mapping.clone())
            .await
            .unwrap_err();
        assert_internal_message(repo_upsert_err, "repo mapping store poisoned");
        let repo_read_err = repo_store
            .mapping_by_repo(&mapping.pubkey, &mapping.identifier)
            .await
            .unwrap_err();
        assert_internal_message(repo_read_err, "repo mapping store poisoned");
        let list_err = repo_store.list_mappings().await.unwrap_err();
        assert_internal_message(list_err, "repo mapping store poisoned");

        let account = sample_account("alice", 0x11);
        let pubkey_store = InMemoryRepositories::new();
        poison_lock(&pubkey_store.accounts_by_pubkey);
        let pubkey_write_err = pubkey_store
            .upsert_account(account.clone())
            .await
            .unwrap_err();
        assert_internal_message(pubkey_write_err, "account pubkey store poisoned");
        let pubkey_read_err = pubkey_store
            .account_by_pubkey(&account.pubkey)
            .await
            .unwrap_err();
        assert_internal_message(pubkey_read_err, "account pubkey store poisoned");

        let username_store = InMemoryRepositories::new();
        poison_lock(&username_store.accounts_by_username);
        let username_write_err = username_store
            .upsert_account(account.clone())
            .await
            .unwrap_err();
        assert_internal_message(username_write_err, "account username store poisoned");
        let username_read_err = username_store
            .account_by_username(&account.forgejo_username)
            .await
            .unwrap_err();
        assert_internal_message(username_read_err, "account username store poisoned");
    }

    #[tokio::test]
    async fn in_memory_reports_poisoned_profile_event_tenant_membership_and_outbox_stores() {
        let profile_store = InMemoryRepositories::new();
        let profile = sample_profile(0x44);
        poison_lock(&profile_store.profiles_by_pubkey);
        let profile_write_err = profile_store
            .upsert_profile(profile.clone())
            .await
            .unwrap_err();
        assert_internal_message(profile_write_err, "profile store poisoned");
        let profile_read_err = profile_store
            .profile_by_pubkey(&profile.pubkey)
            .await
            .unwrap_err();
        assert_internal_message(profile_read_err, "profile store poisoned");

        let compat_store = InMemoryRepositories::new();
        let report = sample_compat_report();
        let compat = RelayCompatibilityRecord::new(&report, 10, &RelayProbeMetadata::default())
            .expect("compat");
        poison_lock(&compat_store.relay_compatibility);
        let compat_write_err = compat_store
            .upsert_relay_compatibility(compat.clone())
            .await
            .unwrap_err();
        assert_internal_message(compat_write_err, "relay compatibility store poisoned");
        let compat_read_err = compat_store
            .relay_compatibility(&report.relay_url)
            .await
            .unwrap_err();
        assert_internal_message(compat_read_err, "relay compatibility store poisoned");

        let tenant = sample_tenant("relay.local");
        let tenant_id_store = InMemoryRepositories::new();
        poison_lock(&tenant_id_store.tenants_by_id);
        let tenant_id_write_err = tenant_id_store
            .upsert_tenant(tenant.clone())
            .await
            .unwrap_err();
        assert_internal_message(tenant_id_write_err, "tenant store poisoned");
        let tenant_id_read_err = tenant_id_store.tenant_by_id(&tenant.id).await.unwrap_err();
        assert_internal_message(tenant_id_read_err, "tenant store poisoned");
        let tenant_list_err = tenant_id_store.list_tenants().await.unwrap_err();
        assert_internal_message(tenant_list_err, "tenant store poisoned");

        let tenant_host_store = InMemoryRepositories::new();
        poison_lock(&tenant_host_store.tenants_by_host);
        let tenant_host_write_err = tenant_host_store
            .upsert_tenant(tenant.clone())
            .await
            .unwrap_err();
        assert_internal_message(tenant_host_write_err, "tenant store poisoned");
        let tenant_host_read_err = tenant_host_store
            .tenant_by_host("relay.local")
            .await
            .unwrap_err();
        assert_internal_message(tenant_host_read_err, "tenant store poisoned");

        let tenant_lookup_store = InMemoryRepositories::new();
        tenant_lookup_store
            .upsert_tenant(tenant.clone())
            .await
            .expect("upsert");
        poison_lock(&tenant_lookup_store.tenants_by_id);
        let tenant_lookup_err = tenant_lookup_store
            .tenant_by_host("relay.local")
            .await
            .unwrap_err();
        assert_internal_message(tenant_lookup_err, "tenant store poisoned");

        let membership = sample_membership(&tenant.id, 0x33);
        let membership_store = InMemoryRepositories::new();
        poison_lock(&membership_store.memberships);
        let membership_write_err = membership_store
            .upsert_membership(membership.clone())
            .await
            .unwrap_err();
        assert_internal_message(membership_write_err, "membership store poisoned");
        let membership_read_err = membership_store
            .membership_by_pubkey(&tenant.id, &membership.pubkey)
            .await
            .unwrap_err();
        assert_internal_message(membership_read_err, "membership store poisoned");
        let membership_list_err = membership_store
            .list_memberships(&tenant.id)
            .await
            .unwrap_err();
        assert_internal_message(membership_list_err, "membership store poisoned");
        let membership_remove_err = membership_store
            .remove_membership(&tenant.id, &membership.pubkey)
            .await
            .unwrap_err();
        assert_internal_message(membership_remove_err, "membership store poisoned");

        let invite = sample_invite(&tenant.id, "code-1");
        let invite_store = InMemoryRepositories::new();
        poison_lock(&invite_store.invites);
        let invite_write_err = invite_store
            .insert_invite(invite.clone())
            .await
            .unwrap_err();
        assert_internal_message(invite_write_err, "invite store poisoned");
        let invite_read_err = invite_store
            .invite_by_code(&tenant.id, &invite.invite_code)
            .await
            .unwrap_err();
        assert_internal_message(invite_read_err, "invite store poisoned");
        let invite_delete_err = invite_store
            .delete_invite(&tenant.id, &invite.invite_code)
            .await
            .unwrap_err();
        assert_internal_message(invite_delete_err, "invite store poisoned");

        let event_store = InMemoryRepositories::new();
        let event = event_record(&"aa".repeat(32), &"bb".repeat(32), 1);
        poison_lock(&event_store.events);
        let event_write_err = event_store.insert_event(event.clone()).await.unwrap_err();
        assert_internal_message(event_write_err, "event store poisoned");
        let event_read_err = event_store
            .get_event(&event.tenant_id, &event.id)
            .await
            .unwrap_err();
        assert_internal_message(event_read_err, "event store poisoned");
        let event_query_err = event_store
            .query_events(&EventQuery::default())
            .await
            .unwrap_err();
        assert_internal_message(event_query_err, "event store poisoned");
        let event_delete_err = event_store
            .delete_event(&event.tenant_id, &event.id)
            .await
            .unwrap_err();
        assert_internal_message(event_delete_err, "event store poisoned");

        let outbox_store = InMemoryRepositories::new();
        poison_lock(&outbox_store.outbox);
        let outbox_enqueue_err = outbox_store
            .enqueue_relay_publish(sample_publish_request())
            .await
            .unwrap_err();
        assert_internal_message(outbox_enqueue_err, "outbox store poisoned");
        let outbox_claim_err = outbox_store
            .claim_relay_publish(OffsetDateTime::now_utc())
            .await
            .unwrap_err();
        assert_internal_message(outbox_claim_err, "outbox store poisoned");
        let outbox_mark_success_err = outbox_store
            .mark_relay_publish_succeeded(1)
            .await
            .unwrap_err();
        assert_internal_message(outbox_mark_success_err, "outbox store poisoned");
        let outbox_mark_failed_err = outbox_store
            .mark_relay_publish_failed(1, "boom", OffsetDateTime::now_utc())
            .await
            .unwrap_err();
        assert_internal_message(outbox_mark_failed_err, "outbox store poisoned");
        let outbox_pending_err = outbox_store
            .pending_relay_publishes(&hex::decode(hex_32(0x22)).expect("pubkey"), "repo", 1)
            .await
            .unwrap_err();
        assert_internal_message(outbox_pending_err, "outbox store poisoned");
    }
}
