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
            if let Some(existing_id) = self.replaceable.get(&key).cloned() {
                if let Some(existing) = self.events.get(&existing_id) {
                    if existing.pubkey == event.pubkey && existing.created_at <= event.created_at {
                        self.remove_event(&existing_id);
                    }
                }
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
}

impl<R: EventRepository> RepositoryStore<R> {
    pub fn new(repo: R) -> Self {
        Self { repo }
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
        let record = event_to_record(&event)?;
        if self
            .repo
            .get_event(&record.id)
            .await
            .map_err(map_repo_err)?
            .is_some()
        {
            return Ok(StoreOutcome::Duplicate);
        }

        if event.kind == 5 {
            apply_delete_repo(&self.repo, &event).await?;
        }

        if let Some(key) = replaceable_key(&event) {
            if apply_replaceable_repo(&self.repo, &event, &key).await? {
                return Ok(StoreOutcome::Duplicate);
            }
        }

        self.repo
            .insert_event(record)
            .await
            .map_err(map_repo_err)?;
        Ok(StoreOutcome::Inserted)
    }

    async fn get(&self, id: &str) -> Result<Option<NostrEvent>, StoreError> {
        let bytes = decode_hex_id(id)?;
        let record = self
            .repo
            .get_event(&bytes)
            .await
            .map_err(map_repo_err)?;
        record.map(record_to_event).transpose()
    }

    async fn delete(&self, id: &str) -> Result<bool, StoreError> {
        let bytes = decode_hex_id(id)?;
        self.repo
            .delete_event(&bytes)
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
            let records = self
                .repo
                .query_events(&plan.query)
                .await
                .map_err(map_repo_err)?;
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
    event: &NostrEvent,
) -> Result<(), StoreError> {
    let author = decode_hex_pubkey(&event.pubkey)?;

    for target in collect_tag_values(&event.tags, "e") {
        let Ok(bytes) = hex::decode(&target) else {
            continue;
        };
        let record = repo
            .get_event(&bytes)
            .await
            .map_err(map_repo_err)?;
        if let Some(record) = record {
            if record.pubkey == author && record.created_at <= event.created_at {
                repo.delete_event(&record.id)
                    .await
                    .map_err(map_repo_err)?;
            }
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
        query.kinds = vec![key.kind];
        query.authors = vec![key.pubkey.clone()];
        if let Some(identifier) = key.identifier {
            query.tags = vec![TagRecord::new("d", identifier)];
        }
        let records = repo.query_events(&query).await.map_err(map_repo_err)?;
        for record in records {
            if record.created_at <= event.created_at {
                repo.delete_event(&record.id)
                    .await
                    .map_err(map_repo_err)?;
            }
        }
    }

    Ok(())
}

async fn apply_replaceable_repo<R: EventRepository>(
    repo: &R,
    event: &NostrEvent,
    key: &ReplaceableKey,
) -> Result<bool, StoreError> {
    let mut query = EventQuery::default();
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
        repo.delete_event(&record.id)
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
        let identifier = collect_tag_values(&event.tags, "d")
            .into_iter()
            .next()?;
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

fn event_to_record(event: &NostrEvent) -> Result<EventRecord, StoreError> {
    EventRecord::new(
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
            ids,
            authors,
            kinds: filter.kinds.clone(),
            since: filter.since,
            until: filter.until,
            tags,
            limit: if needs_post_filter { None } else { filter.limit },
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
    use super::{EventStore, MemoryStore, RepositoryStore, StoreOutcome};
    use crate::NostrEvent;
    use gittree_storage::InMemoryRepositories;
    use serde_json::json;

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
    async fn repository_store_inserts_and_queries() {
        let repo = InMemoryRepositories::new();
        let store = RepositoryStore::new(repo);

        let event = sample_event(&"aa".repeat(32));
        let outcome = store.insert(event.clone()).await.expect("insert");
        assert_eq!(outcome, StoreOutcome::Inserted);

        let filter =
            crate::Filter::from_json(&json!({"ids": [event.id.clone()]})).expect("filter");
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
    async fn repository_store_ignores_invalid_hex_filters() {
        let repo = InMemoryRepositories::new();
        let store = RepositoryStore::new(repo);

        let event = sample_event(&"aa".repeat(32));
        store.insert(event).await.expect("insert");

        let filter = crate::Filter::from_json(&json!({"ids": ["zz"]})).expect("filter");
        let results = store.query(&[filter]).await.expect("query");
        assert!(results.is_empty());
    }
}
