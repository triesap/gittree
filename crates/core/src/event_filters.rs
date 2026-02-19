use crate::event_refs::collect_event_references_with_self;
use crate::kinds::{KIND_GIT_REPO_ANNOUNCEMENT, KIND_GIT_REPO_STATE};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventFilter {
    pub ids: Vec<String>,
    pub kinds: Vec<u32>,
    pub authors: Vec<String>,
    pub tags: BTreeMap<String, Vec<String>>,
    pub limit: Option<u64>,
}

impl EventFilter {
    pub fn new() -> Self {
        Self {
            ids: Vec::new(),
            kinds: Vec::new(),
            authors: Vec::new(),
            tags: BTreeMap::new(),
            limit: None,
        }
    }

    pub fn with_limit(mut self, limit: u64) -> Self {
        self.limit = Some(limit);
        self
    }
}

pub fn build_related_event_filters(
    kind: u32,
    pubkey: &str,
    event_id: &str,
    tags: &[Vec<String>],
) -> Vec<EventFilter> {
    let refs = collect_event_references_with_self(kind, pubkey, event_id, tags);
    let mut filters = Vec::new();

    if !refs.event_ids.is_empty() {
        let mut filter = EventFilter::new();
        filter.ids = refs.event_ids.clone();
        filters.push(filter.with_limit(1));
    }

    for pointer in &refs.address_pointers {
        if let Some(filter) = build_pointer_filter(pointer) {
            filters.push(filter.with_limit(1));
        }
    }

    if !refs.event_ids.is_empty() {
        let mut filter = EventFilter::new();
        filter.tags.insert("e".to_string(), refs.event_ids.clone());
        filter
            .tags
            .entry("E".to_string())
            .or_insert_with(Vec::new)
            .extend(refs.event_ids.clone());
        filters.push(filter.with_limit(1));
    }

    if !refs.address_pointers.is_empty() {
        let mut filter = EventFilter::new();
        filter
            .tags
            .insert("a".to_string(), refs.address_pointers.clone());
        filter
            .tags
            .entry("A".to_string())
            .or_insert_with(Vec::new)
            .extend(refs.address_pointers.clone());
        filters.push(filter.with_limit(1));
    }

    let mut filter = EventFilter::new();
    let mut q = refs.address_pointers.clone();
    q.extend(refs.event_ids.clone());
    filter.tags.insert("q".to_string(), q);
    filters.push(filter.with_limit(1));

    filters
}

fn build_pointer_filter(pointer: &str) -> Option<EventFilter> {
    let parts: Vec<&str> = pointer.split(':').collect();
    match parts.as_slice() {
        [kind, author, identifier] => {
            let kind = kind.parse::<u32>().ok()?;
            let mut filter = EventFilter::new();
            filter.kinds = related_kinds(kind);
            filter.authors = vec![author.to_string()];
            filter
                .tags
                .insert("d".to_string(), vec![identifier.to_string()]);
            Some(filter)
        }
        [kind, author] => {
            let kind = kind.parse::<u32>().ok()?;
            let mut filter = EventFilter::new();
            filter.kinds = related_kinds(kind);
            filter.authors = vec![author.to_string()];
            Some(filter)
        }
        _ => None,
    }
}

fn related_kinds(kind: u32) -> Vec<u32> {
    if kind == KIND_GIT_REPO_STATE.0 {
        vec![KIND_GIT_REPO_STATE.0, KIND_GIT_REPO_ANNOUNCEMENT.0]
    } else {
        vec![kind]
    }
}

#[cfg(test)]
mod tests {
    use super::EventFilter;
    use super::build_related_event_filters;
    use crate::kinds::{KIND_GIT_REPO_ANNOUNCEMENT, KIND_GIT_REPO_STATE};
    use std::collections::BTreeMap;

    #[test]
    fn builds_filters_for_addressable_event() {
        let tags = vec![vec!["d".to_string(), "repo".to_string()]];
        let filters = build_related_event_filters(30617, "pubkey", "eventid", &tags);

        let mut expected = Vec::new();

        let mut pointer_filter = EventFilter::new();
        pointer_filter.kinds = vec![30617];
        pointer_filter.authors = vec!["pubkey".to_string()];
        pointer_filter
            .tags
            .insert("d".to_string(), vec!["repo".to_string()]);
        expected.push(pointer_filter.with_limit(1));

        let mut address_filter = EventFilter::new();
        address_filter
            .tags
            .insert("a".to_string(), vec!["30617:pubkey:repo".to_string()]);
        address_filter
            .tags
            .insert("A".to_string(), vec!["30617:pubkey:repo".to_string()]);
        expected.push(address_filter.with_limit(1));

        let mut q_filter = EventFilter::new();
        q_filter
            .tags
            .insert("q".to_string(), vec!["30617:pubkey:repo".to_string()]);
        expected.push(q_filter.with_limit(1));

        assert_eq!(filters, expected);
    }

