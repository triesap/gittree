use crate::{CoreError, Result};

pub const REPO_ANNOUNCEMENT_KIND: &str = "30617";
pub const HEX_EVENT_ID_LEN: usize = 64;
pub const HEX_PUBKEY_LEN: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoAddress {
    pub pubkey: String,
    pub identifier: String,
}

impl RepoAddress {
    pub fn new(pubkey: impl Into<String>, identifier: impl Into<String>) -> Result<Self> {
        let pubkey = pubkey.into();
        let identifier = identifier.into();

        if pubkey.is_empty() || !is_hex_len(&pubkey, HEX_PUBKEY_LEN) || identifier.is_empty() {
            return Err(CoreError::InvalidField {
                field: "a",
                value: format!("{REPO_ANNOUNCEMENT_KIND}:{pubkey}:{identifier}"),
            });
        }

        Ok(Self { pubkey, identifier })
    }

    pub fn parse(value: &str) -> Result<Self> {
        let mut parts = value.splitn(4, ':');
        let kind = parts.next().unwrap_or("");
        let pubkey = parts.next().unwrap_or("");
        let identifier = parts.next().unwrap_or("");
        let extra = parts.next();

        if kind != REPO_ANNOUNCEMENT_KIND
            || pubkey.is_empty()
            || identifier.is_empty()
            || extra.is_some()
        {
            return Err(CoreError::InvalidField {
                field: "a",
                value: value.to_string(),
            });
        }

        Self::new(pubkey, identifier)
    }
}

impl std::fmt::Display for RepoAddress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{REPO_ANNOUNCEMENT_KIND}:{}:{}",
            self.pubkey, self.identifier
        )
    }
}

pub fn validate_repo_address(value: &str) -> Result<()> {
    RepoAddress::parse(value).map(|_| ())
}

pub fn is_hex_hash(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.chars().all(|c| c.is_ascii_hexdigit())
}

pub fn is_hex_len(value: &str, len: usize) -> bool {
    value.len() == len && value.chars().all(|c| c.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::CoreError;
    use super::RepoAddress;
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
    fn parses_repo_address() {
        let pubkey = "11".repeat(32);
        let value = format!("30617:{pubkey}:repo");
        let address = RepoAddress::parse(&value).expect("parse");
        assert_eq!(address.pubkey, pubkey);
        assert_eq!(address.identifier, "repo");
        assert_eq!(address.to_string(), value);
    }

    #[test]
    fn repo_address_new_rejects_invalid_components() {
        assert!(matches!(
            RepoAddress::new("", "repo"),
            Err(CoreError::InvalidField { field: "a", .. })
        ));

        assert!(matches!(
            RepoAddress::new("zz".repeat(32), "repo"),
            Err(CoreError::InvalidField { field: "a", .. })
        ));

        assert!(matches!(
            RepoAddress::new("11".repeat(32), ""),
            Err(CoreError::InvalidField { field: "a", .. })
        ));
    }

    #[test]
    fn repo_address_parse_rejects_invalid_shapes_and_segments() {
        let pubkey = "11".repeat(32);
        let cases = [
            format!("1:{pubkey}:repo"),
            format!("30617::{pubkey}"),
            format!("30617:{pubkey}:"),
            format!("30617:{pubkey}:repo:extra"),
            format!("30617:{}:repo", "11".repeat(31)),
            format!("30617:{}:repo", "zz".repeat(32)),
        ];
        for value in cases {
            assert!(RepoAddress::parse(&value).is_err());
            assert!(validate_repo_address(&value).is_err());
        }
    }

    #[test]
    fn hex_hash_accepts_40_and_64() {
        assert!(is_hex_hash(&"11".repeat(20)));
        assert!(is_hex_hash(&"11".repeat(32)));
        assert!(!is_hex_hash("11"));
        assert!(!is_hex_hash(&"gg".repeat(20)));
        assert!(!is_hex_hash(&"gg".repeat(32)));
    }

    #[test]
    fn hex_len_checks_length() {
        assert!(is_hex_len(&"aa".repeat(32), 64));
        assert!(!is_hex_len(&"aa".repeat(31), 64));
        assert!(!is_hex_len(&"gg".repeat(32), 64));
    }
}
