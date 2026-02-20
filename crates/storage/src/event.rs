use crate::StorageError;

const HEX_32_LEN: usize = 64;
const HEX_64_LEN: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagRecord {
    pub name: String,
    pub value: String,
}

impl TagRecord {
    pub fn new(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventRecord {
    pub tenant_id: String,
    pub id: Vec<u8>,
    pub pubkey: Vec<u8>,
    pub created_at: i64,
    pub kind: u32,
    pub content: String,
    pub sig: Vec<u8>,
    pub tags: Vec<TagRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct EventQuery {
    pub tenant_id: Option<String>,
    pub ids: Vec<String>,
    pub authors: Vec<String>,
    pub kinds: Vec<u32>,
    pub since: Option<i64>,
    pub until: Option<i64>,
    pub tags: Vec<TagRecord>,
    pub limit: Option<u64>,
}

impl EventQuery {
    pub fn for_tenant(tenant_id: impl Into<String>) -> Self {
        Self {
            tenant_id: Some(tenant_id.into()),
            ..Default::default()
        }
    }

    pub fn for_ids(ids: Vec<String>) -> Self {
        Self {
            ids,
            ..Default::default()
        }
    }

    pub fn for_authors(authors: Vec<String>) -> Self {
        Self {
            authors,
            ..Default::default()
        }
    }

    pub fn for_kinds(kinds: Vec<u32>) -> Self {
        Self {
            kinds,
            ..Default::default()
        }
    }

    pub fn for_tag(name: impl Into<String>, values: Vec<String>) -> Self {
        let name = name.into();
        let tags = values
            .into_iter()
            .map(|value| TagRecord::new(name.clone(), value))
            .collect();
        Self {
            tags,
            ..Default::default()
        }
    }
}

impl EventRecord {
    pub fn new(
        tenant_id: &str,
        id: &str,
        pubkey: &str,
        created_at: i64,
        kind: u32,
        content: String,
        sig: &str,
        tags: Vec<Vec<String>>,
    ) -> Result<Self, StorageError> {
        let tags = flatten_tags(&tags)?;
        if tenant_id.trim().is_empty() {
            return Err(StorageError::InvalidField {
                field: "tenant_id",
                value: tenant_id.to_string(),
            });
        }
        Ok(Self {
            tenant_id: tenant_id.to_string(),
            id: decode_hex("id", id, HEX_32_LEN)?,
            pubkey: decode_hex("pubkey", pubkey, HEX_32_LEN)?,
            created_at,
            kind,
            content,
            sig: decode_hex("sig", sig, HEX_64_LEN)?,
            tags,
        })
    }
}

fn flatten_tags(tags: &[Vec<String>]) -> Result<Vec<TagRecord>, StorageError> {
    let mut records = Vec::new();
    for tag in tags {
        let Some((name, values)) = tag.split_first() else {
            return Err(StorageError::InvalidField {
                field: "tags",
                value: "empty tag".to_string(),
            });
        };
        if name.is_empty() {
            return Err(StorageError::InvalidField {
                field: "tags",
                value: "empty tag name".to_string(),
            });
        }
        for value in values {
            if value.is_empty() {
                return Err(StorageError::InvalidField {
                    field: "tags",
                    value: "empty tag value".to_string(),
                });
            }
            records.push(TagRecord {
                name: name.to_string(),
                value: value.to_string(),
            });
        }
    }
    Ok(records)
}

#[rustfmt::skip]
fn decode_hex(field: &'static str, value: &str, expected_len: usize) -> Result<Vec<u8>, StorageError> {
    if value.len() != expected_len {
        return Err(StorageError::InvalidHex { field, value: value.to_string() });
    }
    hex::decode(value).map_err(|_| StorageError::InvalidHex {
        field,
        value: value.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::EventQuery;
    use super::EventRecord;
    use super::TagRecord;
    use crate::StorageError;

    fn hex_32(byte: u8) -> String {
        format!("{:02x}", byte).repeat(32)
    }

    fn hex_64(byte: u8) -> String {
        format!("{:02x}", byte).repeat(64)
    }

    fn assert_invalid_field(err: StorageError) {
        if !matches!(err, StorageError::InvalidField { .. }) {
            panic!("expected invalid field error, got {err:?}");
        }
    }

    #[test]
    fn event_record_decodes_hex_fields() {
        let record = EventRecord::new(
            "default",
            &hex_32(0x11),
            &hex_32(0x22),
            12,
            1,
            "content".to_string(),
            &hex_64(0x33),
            vec![vec!["e".to_string(), "abc".to_string()]],
        )
        .expect("record");

        assert_eq!(record.id, hex::decode(hex_32(0x11)).expect("id"));
        assert_eq!(record.pubkey, hex::decode(hex_32(0x22)).expect("pubkey"));
        assert_eq!(record.sig, hex::decode(hex_64(0x33)).expect("sig"));
        assert_eq!(record.tags.len(), 1);
        assert_eq!(record.tags[0].name, "e");
        assert_eq!(record.tags[0].value, "abc");
    }

    #[test]
    fn event_record_rejects_empty_tags() {
        let err = EventRecord::new(
            "default",
            &hex_32(0x11),
            &hex_32(0x22),
            12,
            1,
            "content".to_string(),
            &hex_64(0x33),
            vec![vec![]],
        )
        .unwrap_err();

        assert_invalid_field(err);
    }

    #[test]
    #[should_panic(expected = "expected invalid field error")]
    fn assert_invalid_field_panics_for_other_errors() {
        assert_invalid_field(StorageError::Internal {
            message: "wrong variant".to_string(),
        });
    }

    #[test]
    fn event_record_rejects_empty_tenant_id() {
        let err = EventRecord::new(
            " ",
            &hex_32(0x11),
            &hex_32(0x22),
            12,
            1,
            "content".to_string(),
            &hex_64(0x33),
            vec![vec!["e".to_string(), "abc".to_string()]],
        )
        .unwrap_err();
        assert!(matches!(
            err,
            StorageError::InvalidField {
                field: "tenant_id",
                ..
            }
        ));
    }

    #[test]
    fn event_record_rejects_empty_tag_name_and_value() {
        let err = EventRecord::new(
            "default",
            &hex_32(0x11),
            &hex_32(0x22),
            12,
            1,
            "content".to_string(),
            &hex_64(0x33),
            vec![vec!["".to_string(), "abc".to_string()]],
        )
        .unwrap_err();
        assert!(matches!(
            err,
            StorageError::InvalidField { field: "tags", .. }
        ));

        let err = EventRecord::new(
            "default",
            &hex_32(0x11),
            &hex_32(0x22),
            12,
            1,
            "content".to_string(),
            &hex_64(0x33),
            vec![vec!["e".to_string(), "".to_string()]],
        )
        .unwrap_err();
        assert!(matches!(
            err,
            StorageError::InvalidField { field: "tags", .. }
        ));
    }

    #[test]
    fn event_record_rejects_invalid_hex_lengths() {
        let err = EventRecord::new(
            "default",
            "11",
            &hex_32(0x22),
            12,
            1,
            "content".to_string(),
            &hex_64(0x33),
            vec![vec!["e".to_string(), "abc".to_string()]],
        )
        .unwrap_err();
        assert!(matches!(err, StorageError::InvalidHex { field: "id", .. }));

        let err = EventRecord::new(
            "default",
            &hex_32(0x11),
            "22",
            12,
            1,
            "content".to_string(),
            &hex_64(0x33),
            vec![vec!["e".to_string(), "abc".to_string()]],
        )
        .unwrap_err();
        assert!(matches!(
            err,
            StorageError::InvalidHex {
                field: "pubkey",
                ..
            }
        ));

        let err = EventRecord::new(
            "default",
            &hex_32(0x11),
            &hex_32(0x22),
            12,
            1,
            "content".to_string(),
            "33",
            vec![vec!["e".to_string(), "abc".to_string()]],
        )
        .unwrap_err();
        assert!(matches!(err, StorageError::InvalidHex { field: "sig", .. }));
    }

    #[test]
    fn event_record_rejects_invalid_hex_payloads() {
        let bad_hex = format!("{}z", "1".repeat(63));
        let err = EventRecord::new(
            "default",
            &bad_hex,
            &hex_32(0x22),
            12,
            1,
            "content".to_string(),
            &hex_64(0x33),
            vec![vec!["e".to_string(), "abc".to_string()]],
        )
        .unwrap_err();
        assert!(matches!(err, StorageError::InvalidHex { field: "id", .. }));

        let bad_pubkey = format!("{}z", "2".repeat(63));
        let err = EventRecord::new(
            "default",
            &hex_32(0x11),
            &bad_pubkey,
            12,
            1,
            "content".to_string(),
            &hex_64(0x33),
            vec![vec!["e".to_string(), "abc".to_string()]],
        )
        .unwrap_err();
        assert!(matches!(
            err,
            StorageError::InvalidHex {
                field: "pubkey",
                ..
            }
        ));

        let bad_sig = format!("{}z", "3".repeat(127));
        let err = EventRecord::new(
            "default",
            &hex_32(0x11),
            &hex_32(0x22),
            12,
            1,
            "content".to_string(),
            &bad_sig,
            vec![vec!["e".to_string(), "abc".to_string()]],
        )
        .unwrap_err();
        assert!(matches!(err, StorageError::InvalidHex { field: "sig", .. }));
    }

    #[test]
    fn event_query_defaults_empty() {
        let query = EventQuery::default();
        assert!(query.tenant_id.is_none());
        assert!(query.ids.is_empty());
        assert!(query.tags.is_empty());
        assert_eq!(query.limit, None);
    }

    #[test]
    fn tag_record_compares_by_value() {
        let left = TagRecord {
            name: "e".to_string(),
            value: "1".to_string(),
        };
        let right = TagRecord {
            name: "e".to_string(),
            value: "1".to_string(),
        };
        assert_eq!(left, right);
    }

    #[test]
    fn tag_record_new_sets_fields() {
        let record = TagRecord::new("e", "abc");
        assert_eq!(record.name, "e");
        assert_eq!(record.value, "abc");
    }

    #[test]
    fn event_query_helpers_set_fields() {
        let ids = vec!["aa".to_string(), "bb".to_string()];
        let query = EventQuery::for_ids(ids.clone());
        assert_eq!(query.ids, ids);

        let authors = vec!["cc".to_string()];
        let query = EventQuery::for_authors(authors.clone());
        assert_eq!(query.authors, authors);

        let kinds = vec![1, 2];
        let query = EventQuery::for_kinds(kinds.clone());
        assert_eq!(query.kinds, kinds);

        let query = EventQuery::for_tag("e", vec!["one".to_string(), "two".to_string()]);
        assert_eq!(query.tags.len(), 2);
        assert!(query.tags.iter().all(|tag| tag.name == "e"));

        let query = EventQuery::for_tenant("default");
        assert_eq!(query.tenant_id, Some("default".to_string()));
    }
}