    #[test]
    fn builds_filters_for_non_addressable_event() {
        let tags = vec![
            vec!["e".to_string(), "aaaa".to_string()],
            vec!["a".to_string(), "30617:other:repo".to_string()],
        ];
        let filters = build_related_event_filters(1617, "pubkey", "eventid", &tags);

        let mut expected = Vec::new();

        let mut ids_filter = EventFilter::new();
        ids_filter.ids = vec!["aaaa".to_string(), "eventid".to_string()];
        expected.push(ids_filter.with_limit(1));

        let mut pointer_filter = EventFilter::new();
        pointer_filter.kinds = vec![30617];
        pointer_filter.authors = vec!["other".to_string()];
        pointer_filter
            .tags
            .insert("d".to_string(), vec!["repo".to_string()]);
        expected.push(pointer_filter.with_limit(1));

        let mut ref_filter = EventFilter::new();
        ref_filter.tags.insert(
            "e".to_string(),
            vec!["aaaa".to_string(), "eventid".to_string()],
        );
        ref_filter.tags.insert(
            "E".to_string(),
            vec!["aaaa".to_string(), "eventid".to_string()],
        );
        expected.push(ref_filter.with_limit(1));

        let mut address_filter = EventFilter::new();
        address_filter
            .tags
            .insert("a".to_string(), vec!["30617:other:repo".to_string()]);
        address_filter
            .tags
            .insert("A".to_string(), vec!["30617:other:repo".to_string()]);
        expected.push(address_filter.with_limit(1));

        let mut q_filter = EventFilter::new();
        q_filter.tags.insert(
            "q".to_string(),
            vec![
                "30617:other:repo".to_string(),
                "aaaa".to_string(),
                "eventid".to_string(),
            ],
        );
        expected.push(q_filter.with_limit(1));

        assert_eq!(filters, expected);
    }

    #[test]
    fn repo_state_pointer_includes_announcement_kind() {
        let pointer = format!("{}:pubkey:repo", KIND_GIT_REPO_STATE.0);
        let tags = vec![vec!["a".to_string(), pointer]];
        let filters = build_related_event_filters(KIND_GIT_REPO_STATE.0, "pubkey", "event", &tags);

        let pointer_filter = &filters[0];
        assert_eq!(
            pointer_filter.kinds,
            vec![KIND_GIT_REPO_STATE.0, KIND_GIT_REPO_ANNOUNCEMENT.0]
        );
    }

    #[test]
    fn empty_tags_produce_minimal_filters() {
        let filters = build_related_event_filters(1617, "pubkey", "eventid", &[]);
        let mut expected = Vec::new();

        let mut ids_filter = EventFilter::new();
        ids_filter.ids = vec!["eventid".to_string()];
        expected.push(ids_filter.with_limit(1));

        let mut ref_filter = EventFilter::new();
        ref_filter
            .tags
            .insert("e".to_string(), vec!["eventid".to_string()]);
        ref_filter
            .tags
            .insert("E".to_string(), vec!["eventid".to_string()]);
        expected.push(ref_filter.with_limit(1));

        let mut q_filter = EventFilter::new();
        q_filter
            .tags
            .insert("q".to_string(), vec!["eventid".to_string()]);
        expected.push(q_filter.with_limit(1));

        assert_eq!(filters, expected);
    }

    #[test]
    fn ignores_invalid_address_pointer_shapes() {
        let tags = vec![vec![
            "a".to_string(),
            "30617:author:repo:extra".to_string(),
        ]];
        let filters = build_related_event_filters(1617, "pubkey", "eventid", &tags);
        assert!(filters.iter().all(|filter| filter.kinds.is_empty()));
    }

    #[test]
    fn pointer_without_identifier_builds_kind_and_author_filter() {
        let tags = vec![vec!["a".to_string(), "30617:author".to_string()]];
        let filters = build_related_event_filters(1617, "pubkey", "eventid", &tags);
        let pointer_filter = &filters[1];
        assert_eq!(pointer_filter.kinds, vec![30617]);
        assert_eq!(pointer_filter.authors, vec!["author".to_string()]);
        assert!(pointer_filter.tags.get("d").is_none());
    }

    #[test]
    fn ignores_address_pointer_with_invalid_kind() {
        let tags = vec![vec!["a".to_string(), "not-a-kind:author:repo".to_string()]];
        let filters = build_related_event_filters(1617, "pubkey", "eventid", &tags);
        assert!(filters.iter().all(|filter| filter.kinds.is_empty()));
    }

    #[test]
    fn ignores_author_only_pointer_with_invalid_kind() {
        let tags = vec![vec!["a".to_string(), "not-a-kind:author".to_string()]];
        let filters = build_related_event_filters(1617, "pubkey", "eventid", &tags);
        assert!(filters.iter().all(|filter| filter.kinds.is_empty()));
    }

    fn map_for(tag: &str, values: &[&str]) -> BTreeMap<String, Vec<String>> {
        let mut map = BTreeMap::new();
        map.insert(
            tag.to_string(),
            values.iter().map(|v| v.to_string()).collect(),
        );
        map
    }

    #[test]
    fn map_for_helper_smoke() {
        let map = map_for("e", &["a"]);
        assert_eq!(map.get("e").unwrap(), &vec!["a".to_string()]);
    }
}
