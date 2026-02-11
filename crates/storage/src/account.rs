use crate::StorageError;

const HEX_LEN: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountRecord {
    pub pubkey: Vec<u8>,
    pub forgejo_username: String,
}

impl AccountRecord {
    pub fn new(pubkey: &str, forgejo_username: impl Into<String>) -> Result<Self, StorageError> {
        let forgejo_username = forgejo_username.into();
        if forgejo_username.trim().is_empty() {
            return Err(StorageError::InvalidField {
                field: "forgejo_username",
                value: forgejo_username,
            });
        }
        Ok(Self {
            pubkey: decode_hex_32("pubkey", pubkey)?,
            forgejo_username,
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
    use super::AccountRecord;

    #[test]
    fn account_record_maps_fields() {
        let record = AccountRecord::new("11".repeat(32).as_str(), "alice").expect("record");
        assert_eq!(record.forgejo_username, "alice");
        assert_eq!(record.pubkey.len(), 32);
    }

    #[test]
    fn account_record_rejects_empty_username() {
        let err = AccountRecord::new("11".repeat(32).as_str(), " ").unwrap_err();
        assert!(format!("{err}").contains("forgejo_username"));
    }

    #[test]
    fn account_record_rejects_invalid_pubkey() {
        assert!(AccountRecord::new("bad", "alice").is_err());
    }

    #[test]
    fn account_record_rejects_non_hex_pubkey_with_expected_length() {
        let invalid = format!("{}z", "a".repeat(63));
        assert!(AccountRecord::new(&invalid, "alice").is_err());
    }
}
