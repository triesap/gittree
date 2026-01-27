use crate::StorageError;

const HEX_32_LEN: usize = 64;
const HEX_64_LEN: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagRecord {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventRecord {
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
    pub ids: Vec<String>,
    pub authors: Vec<String>,
    pub kinds: Vec<u32>,
    pub since: Option<i64>,
    pub until: Option<i64>,
    pub tags: Vec<TagRecord>,
    pub limit: Option<u64>,
}

impl EventRecord {
    pub fn new(
        id: &str,
        pubkey: &str,
        created_at: i64,
        kind: u32,
        content: impl Into<String>,
        sig: &str,
        tags: Vec<Vec<String>>,
    ) -> Result<Self, StorageError> {
        let tags = flatten_tags(&tags)?;
        Ok(Self {
            id: decode_hex("id", id, HEX_32_LEN)?,
            pubkey: decode_hex("pubkey", pubkey, HEX_32_LEN)?,
            created_at,
            kind,
            content: content.into(),
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

fn decode_hex(field: &'static str, value: &str, expected_len: usize) -> Result<Vec<u8>, StorageError> {
    if value.len() != expected_len {
        return Err(StorageError::InvalidHex {
            field,
            value: value.to_string(),
        });
    }

    hex::decode(value).map_err(|_| StorageError::InvalidHex {
        field,
        value: value.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::EventRecord;
    use super::EventQuery;
    use super::TagRecord;
    use crate::StorageError;

    fn hex_32(byte: u8) -> String {
        format!("{:02x}", byte).repeat(32)
    }

    fn hex_64(byte: u8) -> String {
        format!("{:02x}", byte).repeat(64)
    }

    #[test]
    fn event_record_decodes_hex_fields() {
        let record = EventRecord::new(
            &hex_32(0x11),
            &hex_32(0x22),
            12,
            1,
            "content",
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
            &hex_32(0x11),
            &hex_32(0x22),
            12,
            1,
            "content",
            &hex_64(0x33),
            vec![vec![]],
        )
        .unwrap_err();

        assert!(matches!(err, StorageError::InvalidField { .. }));
    }

    #[test]
    fn event_query_defaults_empty() {
        let query = EventQuery::default();
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
}
