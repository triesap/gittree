use crate::{CoreError, Result};

pub const REPO_ANNOUNCEMENT_KIND: &str = "30617";
pub const HEX_EVENT_ID_LEN: usize = 64;
pub const HEX_PUBKEY_LEN: usize = 64;

pub fn validate_repo_address(value: &str) -> Result<()> {
    let mut parts = value.split(':');
    let kind = parts.next().unwrap_or("");
    let pubkey = parts.next().unwrap_or("");
    let identifier = parts.next().unwrap_or("");

    if kind != REPO_ANNOUNCEMENT_KIND
        || pubkey.is_empty()
        || identifier.is_empty()
        || parts.next().is_some()
    {
        return Err(CoreError::InvalidField {
            field: "a",
            value: value.to_string(),
        });
    }

    if !is_hex_len(pubkey, HEX_PUBKEY_LEN) {
        return Err(CoreError::InvalidField {
            field: "a",
            value: value.to_string(),
        });
    }

    Ok(())
}

pub fn is_hex_hash(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.chars().all(|c| c.is_ascii_hexdigit())
}

pub fn is_hex_len(value: &str, len: usize) -> bool {
    value.len() == len && value.chars().all(|c| c.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::is_hex_hash;
    use super::is_hex_len;
    use super::validate_repo_address;

    #[test]
    fn validates_repo_address_format() {
        let pubkey = "11".repeat(32);
        let value = format!("30617:{pubkey}:repo");
        validate_repo_address(&value).expect("valid");
    }

    #[test]
    fn rejects_bad_repo_address_format() {
        let value = "30617:missing".to_string();
        assert!(validate_repo_address(&value).is_err());
    }

    #[test]
    fn hex_hash_accepts_40_and_64() {
        assert!(is_hex_hash(&"11".repeat(20)));
        assert!(is_hex_hash(&"11".repeat(32)));
        assert!(!is_hex_hash("11"));
    }

    #[test]
    fn hex_len_checks_length() {
        assert!(is_hex_len(&"aa".repeat(32), 64));
        assert!(!is_hex_len(&"aa".repeat(31), 64));
    }
}
