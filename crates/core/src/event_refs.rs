use crate::tags::push_unique;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventReferences {
    pub event_ids: Vec<String>,
    pub address_pointers: Vec<String>,
}

impl EventReferences {
    pub fn new() -> Self {
        Self {
            event_ids: Vec::new(),
            address_pointers: Vec::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.event_ids.is_empty() && self.address_pointers.is_empty()
    }
}

pub fn collect_event_references(tags: &[Vec<String>]) -> EventReferences {
    let mut refs = EventReferences::new();

    for tag in tags {
        if tag.len() < 2 {
            continue;
        }

        let key = tag[0].as_str();
        let value = tag[1].as_str();
        match key {
            "a" | "A" => push_unique(&mut refs.address_pointers, value),
            "e" | "E" => push_unique(&mut refs.event_ids, value),
            "q" => {
                if value.contains(':') {
                    push_unique(&mut refs.address_pointers, value);
                } else {
                    push_unique(&mut refs.event_ids, value);
                }
            }
            _ => {}
        }
    }

    refs
}

#[cfg(test)]
mod tests {
    use super::collect_event_references;
    use super::EventReferences;

    #[test]
    fn collects_event_and_address_refs() {
        let tags = vec![
            vec!["a".to_string(), "30617:pub:repo".to_string()],
            vec!["A".to_string(), "30000:pub:other".to_string()],
            vec!["e".to_string(), "deadbeef".to_string()],
            vec!["E".to_string(), "feedface".to_string()],
            vec!["q".to_string(), "30617:pub:repo".to_string()],
            vec!["q".to_string(), "cafebabe".to_string()],
        ];

        let refs = collect_event_references(&tags);
        assert_eq!(
            refs,
            EventReferences {
                event_ids: vec!["deadbeef".to_string(), "feedface".to_string(), "cafebabe".to_string()],
                address_pointers: vec!["30617:pub:repo".to_string(), "30000:pub:other".to_string()],
            }
        );
    }

    #[test]
    fn ignores_missing_values() {
        let tags = vec![vec!["e".to_string()], vec!["a".to_string()]];
        let refs = collect_event_references(&tags);
        assert!(refs.is_empty());
    }

    #[test]
    fn de_dupes_references() {
        let tags = vec![
            vec!["e".to_string(), "deadbeef".to_string()],
            vec!["E".to_string(), "deadbeef".to_string()],
            vec!["a".to_string(), "30617:pub:repo".to_string()],
            vec!["q".to_string(), "30617:pub:repo".to_string()],
        ];
        let refs = collect_event_references(&tags);
        assert_eq!(refs.event_ids, vec!["deadbeef".to_string()]);
        assert_eq!(refs.address_pointers, vec!["30617:pub:repo".to_string()]);
    }
}
