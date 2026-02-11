use crate::StorageError;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RepoFilter {
    pub pubkey: Vec<u8>,
    pub identifier: String,
}

impl RepoFilter {
    pub fn new(pubkey: Vec<u8>, identifier: impl Into<String>) -> Self {
        Self {
            pubkey,
            identifier: identifier.into(),
        }
    }

    pub fn from_hex(pubkey_hex: &str, identifier: impl Into<String>) -> Result<Self, StorageError> {
        let pubkey = hex::decode(pubkey_hex).map_err(|_| StorageError::InvalidHex {
            field: "pubkey",
            value: pubkey_hex.to_string(),
        })?;
        Ok(Self::new(pubkey, identifier))
    }

    pub fn as_parts(&self) -> (&[u8], &str) {
        (&self.pubkey, &self.identifier)
    }
}

#[cfg(test)]
mod tests {
    use super::RepoFilter;
    use crate::StorageError;

    #[test]
    fn repo_filter_from_hex_parses_pubkey() {
        let filter = RepoFilter::from_hex(&"11".repeat(32), "repo").expect("filter");
        assert_eq!(filter.pubkey.len(), 32);
        assert_eq!(filter.identifier, "repo");
    }

    #[test]
    fn repo_filter_rejects_invalid_hex() {
        let err = RepoFilter::from_hex("zz", "repo").unwrap_err();
        assert!(matches!(
            err,
            StorageError::InvalidHex {
                field: "pubkey",
                ..
            }
        ));
    }

    #[test]
    fn repo_filter_as_parts_returns_slices() {
        let filter = RepoFilter::new(vec![1, 2, 3], "repo");
        let (pubkey, identifier) = filter.as_parts();
        assert_eq!(pubkey, &[1, 2, 3]);
        assert_eq!(identifier, "repo");
    }
}
