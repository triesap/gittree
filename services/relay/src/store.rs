use crate::{Filter, NostrEvent, TagIndex};
use async_trait::async_trait;
use gittree_storage::{EventQuery, EventRecord, EventRepository, TagRecord};
use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreOutcome {
    Inserted,
    Duplicate,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreError {
    Backend(String),
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreError::Backend(message) => write!(f, "store error: {message}"),
        }
    }
}

impl std::error::Error for StoreError {}

#[async_trait]
pub trait EventStore: Send + Sync {
    async fn insert(&self, event: NostrEvent) -> Result<StoreOutcome, StoreError>;
    async fn get(&self, id: &str) -> Result<Option<NostrEvent>, StoreError>;
    async fn delete(&self, id: &str) -> Result<bool, StoreError>;
    async fn query(&self, filters: &[Filter]) -> Result<Vec<NostrEvent>, StoreError>;
}

#[derive(Debug, Default)]
struct MemoryStoreState {
    events: BTreeMap<String, NostrEvent>,
    replaceable: BTreeMap<ReplaceableKey, String>,
}

impl MemoryStoreState {
    fn remove_event(&mut self, id: &str) -> bool {
        let Some(event) = self.events.remove(id) else {
            return false;
        };
        if let Some(key) = replaceable_key(&event) {
            self.replaceable.remove(&key);
        }
        true
    }

    fn apply_delete(&mut self, event: &NostrEvent) {
        let targets = collect_tag_values(&event.tags, "e");
        for target in targets {
            if let Some(existing) = self.events.get(&target) {
                if existing.pubkey == event.pubkey && existing.created_at <= event.created_at {
                    self.remove_event(&target);
                }
            }
        }

        let addresses = collect_tag_values(&event.tags, "a");
        for address in addresses {
            let Some(key) = parse_address(&address) else {
                continue;
            };
            let Some(existing_id) = self.replaceable.get(&key).cloned() else {
                continue;
            };
            let Some(existing) = self.events.get(&existing_id) else {
                continue;
            };
            if existing.pubkey == event.pubkey && existing.created_at <= event.created_at {
                self.remove_event(&existing_id);
            }
        }
    }
}

#[derive(Debug, Default)]
pub struct MemoryStore {
    state: RwLock<MemoryStoreState>,
}

impl MemoryStore {
    pub fn new() -> Self {
        Self {
            state: RwLock::new(MemoryStoreState::default()),
        }
    }
}

#[derive(Debug, Clone)]
pub struct RepositoryStore<R: EventRepository> {
    repo: R,
    tenant_id: String,
}

impl<R: EventRepository> RepositoryStore<R> {
    pub fn new(repo: R) -> Self {
        Self {
            repo,
            tenant_id: DEFAULT_TENANT_ID.to_string(),
        }
    }

    pub fn with_tenant(repo: R, tenant_id: impl Into<String>) -> Self {
        Self {
            repo,
            tenant_id: tenant_id.into(),
        }
    }
}

#[async_trait]
impl EventStore for MemoryStore {
    async fn insert(&self, event: NostrEvent) -> Result<StoreOutcome, StoreError> {
        let mut state = self.state.write().await;
        if state.events.contains_key(&event.id) {
            return Ok(StoreOutcome::Duplicate);
        }

        if event.kind == 5 {
            state.apply_delete(&event);
        }

        if let Some(key) = replaceable_key(&event) {
            if let Some(existing_id) = state.replaceable.get(&key).cloned() {
                if let Some(existing) = state.events.get(&existing_id) {
                    if existing.created_at >= event.created_at {
                        return Ok(StoreOutcome::Duplicate);
                    }
                }
                state.remove_event(&existing_id);
            }
            state.replaceable.insert(key, event.id.clone());
        }

        state.events.insert(event.id.clone(), event);
        Ok(StoreOutcome::Inserted)
    }

    async fn get(&self, id: &str) -> Result<Option<NostrEvent>, StoreError> {
        let state = self.state.read().await;
        Ok(state.events.get(id).cloned())
    }

    async fn delete(&self, id: &str) -> Result<bool, StoreError> {
        let mut state = self.state.write().await;
        Ok(state.remove_event(id))
    }

    async fn query(&self, filters: &[Filter]) -> Result<Vec<NostrEvent>, StoreError> {
        if filters.is_empty() {
            return Ok(Vec::new());
        }

        let state = self.state.read().await;
        let mut ordered: Vec<&NostrEvent> = state.events.values().collect();
        ordered.sort_by(|left, right| {
            right
                .created_at
                .cmp(&left.created_at)
                .then_with(|| left.id.cmp(&right.id))
        });

        let mut seen = HashSet::new();
        let mut results = Vec::new();
        for filter in filters {
            let mut remaining = filter.limit.unwrap_or(u64::MAX);
            for event in &ordered {
                if remaining == 0 {
                    break;
                }
                if seen.contains(&event.id) {
                    continue;
                }
                let tags = TagIndex::from_tags(&event.tags)
                    .map_err(|err| StoreError::Backend(err.to_string()))?;
                if filter.matches(event, &tags) {
                    results.push((*event).clone());
                    seen.insert(event.id.clone());
                    remaining = remaining.saturating_sub(1);
                }
            }
        }
        Ok(results)
    }
}

#[async_trait]
impl<T> EventStore for Arc<T>
where
    T: EventStore + Send + Sync + ?Sized,
{
    async fn insert(&self, event: NostrEvent) -> Result<StoreOutcome, StoreError> {
        (**self).insert(event).await
    }

    async fn get(&self, id: &str) -> Result<Option<NostrEvent>, StoreError> {
        (**self).get(id).await
    }

    async fn delete(&self, id: &str) -> Result<bool, StoreError> {
        (**self).delete(id).await
    }

    async fn query(&self, filters: &[Filter]) -> Result<Vec<NostrEvent>, StoreError> {
        (**self).query(filters).await
    }
}

