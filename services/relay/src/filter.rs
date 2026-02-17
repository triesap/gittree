use crate::{NostrEvent, TagIndex};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Filter {
    pub ids: Vec<String>,
    pub authors: Vec<String>,
    pub kinds: Vec<u32>,
    pub since: Option<i64>,
    pub until: Option<i64>,
    pub limit: Option<u64>,
    pub tags: BTreeMap<String, Vec<String>>,
}

impl Filter {
    pub fn from_json(value: &Value) -> Result<Self, FilterError> {
        let obj = value.as_object().ok_or(FilterError::InvalidFilter)?;
        let mut filter = Filter {
            ids: Vec::new(),
            authors: Vec::new(),
            kinds: Vec::new(),
            since: None,
            until: None,
            limit: None,
            tags: BTreeMap::new(),
        };

        for (key, value) in obj {
            match key.as_str() {
                "ids" => filter.ids = parse_string_array(key, value)?,
                "authors" => filter.authors = parse_string_array(key, value)?,
                "kinds" => filter.kinds = parse_u32_array(key, value)?,
                "since" => filter.since = parse_i64(key, value)?,
                "until" => filter.until = parse_i64(key, value)?,
                "limit" => filter.limit = parse_u64(key, value)?,
                _ if key.starts_with('#') => {
                    let tag = key.trim_start_matches('#');
                    if tag.is_empty() {
                        return Err(FilterError::InvalidField(key.to_string()));
                    }
                    let values = parse_string_array(key, value)?;
                    filter.tags.insert(tag.to_string(), values);
                }
                _ => {}
            }
        }

        Ok(filter)
    }

    pub fn matches(&self, event: &NostrEvent, tags: &TagIndex) -> bool {
        if !self.ids.is_empty() && !self.ids.iter().any(|prefix| event.id.starts_with(prefix)) {
            return false;
        }

        if !self.authors.is_empty()
            && !self
                .authors
                .iter()
                .any(|prefix| event.pubkey.starts_with(prefix))
        {
            return false;
        }

        if !self.kinds.is_empty() && !self.kinds.iter().any(|kind| *kind == event.kind) {
            return false;
        }

        if let Some(since) = self.since {
            if event.created_at < since {
                return false;
            }
        }

        if let Some(until) = self.until {
            if event.created_at > until {
                return false;
            }
        }

        for (tag, values) in &self.tags {
            let Some(existing) = tags.values(tag) else {
                return false;
            };
            if !values
                .iter()
                .any(|value| existing.iter().any(|existing| existing == value))
            {
                return false;
            }
        }

        true
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilterError {
    InvalidFilter,
    InvalidField(String),
}

impl std::fmt::Display for FilterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FilterError::InvalidFilter => write!(f, "invalid filter"),
            FilterError::InvalidField(field) => write!(f, "invalid filter field {field}"),
        }
    }
}

impl std::error::Error for FilterError {}

fn parse_string_array(field: &str, value: &Value) -> Result<Vec<String>, FilterError> {
    let Some(items) = value.as_array() else {
        return Err(FilterError::InvalidField(field.to_string()));
    };
    let mut out = Vec::new();
    for item in items {
        let Some(value) = item.as_str() else {
            return Err(FilterError::InvalidField(field.to_string()));
        };
        out.push(value.to_string());
    }
    Ok(out)
}

fn parse_u32_array(field: &str, value: &Value) -> Result<Vec<u32>, FilterError> {
    let Some(items) = value.as_array() else {
        return Err(FilterError::InvalidField(field.to_string()));
    };
    let mut out = Vec::new();
    for item in items {
        let Some(value) = item.as_u64() else {
            return Err(FilterError::InvalidField(field.to_string()));
        };
        out.push(value as u32);
    }
    Ok(out)
}

fn parse_u64(field: &str, value: &Value) -> Result<Option<u64>, FilterError> {
    match value.as_u64() {
        Some(number) => Ok(Some(number)),
        None => Err(FilterError::InvalidField(field.to_string())),
    }
}

