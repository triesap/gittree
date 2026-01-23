use crate::tags::push_unique;

const ADDRESSABLE_KIND_START: u32 = 30000;
const ADDRESSABLE_KIND_END: u32 = 40000;

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

pub fn collect_event_references_with_self(
    kind: u32,
    pubkey: &str,
    event_id: &str,
    tags: &[Vec<String>],
) -> EventReferences {
    let mut refs = collect_event_references(tags);

    if is_addressable_kind(kind) {
        let pointer = match find_d_tag(tags) {
            Some(identifier) => format!("{kind}:{pubkey}:{identifier}"),
            None => format!("{kind}:{pubkey}"),
        };
        push_unique(&mut refs.address_pointers, &pointer);
    } else {
        push_unique(&mut refs.event_ids, event_id);
    }

    refs
}

fn is_addressable_kind(kind: u32) -> bool {
    kind >= ADDRESSABLE_KIND_START && kind < ADDRESSABLE_KIND_END
}

fn find_d_tag(tags: &[Vec<String>]) -> Option<&str> {
    tags.iter().find_map(|tag| {
        if tag.len() > 1 && tag[0] == "d" {
            Some(tag[1].as_str())
        } else {
            None
        }
    })
}

#[cfg(test)]
mod tests {
    use super::EventReferences;
    use super::collect_event_references;
    use super::collect_event_references_with_self;

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
                event_ids: vec![
                    "deadbeef".to_string(),
                    "feedface".to_string(),
                    "cafebabe".to_string()
                ],
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

    #[test]
    fn adds_self_pointer_for_addressable_kind() {
        let tags = vec![vec!["d".to_string(), "repo".to_string()]];
        let refs = collect_event_references_with_self(30617, "pubkey", "event", &tags);
        assert_eq!(refs.address_pointers, vec!["30617:pubkey:repo".to_string()]);
        assert!(refs.event_ids.is_empty());
    }

    #[test]
    fn adds_self_id_for_non_addressable_kind() {
        let refs = collect_event_references_with_self(10317, "pubkey", "eventid", &[]);
        assert_eq!(refs.event_ids, vec!["eventid".to_string()]);
        assert!(refs.address_pointers.is_empty());
    }

    #[test]
    fn adds_self_pointer_without_d_tag() {
        let refs = collect_event_references_with_self(30617, "pubkey", "event", &[]);
        assert_eq!(refs.address_pointers, vec!["30617:pubkey".to_string()]);
    }
}