#[async_trait]
impl<R: EventRepository> EventStore for RepositoryStore<R> {
    async fn insert(&self, event: NostrEvent) -> Result<StoreOutcome, StoreError> {
        let record = event_to_record(&event, &self.tenant_id)?;
        if self
            .repo
            .get_event(&self.tenant_id, &record.id)
            .await
            .map_err(map_repo_err)?
            .is_some()
        {
            return Ok(StoreOutcome::Duplicate);
        }

        if event.kind == 5 {
            apply_delete_repo(&self.repo, &self.tenant_id, &event).await?;
        }

        if let Some(key) = replaceable_key(&event) {
            if apply_replaceable_repo(&self.repo, &self.tenant_id, &event, &key).await? {
                return Ok(StoreOutcome::Duplicate);
            }
        }

        self.repo.insert_event(record).await.map_err(map_repo_err)?;
        Ok(StoreOutcome::Inserted)
    }

    async fn get(&self, id: &str) -> Result<Option<NostrEvent>, StoreError> {
        let bytes = decode_hex_id(id)?;
        let record = self
            .repo
            .get_event(&self.tenant_id, &bytes)
            .await
            .map_err(map_repo_err)?;
        record.map(record_to_event).transpose()
    }

    async fn delete(&self, id: &str) -> Result<bool, StoreError> {
        let bytes = decode_hex_id(id)?;
        self.repo
            .delete_event(&self.tenant_id, &bytes)
            .await
            .map_err(map_repo_err)
    }

    async fn query(&self, filters: &[Filter]) -> Result<Vec<NostrEvent>, StoreError> {
        if filters.is_empty() {
            return Ok(Vec::new());
        }

        let mut seen = HashSet::new();
        let mut results = Vec::new();
        for filter in filters {
            let plan = build_query_plan(filter);
            let mut query = plan.query;
            query.tenant_id = Some(self.tenant_id.clone());
            let records = self.repo.query_events(&query).await.map_err(map_repo_err)?;
            let mut remaining = filter.limit.unwrap_or(u64::MAX);
            for record in records {
                if remaining == 0 {
                    break;
                }
                let event = record_to_event(record)?;
                let event_id = event.id.clone();
                if seen.contains(&event_id) {
                    continue;
                }
                if plan.needs_post_filter {
                    let tags = TagIndex::from_tags(&event.tags)
                        .map_err(|err| StoreError::Backend(err.to_string()))?;
                    if !filter.matches(&event, &tags) {
                        continue;
                    }
                }
                results.push(event);
                seen.insert(event_id);
                remaining = remaining.saturating_sub(1);
            }
        }

        results.sort_by(|left, right| {
            right
                .created_at
                .cmp(&left.created_at)
                .then_with(|| left.id.cmp(&right.id))
        });
        Ok(results)
    }
}

async fn apply_delete_repo<R: EventRepository>(
    repo: &R,
    tenant_id: &str,
    event: &NostrEvent,
) -> Result<(), StoreError> {
    let author = decode_hex_pubkey(&event.pubkey)?;

    for target in collect_tag_values(&event.tags, "e") {
        let Ok(bytes) = hex::decode(&target) else {
            continue;
        };
        let record = repo
            .get_event(tenant_id, &bytes)
            .await
            .map_err(map_repo_err)?;
        let Some(record) = record else {
            continue;
        };
        if record.pubkey == author && record.created_at <= event.created_at {
            repo.delete_event(tenant_id, &record.id)
                .await
                .map_err(map_repo_err)?;
        }
    }

    for address in collect_tag_values(&event.tags, "a") {
        let Some(key) = parse_address(&address) else {
            continue;
        };
        if key.pubkey != event.pubkey {
            continue;
        }
        let mut query = EventQuery::default();
        query.tenant_id = Some(tenant_id.to_string());
        query.kinds = vec![key.kind];
        query.authors = vec![key.pubkey.clone()];
        if let Some(identifier) = key.identifier {
            query.tags = vec![TagRecord::new("d", identifier)];
        }
        let records = repo.query_events(&query).await.map_err(map_repo_err)?;
        for record in records {
            if record.created_at <= event.created_at {
                repo.delete_event(tenant_id, &record.id)
                    .await
                    .map_err(map_repo_err)?;
            }
        }
    }

    Ok(())
}

