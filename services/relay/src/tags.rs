use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TagError {
    EmptyTag,
    EmptyName,
    EmptyValue,
}

impl std::fmt::Display for TagError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TagError::EmptyTag => write!(f, "tag is empty"),
            TagError::EmptyName => write!(f, "tag name is empty"),
            TagError::EmptyValue => write!(f, "tag value is empty"),
        }
    }
}

impl std::error::Error for TagError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagIndex {
    values: BTreeMap<String, Vec<String>>,
}

impl TagIndex {
    pub fn from_tags(tags: &[Vec<String>]) -> Result<Self, TagError> {
        let mut values = BTreeMap::new();
        for tag in tags {
            if tag.is_empty() {
                return Err(TagError::EmptyTag);
            }
            let name = tag[0].trim();
            if name.is_empty() {
                return Err(TagError::EmptyName);
            }
            let entry = values.entry(name.to_string()).or_insert_with(Vec::new);
            for value in tag.iter().skip(1) {
                if value.trim().is_empty() {
                    return Err(TagError::EmptyValue);
                }
                if !entry.iter().any(|item| item == value) {
                    entry.push(value.clone());
                }
            }
        }
        Ok(Self { values })
    }

    pub fn values(&self, name: &str) -> Option<&[String]> {
        self.values.get(name).map(Vec::as_slice)
    }
}

#[cfg(test)]
mod tests {
    use super::{TagError, TagIndex};

    #[test]
    fn rejects_empty_tags() {
        let tags = vec![vec![]];
        let err = TagIndex::from_tags(&tags).unwrap_err();
        assert_eq!(err, TagError::EmptyTag);
    }

    #[test]
    fn rejects_empty_name() {
        let tags = vec![vec!["".to_string()]];
        let err = TagIndex::from_tags(&tags).unwrap_err();
        assert_eq!(err, TagError::EmptyName);
    }

    #[test]
    fn rejects_empty_value() {
        let tags = vec![vec!["e".to_string(), " ".to_string()]];
        let err = TagIndex::from_tags(&tags).unwrap_err();
        assert_eq!(err, TagError::EmptyValue);
    }

    #[test]
    fn collects_values_per_tag_name() {
        let tags = vec![
            vec!["e".to_string(), "1".to_string()],
            vec!["e".to_string(), "1".to_string(), "2".to_string()],
            vec!["p".to_string(), "alice".to_string()],
        ];
        let index = TagIndex::from_tags(&tags).expect("index");
        assert_eq!(index.values("e"), Some(&["1".to_string(), "2".to_string()][..]));
        assert_eq!(index.values("p"), Some(&["alice".to_string()][..]));
        assert_eq!(index.values("missing"), None);
    }

    #[test]
    fn collects_unique_values_and_trims_names() {
        let tags = vec![
            vec![" e ".to_string(), "1".to_string(), "1".to_string()],
            vec!["e".to_string(), "2".to_string()],
        ];
        let index = TagIndex::from_tags(&tags).expect("index");
        assert_eq!(index.values("e"), Some(&["1".to_string(), "2".to_string()][..]));
    }

    #[test]
    fn accepts_tags_with_only_names() {
        let tags = vec![vec!["e".to_string()]];
        let index = TagIndex::from_tags(&tags).expect("index");
        assert_eq!(index.values("e"), Some(&[][..]));
    }
}
