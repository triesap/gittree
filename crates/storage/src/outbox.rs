#![forbid(unsafe_code)]

use crate::StorageError;
use time::OffsetDateTime;

const HEX_32_LEN: usize = 64;
const HEX_64_LEN: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayPublishRequest {
    pub relay_url: String,
    pub event_id: String,
    pub pubkey: String,
    pub created_at: i64,
    pub kind: u32,
    pub tags: Vec<Vec<String>>,
    pub content: String,
    pub sig: String,
    pub forgejo_owner: String,
    pub forgejo_repo: String,
    pub identifier: String,
}

impl RelayPublishRequest {
    pub fn decode(&self) -> Result<RelayPublishEntry, StorageError> {
        require_non_empty("relay_url", &self.relay_url)?;
        require_non_empty("forgejo_owner", &self.forgejo_owner)?;
        require_non_empty("forgejo_repo", &self.forgejo_repo)?;
        require_non_empty("identifier", &self.identifier)?;
        Ok(RelayPublishEntry {
            relay_url: self.relay_url.clone(),
            event_id: decode_hex("event_id", &self.event_id, HEX_32_LEN)?,
            pubkey: decode_hex("pubkey", &self.pubkey, HEX_32_LEN)?,
            created_at: self.created_at,
            kind: self.kind,
            tags: self.tags.clone(),
            content: self.content.clone(),
            sig: decode_hex("sig", &self.sig, HEX_64_LEN)?,
            forgejo_owner: self.forgejo_owner.clone(),
            forgejo_repo: self.forgejo_repo.clone(),
            identifier: self.identifier.clone(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayPublishEntry {
    pub relay_url: String,
    pub event_id: Vec<u8>,
    pub pubkey: Vec<u8>,
    pub created_at: i64,
    pub kind: u32,
    pub tags: Vec<Vec<String>>,
    pub content: String,
    pub sig: Vec<u8>,
    pub forgejo_owner: String,
    pub forgejo_repo: String,
    pub identifier: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayPublishJob {
    pub id: i64,
    pub relay_url: String,
    pub event_id: Vec<u8>,
    pub pubkey: Vec<u8>,
    pub created_at: i64,
    pub kind: u32,
    pub tags: Vec<Vec<String>>,
    pub content: String,
    pub sig: Vec<u8>,
    pub forgejo_owner: String,
    pub forgejo_repo: String,
    pub identifier: String,
    pub attempt_count: i32,
    pub publish_after: OffsetDateTime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelayPublishStatus {
    Pending,
    Publishing,
    Published,
}

impl RelayPublishStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            RelayPublishStatus::Pending => "pending",
            RelayPublishStatus::Publishing => "publishing",
            RelayPublishStatus::Published => "published",
        }
    }

    pub fn parse(value: &str) -> Result<Self, StorageError> {
        match value {
            "pending" => Ok(RelayPublishStatus::Pending),
            "publishing" => Ok(RelayPublishStatus::Publishing),
            "published" => Ok(RelayPublishStatus::Published),
            _ => Err(StorageError::InvalidField {
                field: "status",
                value: value.to_string(),
            }),
        }
    }
}

fn decode_hex(field: &'static str, value: &str, len: usize) -> Result<Vec<u8>, StorageError> {
    if value.len() != len {
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

fn require_non_empty(field: &'static str, value: &str) -> Result<(), StorageError> {
    if value.trim().is_empty() {
        return Err(StorageError::InvalidField {
            field,
            value: "".to_string(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{RelayPublishRequest, RelayPublishStatus};

    fn valid_request() -> RelayPublishRequest {
        RelayPublishRequest {
            relay_url: "wss://relay.example".to_string(),
            event_id: "11".repeat(32),
            pubkey: "22".repeat(32),
            created_at: 1,
            kind: 1,
            tags: Vec::new(),
            content: String::new(),
            sig: "33".repeat(64),
            forgejo_owner: "owner".to_string(),
            forgejo_repo: "repo".to_string(),
            identifier: "repo".to_string(),
        }
    }

    #[test]
    fn request_rejects_empty_required_fields() {
        let mut request = valid_request();
        request.relay_url = "".to_string();
        assert!(request.decode().is_err());
    }

    #[test]
    fn request_rejects_empty_forgejo_fields() {
        let mut request = valid_request();
        request.forgejo_owner = " ".to_string();
        assert!(request.decode().is_err());

        request = valid_request();
        request.forgejo_repo = " ".to_string();
        assert!(request.decode().is_err());

        request = valid_request();
        request.identifier = " ".to_string();
        assert!(request.decode().is_err());
    }

    #[test]
    fn status_parses_expected_values() {
        assert_eq!(
            RelayPublishStatus::parse("pending").unwrap(),
            RelayPublishStatus::Pending
        );
        assert_eq!(
            RelayPublishStatus::parse("publishing").unwrap(),
            RelayPublishStatus::Publishing
        );
        assert_eq!(
            RelayPublishStatus::parse("published").unwrap(),
            RelayPublishStatus::Published
        );
        assert!(RelayPublishStatus::parse("other").is_err());
    }

    #[test]
    fn request_rejects_invalid_hex_lengths() {
        let mut request = valid_request();
        request.event_id = "aa".repeat(31);
        assert!(request.decode().is_err());

        request = valid_request();
        request.pubkey = "bb".repeat(31);
        assert!(request.decode().is_err());
    }

    #[test]
    fn request_rejects_invalid_hex_payloads() {
        let mut request = valid_request();
        request.sig = format!("{}z", "a".repeat(127));
        assert!(request.decode().is_err());
    }
}
