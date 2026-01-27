use crate::{Filter, NostrEvent, TagIndex};
use async_trait::async_trait;
use std::collections::{BTreeMap, HashSet};
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

#[cfg(test)]
mod tests {
    use super::{EventStore, MemoryStore, StoreOutcome};
    use crate::NostrEvent;
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
}
