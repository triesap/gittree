use crate::StorageError;
use gittree_core::{RepoAnnouncement, RepoState};
use std::collections::HashMap;

const HEX_LEN: usize = 64;

fn to_json_field(
    field: &'static str,
    encode: impl FnOnce() -> Result<String, serde_json::Error>,
) -> Result<String, StorageError> {
    encode().map_err(|source| StorageError::Serialization { field, source })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoAnnouncementRecord {
    pub event_id: Vec<u8>,
    pub pubkey: Vec<u8>,
    pub identifier: String,
    pub name: Option<String>,
    pub description: Option<String>,
    pub root_commit: Option<String>,
    pub clone_urls: Vec<String>,
    pub web_urls: Vec<String>,
    pub relays: Vec<String>,
    pub blossoms: Vec<String>,
    pub hashtags: Vec<String>,
    pub maintainers: Vec<String>,
    pub created_at: i64,
}

impl RepoAnnouncementRecord {
    pub fn new(
        event_id: &str,
        pubkey: &str,
        created_at: i64,
        announcement: &RepoAnnouncement,
    ) -> Result<Self, StorageError> {
        announcement
            .validate()
            .map_err(|err| StorageError::InvalidField {
                field: "announcement",
                value: err.to_string(),
            })?;
        Ok(Self {
            event_id: decode_hex_32("event_id", event_id)?,
            pubkey: decode_hex_32("pubkey", pubkey)?,
            identifier: announcement.identifier.clone(),
            name: announcement.name.clone(),
            description: announcement.description.clone(),
            root_commit: announcement.root_commit.clone(),
            clone_urls: announcement.clone.clone(),
            web_urls: announcement.web.clone(),
            relays: announcement.relays.clone(),
            blossoms: announcement.blossoms.clone(),
            hashtags: announcement.hashtags.clone(),
            maintainers: announcement.maintainers.clone(),
            created_at,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoStateRecord {
    pub event_id: Vec<u8>,
    pub pubkey: Vec<u8>,
    pub identifier: String,
    pub created_at: i64,
    pub state_json: String,
}

impl RepoStateRecord {
    pub fn new(
        event_id: &str,
        pubkey: &str,
        created_at: i64,
        state: &RepoState,
    ) -> Result<Self, StorageError> {
        state.validate().map_err(|err| StorageError::InvalidField {
            field: "state",
            value: err.to_string(),
        })?;
        let state_json = to_json_field("state", || serde_json::to_string(&state.state))?;

        Ok(Self {
            event_id: decode_hex_32("event_id", event_id)?,
            pubkey: decode_hex_32("pubkey", pubkey)?,
            identifier: state.identifier.clone(),
            created_at,
            state_json,
        })
    }

    pub fn state_map(&self) -> Result<HashMap<String, String>, StorageError> {
        serde_json::from_str(&self.state_json).map_err(|source| StorageError::Serialization {
            field: "state",
            source,
        })
    }
}

fn decode_hex_32(field: &'static str, value: &str) -> Result<Vec<u8>, StorageError> {
    if value.len() != HEX_LEN {
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
    use super::RepoAnnouncementRecord;
    use super::RepoStateRecord;
    use crate::StorageError;
    use gittree_core::RepoAnnouncement;
    use gittree_core::RepoState;
    use std::collections::HashMap;

    fn hex_32(byte: u8) -> String {
        format!("{:02x}", byte).repeat(32)
    }

    fn sample_announcement() -> RepoAnnouncement {
        RepoAnnouncement {
            identifier: "repo".to_string(),
            name: Some("Repo".to_string()),
            description: Some("Example repo".to_string()),
            root_commit: Some("0123456789abcdef0123456789abcdef01234567".to_string()),
            clone: vec!["https://git.example/repo.git".to_string()],
            web: vec!["https://git.example/repo".to_string()],
            relays: vec!["wss://relay.example".to_string()],
            blossoms: vec!["https://blossom.example".to_string()],
            hashtags: vec!["nostr".to_string()],
            maintainers: vec![
                "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".to_string(),
            ],
        }
    }

    #[test]
    fn announcement_record_maps_fields() {
        let event_id = hex_32(0x11);
        let pubkey = hex_32(0x22);
        let created_at = 1234;
        let announcement = sample_announcement();

        let record = RepoAnnouncementRecord::new(&event_id, &pubkey, created_at, &announcement)
            .expect("record");

        assert_eq!(record.event_id, hex::decode(&event_id).expect("event"));
        assert_eq!(record.pubkey, hex::decode(&pubkey).expect("pubkey"));
        assert_eq!(record.identifier, announcement.identifier);
        assert_eq!(record.clone_urls, announcement.clone);
        assert_eq!(record.relays, announcement.relays);
        assert_eq!(record.created_at, created_at);
    }

    #[test]
    fn state_record_serializes_json() {
        let event_id = hex_32(0x33);
        let pubkey = hex_32(0x44);
        let created_at = 5678;
        let mut state = HashMap::new();
        state.insert("HEAD".to_string(), "ref: refs/heads/main".to_string());
        state.insert(
            "refs/heads/main".to_string(),
            "0123456789abcdef0123456789abcdef01234567".to_string(),
        );
        let state = RepoState {
            identifier: "repo".to_string(),
            state,
        };

        let record = RepoStateRecord::new(&event_id, &pubkey, created_at, &state).expect("record");

        let parsed: serde_json::Value = serde_json::from_str(&record.state_json).expect("json");
        let expected = serde_json::json!({
            "HEAD": "ref: refs/heads/main",
            "refs/heads/main": "0123456789abcdef0123456789abcdef01234567",
        });

        assert_eq!(parsed, expected);
        assert_eq!(record.identifier, "repo");
        assert_eq!(record.created_at, created_at);
    }

    #[test]
    fn state_record_parses_json_map() {
        let event_id = hex_32(0x33);
        let pubkey = hex_32(0x44);
        let created_at = 5678;
        let mut state = HashMap::new();
        state.insert("HEAD".to_string(), "ref: refs/heads/main".to_string());
        state.insert(
            "refs/heads/main".to_string(),
            "0123456789abcdef0123456789abcdef01234567".to_string(),
        );
        let state = RepoState {
            identifier: "repo".to_string(),
            state,
        };

        let record = RepoStateRecord::new(&event_id, &pubkey, created_at, &state).expect("record");
        let parsed = record.state_map().expect("state map");
        assert!(parsed.contains_key("HEAD"));
        assert!(parsed.contains_key("refs/heads/main"));
    }

    #[test]
    fn to_json_field_maps_serialization_error_with_field_name() {
        use serde::ser::Error as _;
        use serde::ser::Serializer;
        use serde::Serialize;

        struct AlwaysFail;

        impl Serialize for AlwaysFail {
            fn serialize<S>(&self, _serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                Err(S::Error::custom("always fail"))
            }
        }

        let err = super::to_json_field("state", || serde_json::to_string(&AlwaysFail)).unwrap_err();
        assert!(matches!(err, StorageError::Serialization { field, .. } if field == "state"));
    }

    #[test]
    fn record_rejects_short_hex() {
        let announcement = sample_announcement();
        let err = RepoAnnouncementRecord::new("abcd", &hex_32(0x55), 0, &announcement).unwrap_err();
        assert!(matches!(
            err,
            StorageError::InvalidHex {
                field: "event_id",
                ..
            }
        ));
    }

    #[test]
    fn record_rejects_invalid_announcement() {
        let mut announcement = sample_announcement();
        announcement.clone.clear();
        let err = RepoAnnouncementRecord::new(&hex_32(0x11), &hex_32(0x22), 0, &announcement)
            .unwrap_err();
        assert!(matches!(
            err,
            StorageError::InvalidField {
                field: "announcement",
                ..
            }
        ));
    }

    #[test]
    fn record_rejects_invalid_state() {
        let mut state_map = std::collections::HashMap::new();
        state_map.insert(
            "refs/heads/main".to_string(),
            "0123456789abcdef0123456789abcdef01234567".to_string(),
        );
        let state = RepoState {
            identifier: "repo".to_string(),
            state: state_map,
        };
        let err = RepoStateRecord::new(&hex_32(0x11), &hex_32(0x22), 0, &state).unwrap_err();
        assert!(matches!(
            err,
            StorageError::InvalidField { field: "state", .. }
        ));
    }

    #[test]
    fn record_rejects_non_hex_with_expected_length() {
        let announcement = sample_announcement();
        let err = RepoAnnouncementRecord::new(&"zz".repeat(32), &hex_32(0x55), 0, &announcement)
            .unwrap_err();
        assert!(matches!(
            err,
            StorageError::InvalidHex {
                field: "event_id",
                ..
            }
        ));

        let valid_state = RepoState {
            identifier: "repo".to_string(),
            state: HashMap::from([
                ("HEAD".to_string(), "ref: refs/heads/main".to_string()),
                (
                    "refs/heads/main".to_string(),
                    "0123456789abcdef0123456789abcdef01234567".to_string(),
                ),
            ]),
        };
        let err = RepoStateRecord::new(&hex_32(0x11), &"gg".repeat(32), 0, &valid_state)
            .unwrap_err();
        assert!(matches!(
            err,
            StorageError::InvalidHex { field: "pubkey", .. }
        ));
    }

    #[test]
    fn state_record_rejects_invalid_state_json_on_read() {
        let mut record = RepoStateRecord {
            event_id: vec![0u8; 32],
            pubkey: vec![1u8; 32],
            identifier: "repo".to_string(),
            created_at: 1,
            state_json: "{\"broken\":".to_string(),
        };
        let err = record.state_map().unwrap_err();
        assert!(matches!(
            err,
            StorageError::Serialization { field: "state", .. }
        ));

        record.state_json = "{\"refs/heads/main\":\"deadbeef\"}".to_string();
        assert!(record.state_map().is_ok());
    }
}