fn parse_i64(field: &str, value: &Value) -> Result<Option<i64>, FilterError> {
    match value.as_i64() {
        Some(number) => Ok(Some(number)),
        None => Err(FilterError::InvalidField(field.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::{Filter, FilterError};
    use crate::{NostrEvent, TagIndex};
    use serde_json::json;

    fn sample_event() -> NostrEvent {
        NostrEvent {
            id: "abcdef1234".to_string(),
            pubkey: "aa".repeat(32),
            created_at: 100,
            kind: 1,
            tags: vec![vec!["e".to_string(), "1".to_string()]],
            content: String::new(),
            sig: "00".repeat(64),
        }
    }

    #[test]
    fn parse_filter_reads_fields() {
        let value = json!({
            "ids": ["abc"],
            "authors": ["aa"],
            "kinds": [1, 2],
            "since": 50,
            "until": 150,
            "limit": 10,
            "#e": ["1", "2"]
        });
        let filter = Filter::from_json(&value).expect("filter");
        assert_eq!(filter.ids, vec!["abc"]);
        assert_eq!(filter.authors, vec!["aa"]);
        assert_eq!(filter.kinds, vec![1, 2]);
        assert_eq!(filter.since, Some(50));
        assert_eq!(filter.until, Some(150));
        assert_eq!(filter.limit, Some(10));
        assert_eq!(
            filter.tags.get("e"),
            Some(&vec!["1".to_string(), "2".to_string()])
        );
    }

    #[test]
    fn parse_filter_rejects_invalid_field() {
        let value = json!({"ids": "nope"});
        let err = Filter::from_json(&value).unwrap_err();
        assert_eq!(err, FilterError::InvalidField("ids".to_string()));
    }

    #[test]
    fn parse_filter_rejects_non_object_and_formats_invalid_filter_error() {
        let err = Filter::from_json(&json!(["not", "an", "object"])).unwrap_err();
        assert_eq!(err, FilterError::InvalidFilter);
        assert_eq!(err.to_string(), "invalid filter");
    }

    #[test]
    fn parse_filter_ignores_unknown_keys_and_rejects_empty_tag_keys() {
        let filter = Filter::from_json(&json!({"unsupported": 1})).expect("filter");
        assert!(filter.ids.is_empty());
        assert!(filter.authors.is_empty());

        let err = Filter::from_json(&json!({"#": ["value"]})).unwrap_err();
        assert_eq!(err, FilterError::InvalidField("#".to_string()));
    }

    #[test]
    fn parse_filter_rejects_invalid_array_value_types() {
        let err = Filter::from_json(&json!({"ids": [1]})).unwrap_err();
        assert_eq!(err, FilterError::InvalidField("ids".to_string()));

        let err = Filter::from_json(&json!({"authors": [1]})).unwrap_err();
        assert_eq!(err, FilterError::InvalidField("authors".to_string()));

        let err = Filter::from_json(&json!({"kinds": "nope"})).unwrap_err();
        assert_eq!(err, FilterError::InvalidField("kinds".to_string()));

        let err = Filter::from_json(&json!({"kinds": ["x"]})).unwrap_err();
        assert_eq!(err, FilterError::InvalidField("kinds".to_string()));

        let err = Filter::from_json(&json!({"since": "x"})).unwrap_err();
        assert_eq!(err, FilterError::InvalidField("since".to_string()));

        let err = Filter::from_json(&json!({"until": "x"})).unwrap_err();
        assert_eq!(err, FilterError::InvalidField("until".to_string()));

        let err = Filter::from_json(&json!({"limit": "x"})).unwrap_err();
        assert_eq!(err, FilterError::InvalidField("limit".to_string()));

        let err = Filter::from_json(&json!({"#e": "x"})).unwrap_err();
        assert_eq!(err, FilterError::InvalidField("#e".to_string()));
    }

    #[test]
    fn matches_event_with_prefixes_and_time() {
        let value = json!({
            "ids": ["abc"],
            "authors": ["aa"],
            "kinds": [1],
            "since": 10,
            "until": 200
        });
        let filter = Filter::from_json(&value).expect("filter");
        let event = sample_event();
        let tags = TagIndex::from_tags(&event.tags).expect("tags");
        assert!(filter.matches(&event, &tags));
    }

    #[test]
    fn matches_when_tag_value_intersects() {
        let value = json!({"#e": ["1", "missing"]});
        let filter = Filter::from_json(&value).expect("filter");
        let event = sample_event();
        let tags = TagIndex::from_tags(&event.tags).expect("tags");
        assert!(filter.matches(&event, &tags));
    }

    #[test]
    fn rejects_when_tag_missing() {
        let value = json!({"#e": ["nope"]});
        let filter = Filter::from_json(&value).expect("filter");
        let event = sample_event();
        let tags = TagIndex::from_tags(&event.tags).expect("tags");
        assert!(!filter.matches(&event, &tags));
    }

    #[test]
    fn matches_rejects_author_kind_since_and_missing_tag() {
        let event = sample_event();
        let tags = TagIndex::from_tags(&event.tags).expect("tags");

        let author_mismatch = Filter::from_json(&json!({"authors": ["bb"]})).expect("filter");
        assert!(!author_mismatch.matches(&event, &tags));

        let kind_mismatch = Filter::from_json(&json!({"kinds": [2]})).expect("filter");
        assert!(!kind_mismatch.matches(&event, &tags));

        let since_mismatch = Filter::from_json(&json!({"since": 101})).expect("filter");
        assert!(!since_mismatch.matches(&event, &tags));

        let missing_tag = Filter::from_json(&json!({"#p": ["missing"]})).expect("filter");
        assert!(!missing_tag.matches(&event, &tags));
    }
}
