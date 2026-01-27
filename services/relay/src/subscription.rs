use crate::Filter;
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SubscriptionId(String);

impl SubscriptionId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SubscriptionState {
    eose_sent: bool,
    filters: Vec<Filter>,
}

#[derive(Debug, Default)]
pub struct SubscriptionRegistry {
    subscriptions: BTreeMap<SubscriptionId, SubscriptionState>,
}

impl SubscriptionRegistry {
    pub fn insert(&mut self, id: SubscriptionId, filters: Vec<Filter>) -> bool {
        self.subscriptions
            .insert(
                id,
                SubscriptionState {
                    eose_sent: false,
                    filters,
                },
            )
            .is_none()
    }

    pub fn remove(&mut self, id: &SubscriptionId) -> bool {
        self.subscriptions.remove(id).is_some()
    }

    pub fn contains(&self, id: &SubscriptionId) -> bool {
        self.subscriptions.contains_key(id)
    }

    pub fn len(&self) -> usize {
        self.subscriptions.len()
    }

    pub fn mark_eose(&mut self, id: &SubscriptionId) -> bool {
        let Some(state) = self.subscriptions.get_mut(id) else {
            return false;
        };
        state.eose_sent = true;
        true
    }

    pub fn eose_sent(&self, id: &SubscriptionId) -> bool {
        self.subscriptions
            .get(id)
            .map(|state| state.eose_sent)
            .unwrap_or(false)
    }

    pub fn filters(&self, id: &SubscriptionId) -> Option<&[Filter]> {
        self.subscriptions
            .get(id)
            .map(|state| state.filters.as_slice())
    }

    pub fn iter(&self) -> impl Iterator<Item = (&SubscriptionId, &[Filter])> {
        self.subscriptions
            .iter()
            .map(|(id, state)| (id, state.filters.as_slice()))
    }
}

#[cfg(test)]
mod tests {
    use super::{SubscriptionId, SubscriptionRegistry};
    use crate::Filter;

    #[test]
    fn insert_and_remove_subscriptions() {
        let mut registry = SubscriptionRegistry::default();
        let id = SubscriptionId::new("sub");

        assert!(registry.insert(id.clone(), Vec::new()));
        assert!(registry.contains(&id));
        assert!(registry.remove(&id));
        assert!(!registry.contains(&id));
    }

    #[test]
    fn mark_eose_tracks_state() {
        let mut registry = SubscriptionRegistry::default();
        let id = SubscriptionId::new("sub");

        assert!(!registry.eose_sent(&id));
        assert!(!registry.mark_eose(&id));

        registry.insert(id.clone(), Vec::new());
        assert!(registry.mark_eose(&id));
        assert!(registry.eose_sent(&id));
    }

    #[test]
    fn insert_reports_existing_entries() {
        let mut registry = SubscriptionRegistry::default();
        let id = SubscriptionId::new("dup");
        assert!(registry.insert(id.clone(), Vec::new()));
        assert!(!registry.insert(id, Vec::new()));
    }

    #[test]
    fn registry_returns_filters() {
        let mut registry = SubscriptionRegistry::default();
        let id = SubscriptionId::new("sub");
        let filter = Filter::from_json(&serde_json::json!({})).expect("filter");
        registry.insert(id.clone(), vec![filter.clone()]);
        let stored = registry.filters(&id).expect("filters");
        assert_eq!(stored, &[filter]);
    }
}