async fn apply_replaceable_repo<R: EventRepository>(
    repo: &R,
    tenant_id: &str,
    event: &NostrEvent,
    key: &ReplaceableKey,
) -> Result<bool, StoreError> {
    let mut query = EventQuery::default();
    query.tenant_id = Some(tenant_id.to_string());
    query.kinds = vec![key.kind];
    query.authors = vec![key.pubkey.clone()];
    if let Some(identifier) = &key.identifier {
        query.tags = vec![TagRecord::new("d", identifier.clone())];
    }
    let records = repo.query_events(&query).await.map_err(map_repo_err)?;
    if records
        .iter()
        .any(|record| record.created_at >= event.created_at)
    {
        return Ok(true);
    }
    for record in records {
        repo.delete_event(tenant_id, &record.id)
            .await
            .map_err(map_repo_err)?;
    }
    Ok(false)
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct ReplaceableKey {
    kind: u32,
    pubkey: String,
    identifier: Option<String>,
}

fn replaceable_key(event: &NostrEvent) -> Option<ReplaceableKey> {
    if is_replaceable_kind(event.kind) {
        return Some(ReplaceableKey {
            kind: event.kind,
            pubkey: event.pubkey.clone(),
            identifier: None,
        });
    }
    if is_parameterized_replaceable_kind(event.kind) {
        let identifier = collect_tag_values(&event.tags, "d").into_iter().next()?;
        return Some(ReplaceableKey {
            kind: event.kind,
            pubkey: event.pubkey.clone(),
            identifier: Some(identifier),
        });
    }
    None
}

fn is_replaceable_kind(kind: u32) -> bool {
    kind == 0 || kind == 3 || (10000..20000).contains(&kind)
}

fn is_parameterized_replaceable_kind(kind: u32) -> bool {
    (30000..40000).contains(&kind)
}

fn collect_tag_values(tags: &[Vec<String>], name: &str) -> Vec<String> {
    let mut values = Vec::new();
    for tag in tags {
        if tag.first().map(|value| value == name).unwrap_or(false) {
            values.extend(tag.iter().skip(1).cloned());
        }
    }
    values
}

fn parse_address(value: &str) -> Option<ReplaceableKey> {
    let mut parts = value.split(':');
    let kind = parts.next()?.parse::<u32>().ok()?;
    let pubkey = parts.next()?.to_string();
    let identifier = parts.next().map(|part| part.to_string());
    Some(ReplaceableKey {
        kind,
        pubkey,
        identifier,
    })
}

fn event_to_record(event: &NostrEvent, tenant_id: &str) -> Result<EventRecord, StoreError> {
    EventRecord::new(
        tenant_id,
        &event.id,
        &event.pubkey,
        event.created_at,
        event.kind,
        event.content.clone(),
        &event.sig,
        event.tags.clone(),
    )
    .map_err(|err| StoreError::Backend(err.to_string()))
}

fn record_to_event(record: EventRecord) -> Result<NostrEvent, StoreError> {
    let id = hex::encode(record.id);
    let pubkey = hex::encode(record.pubkey);
    let sig = hex::encode(record.sig);
    let tags = group_tags(&record.tags);
    Ok(NostrEvent {
        id,
        pubkey,
        created_at: record.created_at,
        kind: record.kind,
        tags,
        content: record.content,
        sig,
    })
}

fn group_tags(tags: &[TagRecord]) -> Vec<Vec<String>> {
    let mut by_name: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for tag in tags {
        by_name
            .entry(tag.name.clone())
            .or_default()
            .push(tag.value.clone());
    }
    by_name
        .into_iter()
        .map(|(name, values)| {
            let mut row = Vec::with_capacity(values.len() + 1);
            row.push(name);
            row.extend(values);
            row
        })
        .collect()
}

const HEX_32_LEN: usize = 64;
const DEFAULT_TENANT_ID: &str = "default";

struct QueryPlan {
    query: EventQuery,
    needs_post_filter: bool,
}

fn build_query_plan(filter: &Filter) -> QueryPlan {
    let (ids, ids_need_post) = exact_hex_filters(&filter.ids);
    let (authors, authors_need_post) = exact_hex_filters(&filter.authors);
    let needs_post_filter = ids_need_post || authors_need_post;
    let mut tags = Vec::new();
    for (name, values) in &filter.tags {
        for value in values {
            tags.push(TagRecord {
                name: name.clone(),
                value: value.clone(),
            });
        }
    }

    QueryPlan {
        query: EventQuery {
            tenant_id: None,
            ids,
            authors,
            kinds: filter.kinds.clone(),
            since: filter.since,
            until: filter.until,
            tags,
            limit: if needs_post_filter {
                None
            } else {
                filter.limit
            },
        },
        needs_post_filter,
    }
}

fn exact_hex_filters(values: &[String]) -> (Vec<String>, bool) {
    if values.is_empty() {
        return (Vec::new(), false);
    }
    let all_exact = values
        .iter()
        .all(|value| value.len() == HEX_32_LEN && hex::decode(value).is_ok());
    if all_exact {
        (values.to_vec(), false)
    } else {
        (Vec::new(), true)
    }
}

fn decode_hex_id(value: &str) -> Result<Vec<u8>, StoreError> {
    hex::decode(value).map_err(|_| StoreError::Backend("invalid event id".to_string()))
}

fn decode_hex_pubkey(value: &str) -> Result<Vec<u8>, StoreError> {
    hex::decode(value).map_err(|_| StoreError::Backend("invalid pubkey".to_string()))
}

fn map_repo_err(err: gittree_storage::StorageError) -> StoreError {
    StoreError::Backend(err.to_string())
}

#[cfg(test)]
mod tests {
    use super::{
        EventStore, MemoryStore, MemoryStoreState, RepositoryStore, StoreError, StoreOutcome,
        apply_delete_repo, apply_replaceable_repo, collect_tag_values, event_to_record,
        exact_hex_filters, parse_address, replaceable_key,
    };
    use crate::NostrEvent;
    use async_trait::async_trait;
    use gittree_storage::{
        EventQuery, EventRecord, EventRepository, InMemoryRepositories, PostgresRepositories,
        StorageConfig, StorageError, TagRecord,
    };
    use serde_json::json;
    use std::sync::Arc;
    use std::time::Duration;

    #[derive(Debug, Clone)]
    struct ScriptedEventRepo {
        fail_insert: bool,
        fail_get: bool,
        fail_delete: bool,
        fail_query: bool,
        query_results: Vec<EventRecord>,
    }

    impl ScriptedEventRepo {
        fn insert_error() -> Self {
            Self {
                fail_insert: true,
                fail_get: false,
                fail_delete: false,
                fail_query: false,
                query_results: Vec::new(),
            }
        }

        fn get_error() -> Self {
            Self {
                fail_insert: false,
                fail_get: true,
                fail_delete: false,
                fail_query: false,
                query_results: Vec::new(),
            }
        }

        fn query_error() -> Self {
            Self {
                fail_insert: false,
                fail_get: false,
                fail_delete: false,
                fail_query: true,
                query_results: Vec::new(),
            }
        }

        fn delete_error() -> Self {
            Self {
                fail_insert: false,
                fail_get: false,
                fail_delete: true,
                fail_query: false,
                query_results: Vec::new(),
            }
        }

        fn with_query_results(query_results: Vec<EventRecord>) -> Self {
            Self {
                fail_insert: false,
                fail_get: false,
                fail_delete: false,
                fail_query: false,
                query_results,
            }
        }
    }

    fn delete_target_record() -> EventRecord {
        EventRecord::new(
            "default",
            &"11".repeat(32),
            &"aa".repeat(32),
            0,
            1,
            "".to_string(),
            &"00".repeat(64),
            Vec::new(),
        )
        .expect("event record")
    }

    #[async_trait]
    impl EventRepository for ScriptedEventRepo {
        async fn insert_event(&self, _record: EventRecord) -> Result<(), StorageError> {
            if self.fail_insert {
                return Err(StorageError::Internal {
                    message: "insert failure".to_string(),
                });
            }
            Ok(())
        }

        async fn get_event(
            &self,
            _tenant_id: &str,
            event_id: &[u8],
        ) -> Result<Option<EventRecord>, StorageError> {
            if self.fail_get {
                return Err(StorageError::Internal {
                    message: "get failure".to_string(),
                });
            }
            if self.fail_delete {
                let target_id = hex::decode("11".repeat(32)).expect("target id");
                if event_id == target_id.as_slice() {
                    return Ok(Some(delete_target_record()));
                }
                return Ok(None);
            }
            if let Some(record) = self
                .query_results
                .iter()
                .find(|record| record.id.as_slice() == event_id)
            {
                return Ok(Some(record.clone()));
            }
            Ok(None)
        }

        async fn delete_event(
            &self,
            _tenant_id: &str,
            _event_id: &[u8],
        ) -> Result<bool, StorageError> {
            if self.fail_delete {
                return Err(StorageError::Internal {
                    message: "delete failure".to_string(),
                });
            }
            Ok(false)
        }

        async fn query_events(
            &self,
            _query: &EventQuery,
        ) -> Result<Vec<EventRecord>, StorageError> {
            if self.fail_query {
                return Err(StorageError::Internal {
                    message: "query failure".to_string(),
                });
            }
            if self.fail_delete {
                return Ok(vec![delete_target_record()]);
            }
            Ok(self.query_results.clone())
        }
    }

    fn unreachable_postgres_repo() -> PostgresRepositories {
        let storage = StorageConfig {
            read_connection: "postgres://user:pass@127.0.0.1:1/gittree".to_string(),
            write_connection: None,
            max_connections: 1,
            min_connections: 0,
            idle_timeout_secs: None,
            max_lifetime_secs: None,
            application_name: Some("gittree-store-test".to_string()),
        };
        let pool_options = storage
            .pool_options()
            .expect("pool options")
            .acquire_timeout(Duration::from_secs(1));
        let connect_options = storage.read_connect_options().expect("connect options");
        PostgresRepositories::new(pool_options.connect_lazy_with(connect_options))
    }

    fn sample_event(id: &str) -> NostrEvent {
        NostrEvent {
            id: id.to_string(),
            pubkey: "aa".repeat(32),
            created_at: 1,
            kind: 1,
            tags: Vec::new(),
            content: String::new(),
            sig: "00".repeat(64),
        }
    }

    #[tokio::test]
    async fn insert_and_get_event() {
        let store = MemoryStore::new();
        let event = sample_event("abc");
        let outcome = store.insert(event.clone()).await.expect("insert");
        assert_eq!(outcome, StoreOutcome::Inserted);

        let fetched = store.get("abc").await.expect("get").expect("event");
        assert_eq!(fetched.id, event.id);
    }

    #[tokio::test]
    async fn insert_reports_duplicates() {
        let store = MemoryStore::new();
        store.insert(sample_event("dup")).await.expect("insert");
        let outcome = store.insert(sample_event("dup")).await.expect("insert");
        assert_eq!(outcome, StoreOutcome::Duplicate);
    }

    #[tokio::test]
    async fn delete_removes_event() {
        let store = MemoryStore::new();
        store.insert(sample_event("gone")).await.expect("insert");
        assert!(store.delete("gone").await.expect("delete"));
        assert!(store.get("gone").await.expect("get").is_none());
    }

    #[tokio::test]
    async fn delete_missing_event_returns_false() {
        let store = MemoryStore::new();
        assert!(!store.delete("missing").await.expect("delete"));
    }

    #[tokio::test]
    async fn query_orders_by_created_at_desc() {
        let store = MemoryStore::new();
        let mut event_a = sample_event("a");
        event_a.created_at = 10;
        let mut event_b = sample_event("b");
        event_b.created_at = 30;
        let mut event_c = sample_event("c");
        event_c.created_at = 20;

        store.insert(event_a).await.expect("insert");
        store.insert(event_b).await.expect("insert");
        store.insert(event_c).await.expect("insert");

        let filter = crate::Filter::from_json(&json!({})).expect("filter");
        let results = store.query(&[filter]).await.expect("query");
        let ids: Vec<String> = results.into_iter().map(|event| event.id).collect();
        assert_eq!(ids, vec!["b", "c", "a"]);
    }

    #[tokio::test]
    async fn query_applies_limit() {
        let store = MemoryStore::new();
        for id in ["a", "b", "c"] {
            let mut event = sample_event(id);
            event.created_at = 10;
            store.insert(event).await.expect("insert");
        }

        let filter = crate::Filter::from_json(&json!({"limit": 1})).expect("filter");
        let results = store.query(&[filter]).await.expect("query");
        assert_eq!(results.len(), 1);
    }

    #[tokio::test]
    async fn query_dedupes_across_filters() {
        let store = MemoryStore::new();
        let event = sample_event("dup");
        store.insert(event).await.expect("insert");

        let filter_a = crate::Filter::from_json(&json!({"ids": ["d"]})).expect("filter");
        let filter_b = crate::Filter::from_json(&json!({"authors": ["aa"]})).expect("filter");
        let results = store.query(&[filter_a, filter_b]).await.expect("query");
        assert_eq!(results.len(), 1);
    }

    #[tokio::test]
    async fn replaceable_events_replace_older_versions() {
        let store = MemoryStore::new();
        let mut older = sample_event("old");
        older.kind = 0;
        older.created_at = 10;
        older.pubkey = "aa".repeat(32);
        store.insert(older).await.expect("insert");

        let mut newer = sample_event("new");
        newer.kind = 0;
        newer.created_at = 20;
        newer.pubkey = "aa".repeat(32);
        let outcome = store.insert(newer.clone()).await.expect("insert");
        assert_eq!(outcome, StoreOutcome::Inserted);
        assert!(store.get("old").await.expect("get").is_none());
        assert!(store.get("new").await.expect("get").is_some());
    }

    #[tokio::test]
    async fn replaceable_events_reject_older_updates() {
        let store = MemoryStore::new();
        let mut newer = sample_event("new");
        newer.kind = 0;
        newer.created_at = 20;
        newer.pubkey = "aa".repeat(32);
        store.insert(newer).await.expect("insert");

        let mut older = sample_event("old");
        older.kind = 0;
        older.created_at = 10;
        older.pubkey = "aa".repeat(32);
        let outcome = store.insert(older).await.expect("insert");
        assert_eq!(outcome, StoreOutcome::Duplicate);
    }

    #[tokio::test]
    async fn delete_event_removes_matching_event() {
        let store = MemoryStore::new();
        let mut target = sample_event("target");
        target.pubkey = "aa".repeat(32);
        target.created_at = 5;
        store.insert(target).await.expect("insert");

        let mut delete = sample_event("delete");
        delete.kind = 5;
        delete.pubkey = "aa".repeat(32);
        delete.created_at = 10;
        delete.tags = vec![vec!["e".to_string(), "target".to_string()]];
        store.insert(delete.clone()).await.expect("insert");

        assert!(store.get("target").await.expect("get").is_none());
        assert!(store.get("delete").await.expect("get").is_some());
    }

    #[tokio::test]
    async fn memory_store_delete_by_address_respects_timestamp() {
        let store = MemoryStore::new();
        let pubkey = "aa".repeat(32);

        let mut target = sample_event("target-address");
        target.kind = 30023;
        target.pubkey = pubkey.clone();
        target.created_at = 10;
        target.tags = vec![vec!["d".to_string(), "demo".to_string()]];
        store.insert(target.clone()).await.expect("insert target");

        let mut older_delete = sample_event("older-delete");
        older_delete.kind = 5;
        older_delete.pubkey = pubkey.clone();
        older_delete.created_at = 5;
        older_delete.tags = vec![vec!["a".to_string(), format!("30023:{pubkey}:demo")]];
        store
            .insert(older_delete)
            .await
            .expect("insert older delete");
        assert!(store.get(&target.id).await.expect("get target").is_some());

        let mut newer_delete = sample_event("newer-delete");
        newer_delete.kind = 5;
        newer_delete.pubkey = pubkey.clone();
        newer_delete.created_at = 20;
        newer_delete.tags = vec![
            vec!["a".to_string(), "bad-address".to_string()],
            vec!["a".to_string(), format!("30023:{pubkey}:demo")],
        ];
        store
            .insert(newer_delete)
            .await
            .expect("insert newer delete");
        assert!(store.get(&target.id).await.expect("get target").is_none());
    }

    #[tokio::test]
    async fn memory_store_delete_ignores_mismatched_author() {
        let store = MemoryStore::new();
        let target_pubkey = "aa".repeat(32);

        let mut target = sample_event("target-delete-author");
        target.pubkey = target_pubkey.clone();
        target.created_at = 5;
        store.insert(target.clone()).await.expect("insert target");

        let mut delete = sample_event("delete-mismatch");
        delete.kind = 5;
        delete.pubkey = "bb".repeat(32);
        delete.created_at = 10;
        delete.tags = vec![vec!["e".to_string(), target.id.clone()]];
        store
            .insert(delete)
            .await
            .expect("insert mismatched delete");
        assert!(store.get(&target.id).await.expect("get target").is_some());

        let mut target_address = sample_event("target-address-mismatch");
        target_address.kind = 30023;
        target_address.pubkey = target_pubkey.clone();
        target_address.created_at = 5;
        target_address.tags = vec![vec!["d".to_string(), "demo".to_string()]];
        store
            .insert(target_address.clone())
            .await
            .expect("insert address target");

        let mut delete_address = sample_event("delete-address-mismatch");
        delete_address.kind = 5;
        delete_address.pubkey = "bb".repeat(32);
        delete_address.created_at = 20;
        delete_address.tags = vec![vec!["a".to_string(), format!("30023:{target_pubkey}:demo")]];
        store
            .insert(delete_address)
            .await
            .expect("insert address delete");
        assert!(
            store
                .get(&target_address.id)
                .await
                .expect("get address target")
                .is_some()
        );
    }

    #[tokio::test]
    async fn memory_store_delete_missing_targets_is_noop() {
        let store = MemoryStore::new();
        let mut delete = sample_event("delete-missing-targets");
        delete.kind = 5;
        delete.pubkey = "aa".repeat(32);
        delete.created_at = 10;
        delete.tags = vec![
            vec!["e".to_string(), "missing-event".to_string()],
            vec!["a".to_string(), "bad-address".to_string()],
            vec!["a".to_string(), format!("30023:{}:demo", delete.pubkey)],
        ];
        store.insert(delete.clone()).await.expect("insert delete");
        assert!(store.get(&delete.id).await.expect("get delete").is_some());
    }

    #[test]
    fn memory_store_state_apply_delete_skips_stale_replaceable_pointer() {
        let mut state = MemoryStoreState::default();
        let mut delete = sample_event("delete-stale-replaceable");
        delete.kind = 5;
        delete.pubkey = "aa".repeat(32);
        delete.created_at = 10;
        let address = format!("30023:{}:demo", delete.pubkey);
        let key = parse_address(&address).expect("address");
        state
            .replaceable
            .insert(key.clone(), "missing-id".to_string());
        delete.tags = vec![vec!["a".to_string(), address]];

        state.apply_delete(&delete);
        assert!(state.replaceable.contains_key(&key));
    }

    #[tokio::test]
    async fn memory_store_query_reports_invalid_tag_index() {
        let store = MemoryStore::new();
        let mut event = sample_event("bad-tags");
        event.tags = vec![Vec::new()];
        store.insert(event).await.expect("insert");

        let filter = crate::Filter::from_json(&json!({})).expect("filter");
        let err = store.query(&[filter]).await.expect_err("invalid tag index");
        assert!(matches!(err, StoreError::Backend(_)));
    }

    #[tokio::test]
    async fn repository_store_inserts_and_queries() {
        let repo = InMemoryRepositories::new();
        let store = RepositoryStore::new(repo);

        let event = sample_event(&"aa".repeat(32));
        let outcome = store.insert(event.clone()).await.expect("insert");
        assert_eq!(outcome, StoreOutcome::Inserted);

        let filter = crate::Filter::from_json(&json!({"ids": [event.id.clone()]})).expect("filter");
        let results = store.query(&[filter]).await.expect("query");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, event.id);
    }

    #[tokio::test]
    async fn repository_store_replaces_replaceable_events() {
        let repo = InMemoryRepositories::new();
        let store = RepositoryStore::new(repo);

        let mut older = sample_event(&"11".repeat(32));
        older.kind = 0;
        older.created_at = 10;
        older.pubkey = "aa".repeat(32);
        store.insert(older).await.expect("insert");

        let mut newer = sample_event(&"22".repeat(32));
        newer.kind = 0;
        newer.created_at = 20;
        newer.pubkey = "aa".repeat(32);
        let outcome = store.insert(newer.clone()).await.expect("insert");
        assert_eq!(outcome, StoreOutcome::Inserted);
        assert!(store.get(&"11".repeat(32)).await.expect("get").is_none());
        assert!(store.get(&"22".repeat(32)).await.expect("get").is_some());
    }

    #[tokio::test]
    async fn repository_store_applies_delete_events() {
        let repo = InMemoryRepositories::new();
        let store = RepositoryStore::new(repo);

        let mut target = sample_event(&"33".repeat(32));
        target.pubkey = "aa".repeat(32);
        target.created_at = 5;
        store.insert(target.clone()).await.expect("insert");

        let mut delete = sample_event(&"44".repeat(32));
        delete.kind = 5;
        delete.pubkey = "aa".repeat(32);
        delete.created_at = 10;
        delete.tags = vec![vec!["e".to_string(), target.id.clone()]];
        store.insert(delete.clone()).await.expect("insert");

        assert!(store.get(&target.id).await.expect("get").is_none());
        assert!(store.get(&delete.id).await.expect("get").is_some());
    }

    #[tokio::test]
    async fn repository_store_matches_prefix_filters() {
        let repo = InMemoryRepositories::new();
        let store = RepositoryStore::new(repo);

        let event = sample_event(&"aa".repeat(32));
        store.insert(event.clone()).await.expect("insert");

        let filter = crate::Filter::from_json(&json!({
            "ids": ["aa"],
            "authors": ["aa"]
        }))
        .expect("filter");
        let results = store.query(&[filter]).await.expect("query");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, event.id);
    }

    #[tokio::test]
    async fn repository_store_with_tenant_and_arc_eventstore_paths_work() {
        let repo = InMemoryRepositories::new();
        let store = Arc::new(RepositoryStore::with_tenant(repo, "tenant-x"));
        let event = sample_event(&"bb".repeat(32));
        assert_eq!(
            EventStore::insert(&store, event.clone())
                .await
                .expect("insert"),
            StoreOutcome::Inserted
        );
        assert!(
            EventStore::get(&store, &event.id)
                .await
                .expect("get")
                .is_some()
        );
        assert!(EventStore::delete(&store, &event.id).await.expect("delete"));
        assert!(
            EventStore::get(&store, &event.id)
                .await
                .expect("get")
                .is_none()
        );
    }

    #[tokio::test]
    async fn repository_store_ignores_invalid_hex_filters() {
        let repo = InMemoryRepositories::new();
        let store = RepositoryStore::new(repo);

        let event = sample_event(&"aa".repeat(32));
        store.insert(event).await.expect("insert");

        let filter = crate::Filter::from_json(&json!({"ids": ["zz"]})).expect("filter");
        let results = store.query(&[filter]).await.expect("query");
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn repository_store_sorts_results_and_handles_tag_filters() {
        let repo = InMemoryRepositories::new();
        let store = RepositoryStore::new(repo);

        let mut older = sample_event(&"51".repeat(32));
        older.created_at = 10;
        older.tags = vec![vec!["e".to_string(), "topic-1".to_string()]];
        store.insert(older.clone()).await.expect("insert older");

        let mut newer = sample_event(&"52".repeat(32));
        newer.created_at = 20;
        newer.tags = vec![vec!["e".to_string(), "topic-1".to_string()]];
        store.insert(newer.clone()).await.expect("insert newer");

        let filter_old =
            crate::Filter::from_json(&json!({"ids": [older.id.clone()]})).expect("old filter");
        let filter_new =
            crate::Filter::from_json(&json!({"ids": [newer.id.clone()]})).expect("new filter");
        let results = store
            .query(&[filter_old, filter_new])
            .await
            .expect("query sorted");
        let ids: Vec<String> = results.into_iter().map(|event| event.id).collect();
        assert_eq!(ids, vec![newer.id.clone(), older.id.clone()]);

        let tag_filter = crate::Filter::from_json(&json!({"#e": ["topic-1"]})).expect("tag filter");
        let tagged = store.query(&[tag_filter]).await.expect("query tagged");
        assert_eq!(tagged.len(), 2);
    }

    #[tokio::test]
    async fn memory_store_query_empty_filters_returns_empty() {
        let store = MemoryStore::new();
        let results = store.query(&[]).await.expect("query");
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn repository_store_query_empty_filters_returns_empty() {
        let repo = InMemoryRepositories::new();
        let store = RepositoryStore::new(repo);
        let results = store.query(&[]).await.expect("query");
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn repository_store_get_and_delete_reject_invalid_hex_ids() {
        let repo = InMemoryRepositories::new();
        let store = RepositoryStore::new(repo);

        let get_err = store.get("not-hex").await.expect_err("invalid get id");
        assert!(matches!(get_err, StoreError::Backend(_)));

        let delete_err = store
            .delete("still-not-hex")
            .await
            .expect_err("invalid delete id");
        assert!(matches!(delete_err, StoreError::Backend(_)));
    }

    #[tokio::test]
    async fn repository_store_replaces_parameterized_replaceable_events() {
        let repo = InMemoryRepositories::new();
        let store = RepositoryStore::new(repo);

        let pubkey = "aa".repeat(32);
        let mut older = sample_event(&"31".repeat(32));
        older.kind = 30023;
        older.created_at = 10;
        older.pubkey = pubkey.clone();
        older.tags = vec![vec!["d".to_string(), "demo".to_string()]];
        store.insert(older.clone()).await.expect("insert older");

        let mut newer = sample_event(&"32".repeat(32));
        newer.kind = 30023;
        newer.created_at = 20;
        newer.pubkey = pubkey;
        newer.tags = vec![vec!["d".to_string(), "demo".to_string()]];
        let outcome = store.insert(newer.clone()).await.expect("insert newer");
        assert_eq!(outcome, StoreOutcome::Inserted);

        assert!(store.get(&older.id).await.expect("get older").is_none());
        assert!(store.get(&newer.id).await.expect("get newer").is_some());
    }

    #[tokio::test]
    async fn repository_store_delete_event_removes_parameterized_by_address() {
        let repo = InMemoryRepositories::new();
        let store = RepositoryStore::new(repo);

        let pubkey = "aa".repeat(32);
        let mut target = sample_event(&"41".repeat(32));
        target.kind = 30023;
        target.created_at = 5;
        target.pubkey = pubkey.clone();
        target.tags = vec![vec!["d".to_string(), "demo".to_string()]];
        store.insert(target.clone()).await.expect("insert target");

        let mut delete = sample_event(&"42".repeat(32));
        delete.kind = 5;
        delete.created_at = 15;
        delete.pubkey = pubkey.clone();
        delete.tags = vec![vec!["a".to_string(), format!("30023:{pubkey}:demo")]];
        store.insert(delete.clone()).await.expect("insert delete");

        assert!(store.get(&target.id).await.expect("get target").is_none());
        assert!(store.get(&delete.id).await.expect("get delete").is_some());
    }

    #[tokio::test]
    async fn repository_store_delete_by_address_requires_matching_pubkey() {
        let repo = InMemoryRepositories::new();
        let store = RepositoryStore::new(repo);

        let target_pubkey = "aa".repeat(32);
        let mut target = sample_event(&"61".repeat(32));
        target.kind = 30023;
        target.created_at = 5;
        target.pubkey = target_pubkey.clone();
        target.tags = vec![vec!["d".to_string(), "demo".to_string()]];
        store.insert(target.clone()).await.expect("insert target");

        let mut delete = sample_event(&"62".repeat(32));
        delete.kind = 5;
        delete.created_at = 15;
        delete.pubkey = "bb".repeat(32);
        delete.tags = vec![vec!["a".to_string(), format!("30023:{target_pubkey}:demo")]];
        store.insert(delete).await.expect("insert delete");

        assert!(store.get(&target.id).await.expect("get target").is_some());
    }

    #[tokio::test]
    async fn repository_store_delete_event_rejects_invalid_pubkey_hex() {
        let repo = InMemoryRepositories::new();
        let store = RepositoryStore::new(repo);

        let mut delete = sample_event(&"71".repeat(32));
        delete.kind = 5;
        delete.pubkey = "not-hex".to_string();
        delete.tags = vec![vec!["e".to_string(), "11".repeat(32)]];

        let err = store
            .insert(delete)
            .await
            .expect_err("invalid delete pubkey should fail");
        assert!(matches!(err, StoreError::Backend(_)));
    }

    #[tokio::test]
    async fn repository_store_propagates_insert_and_query_repository_errors() {
        let insert_store = RepositoryStore::new(ScriptedEventRepo::insert_error());
        let insert_err = insert_store
            .insert(sample_event(&"72".repeat(32)))
            .await
            .expect_err("insert should fail");
        assert!(matches!(insert_err, StoreError::Backend(_)));

        let query_store = RepositoryStore::new(ScriptedEventRepo::query_error());
        let filter = crate::Filter::from_json(&json!({})).expect("filter");
        let query_err = query_store
            .query(&[filter])
            .await
            .expect_err("query should fail");
        assert!(matches!(query_err, StoreError::Backend(_)));
    }

    #[tokio::test]
    async fn repository_store_scripted_query_post_filter_and_sort_paths() {
        let mut older = sample_event(&"a1".repeat(32));
        older.created_at = 10;
        let mut newer = sample_event(&"a2".repeat(32));
        newer.created_at = 10;
        let records = vec![
            event_to_record(&older, "default").expect("older record"),
            event_to_record(&newer, "default").expect("newer record"),
        ];
        let store = RepositoryStore::new(ScriptedEventRepo::with_query_results(records));
        let filter = crate::Filter::from_json(&json!({"ids": ["a"]})).expect("filter");
        let results = store.query(&[filter]).await.expect("query");
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].id, older.id);
        assert_eq!(results[1].id, newer.id);
    }

    #[tokio::test]
    async fn repository_store_scripted_query_post_filter_maps_tag_index_errors() {
        let event = sample_event(&"af".repeat(32));
        let mut record = event_to_record(&event, "default").expect("record");
        record.tags = vec![TagRecord::new("", "broken")];
        let store = RepositoryStore::new(ScriptedEventRepo::with_query_results(vec![record]));
        let filter = crate::Filter::from_json(&json!({"ids": ["a"]})).expect("filter");
        let err = store.query(&[filter]).await.expect_err("invalid tag index");
        assert!(matches!(err, StoreError::Backend(_)));
    }

    #[tokio::test]
    async fn repository_store_delete_event_propagates_lookup_and_address_query_errors() {
        let lookup_store = RepositoryStore::new(ScriptedEventRepo::get_error());
        let mut delete_lookup = sample_event(&"73".repeat(32));
        delete_lookup.kind = 5;
        delete_lookup.tags = vec![vec!["e".to_string(), "11".repeat(32)]];
        let lookup_err = lookup_store
            .insert(delete_lookup)
            .await
            .expect_err("delete lookup should fail");
        assert!(matches!(lookup_err, StoreError::Backend(_)));

        let query_store = RepositoryStore::new(ScriptedEventRepo::query_error());
        let author = "aa".repeat(32);
        let mut delete_address = sample_event(&"74".repeat(32));
        delete_address.kind = 5;
        delete_address.pubkey = author.clone();
        delete_address.tags = vec![vec!["a".to_string(), format!("30023:{author}:demo")]];
        let query_err = query_store
            .insert(delete_address)
            .await
            .expect_err("delete address query should fail");
        assert!(matches!(query_err, StoreError::Backend(_)));
    }

    #[tokio::test]
    async fn repository_store_delete_event_propagates_delete_repository_errors() {
        let delete_lookup_store = RepositoryStore::new(ScriptedEventRepo::delete_error());
        let mut delete_lookup = sample_event(&"75".repeat(32));
        delete_lookup.kind = 5;
        delete_lookup.tags = vec![vec!["e".to_string(), "11".repeat(32)]];
        let lookup_err = delete_lookup_store
            .insert(delete_lookup)
            .await
            .expect_err("delete lookup should fail");
        assert!(matches!(lookup_err, StoreError::Backend(_)));

        let delete_address_store = RepositoryStore::new(ScriptedEventRepo::delete_error());
        let mut delete_address = sample_event(&"76".repeat(32));
        delete_address.kind = 5;
        delete_address.tags = vec![vec![
            "a".to_string(),
            format!("30023:{}:demo", delete_address.pubkey),
        ]];
        let query_err = delete_address_store
            .insert(delete_address)
            .await
            .expect_err("delete address should fail");
        assert!(matches!(query_err, StoreError::Backend(_)));
    }

    #[tokio::test]
    async fn repository_store_postgres_eventstore_paths_surface_backend_errors() {
        let store = RepositoryStore::new(unreachable_postgres_repo());
        let insert_err = store
            .insert(sample_event("not-hex-id"))
            .await
            .expect_err("insert should fail before backend call");
        assert!(matches!(insert_err, StoreError::Backend(_)));

        let id = "not-hex-id";
        let get_err = store.get(&id).await.expect_err("postgres get should fail");
        assert!(matches!(get_err, StoreError::Backend(_)));

        let delete_err = store
            .delete(&id)
            .await
            .expect_err("postgres delete should fail");
        assert!(matches!(delete_err, StoreError::Backend(_)));

        let query = store.query(&[]).await.expect("empty query");
        assert!(query.is_empty());

        let dyn_store: Arc<dyn EventStore> =
            Arc::new(RepositoryStore::new(unreachable_postgres_repo()));
        let dyn_delete_err = EventStore::delete(&dyn_store, "still-not-hex")
            .await
            .expect_err("dyn delete should fail");
        assert!(matches!(dyn_delete_err, StoreError::Backend(_)));
    }

    #[tokio::test]
    async fn repository_store_postgres_helper_generics_are_exercised() {
        let repo = unreachable_postgres_repo();

        let mut delete = sample_event(&"84".repeat(32));
        delete.kind = 5;
        delete.pubkey = "not-hex".to_string();
        delete.tags = vec![vec!["e".to_string(), "11".repeat(32)]];
        let delete_err = apply_delete_repo(&repo, "default", &delete)
            .await
            .expect_err("invalid pubkey should fail");
        assert!(matches!(delete_err, StoreError::Backend(_)));

        let mut replaceable = sample_event(&"85".repeat(32));
        replaceable.kind = 0;
        replaceable.pubkey = "aa".repeat(32);
        let key = replaceable_key(&replaceable).expect("replaceable key");
        let replaceable_err = apply_replaceable_repo(&repo, "default", &replaceable, &key)
            .await
            .expect_err("postgres replaceable query should fail");
        assert!(matches!(replaceable_err, StoreError::Backend(_)));
    }

    #[tokio::test]
    async fn apply_delete_repo_skips_invalid_targets_and_mismatched_records() {
        let repo = ScriptedEventRepo::delete_error();
        let mut delete = sample_event(&"86".repeat(32));
        delete.kind = 5;
        delete.pubkey = "bb".repeat(32);
        delete.created_at = -1;
        delete.tags = vec![
            vec!["e".to_string(), "zz-not-hex".to_string()],
            vec!["e".to_string(), "11".repeat(32)],
            vec!["a".to_string(), "bad-address".to_string()],
            vec!["a".to_string(), format!("30023:{}:demo", delete.pubkey)],
        ];

        apply_delete_repo(&repo, "default", &delete)
            .await
            .expect("invalid and mismatched targets should be skipped");
    }

    #[tokio::test]
    async fn apply_delete_repo_deletes_matching_e_target() {
        let repo = ScriptedEventRepo::with_query_results(vec![delete_target_record()]);
        let mut delete = sample_event(&"88".repeat(32));
        delete.kind = 5;
        delete.pubkey = "aa".repeat(32);
        delete.created_at = 5;
        delete.tags = vec![vec!["e".to_string(), "11".repeat(32)]];

        apply_delete_repo(&repo, "default", &delete)
            .await
            .expect("matching target should delete");
    }

    #[tokio::test]
    async fn apply_delete_repo_skips_missing_e_target() {
        let repo = ScriptedEventRepo::with_query_results(Vec::new());
        let mut delete = sample_event(&"89".repeat(32));
        delete.kind = 5;
        delete.pubkey = "aa".repeat(32);
        delete.created_at = 5;
        delete.tags = vec![vec!["e".to_string(), "11".repeat(32)]];

        apply_delete_repo(&repo, "default", &delete)
            .await
            .expect("missing target should be skipped");
    }

    #[tokio::test]
    async fn apply_replaceable_repo_returns_duplicate_when_existing_is_newer() {
        let mut existing = sample_event(&"87".repeat(32));
        existing.kind = 0;
        existing.pubkey = "aa".repeat(32);
        existing.created_at = 10;
        let record = event_to_record(&existing, "default").expect("existing record");
        let repo = ScriptedEventRepo::with_query_results(vec![record]);

        let mut incoming = sample_event(&"88".repeat(32));
        incoming.kind = 0;
        incoming.pubkey = "aa".repeat(32);
        incoming.created_at = 5;
        let key = replaceable_key(&incoming).expect("replaceable key");
        let duplicate = apply_replaceable_repo(&repo, "default", &incoming, &key)
            .await
            .expect("apply");
        assert!(duplicate);
    }

    #[test]
    fn exact_hex_filters_and_helpers_cover_edge_cases() {
        let exact = vec!["11".repeat(32), "22".repeat(32)];
        let (values, needs_post) = exact_hex_filters(&exact);
        assert_eq!(values, exact);
        assert!(!needs_post);

        let mixed = vec!["11".repeat(31), "prefix".to_string()];
        let (values, needs_post) = exact_hex_filters(&mixed);
        assert!(values.is_empty());
        assert!(needs_post);

        let tags = vec![
            vec!["e".to_string(), "a".to_string(), "b".to_string()],
            vec!["p".to_string(), "x".to_string()],
        ];
        assert_eq!(
            collect_tag_values(&tags, "e"),
            vec!["a".to_string(), "b".to_string()]
        );
        assert_eq!(collect_tag_values(&tags, "z"), Vec::<String>::new());

        assert!(parse_address("bad").is_none());
        assert!(parse_address("not-a-kind:pubkey:demo").is_none());
        let parsed = parse_address("30023:pubkey:demo").expect("address");
        assert_eq!(parsed.kind, 30023);
        assert_eq!(parsed.pubkey, "pubkey");
        assert_eq!(parsed.identifier.as_deref(), Some("demo"));
    }
}
