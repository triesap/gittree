use crate::NostrEvent;
use std::collections::BTreeMap;

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
}

#[cfg(test)]
mod tests {
    use super::{EventStore, MemoryStore, StoreOutcome};
    use crate::NostrEvent;

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
}
