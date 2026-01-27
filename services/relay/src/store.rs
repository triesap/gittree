use crate::{Filter, NostrEvent, TagIndex};
use std::collections::{BTreeMap, HashSet};

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

pub trait EventStore {
    fn insert(&mut self, event: NostrEvent) -> Result<StoreOutcome, StoreError>;
    fn get(&self, id: &str) -> Result<Option<NostrEvent>, StoreError>;
    fn delete(&mut self, id: &str) -> Result<bool, StoreError>;
    fn query(&self, filters: &[Filter]) -> Result<Vec<NostrEvent>, StoreError>;
}

#[derive(Debug, Default)]
pub struct MemoryStore {
    events: BTreeMap<String, NostrEvent>,
}

impl MemoryStore {
    pub fn new() -> Self {
        Self::default()
    }
}

impl EventStore for MemoryStore {
    fn insert(&mut self, event: NostrEvent) -> Result<StoreOutcome, StoreError> {
        if self.events.contains_key(&event.id) {
            return Ok(StoreOutcome::Duplicate);
        }
        self.events.insert(event.id.clone(), event);
        Ok(StoreOutcome::Inserted)
    }

    fn get(&self, id: &str) -> Result<Option<NostrEvent>, StoreError> {
        Ok(self.events.get(id).cloned())
    }

    fn delete(&mut self, id: &str) -> Result<bool, StoreError> {
        Ok(self.events.remove(id).is_some())
    }

    fn query(&self, filters: &[Filter]) -> Result<Vec<NostrEvent>, StoreError> {
        if filters.is_empty() {
            return Ok(Vec::new());
        }

        let mut ordered: Vec<&NostrEvent> = self.events.values().collect();
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

    #[test]
    fn insert_and_get_event() {
        let mut store = MemoryStore::new();
        let event = sample_event("abc");
        let outcome = store.insert(event.clone()).expect("insert");
        assert_eq!(outcome, StoreOutcome::Inserted);

        let fetched = store.get("abc").expect("get").expect("event");
        assert_eq!(fetched.id, event.id);
    }

    #[test]
    fn insert_reports_duplicates() {
        let mut store = MemoryStore::new();
        store.insert(sample_event("dup")).expect("insert");
        let outcome = store.insert(sample_event("dup")).expect("insert");
        assert_eq!(outcome, StoreOutcome::Duplicate);
    }

    #[test]
    fn delete_removes_event() {
        let mut store = MemoryStore::new();
        store.insert(sample_event("gone")).expect("insert");
        assert!(store.delete("gone").expect("delete"));
        assert!(store.get("gone").expect("get").is_none());
    }

    #[test]
    fn query_orders_by_created_at_desc() {
        let mut store = MemoryStore::new();
        let mut event_a = sample_event("a");
        event_a.created_at = 10;
        let mut event_b = sample_event("b");
        event_b.created_at = 30;
        let mut event_c = sample_event("c");
        event_c.created_at = 20;

        store.insert(event_a).expect("insert");
        store.insert(event_b).expect("insert");
        store.insert(event_c).expect("insert");

        let filter = crate::Filter::from_json(&json!({})).expect("filter");
        let results = store.query(&[filter]).expect("query");
        let ids: Vec<String> = results.into_iter().map(|event| event.id).collect();
        assert_eq!(ids, vec!["b", "c", "a"]);
    }

    #[test]
    fn query_applies_limit() {
        let mut store = MemoryStore::new();
        for id in ["a", "b", "c"] {
            let mut event = sample_event(id);
            event.created_at = 10;
            store.insert(event).expect("insert");
        }

        let filter = crate::Filter::from_json(&json!({"limit": 1})).expect("filter");
        let results = store.query(&[filter]).expect("query");
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn query_dedupes_across_filters() {
        let mut store = MemoryStore::new();
        let event = sample_event("dup");
        store.insert(event).expect("insert");

        let filter_a = crate::Filter::from_json(&json!({"ids": ["d"]})).expect("filter");
        let filter_b = crate::Filter::from_json(&json!({"authors": ["aa"]})).expect("filter");
        let results = store.query(&[filter_a, filter_b]).expect("query");
        assert_eq!(results.len(), 1);
    }
}
