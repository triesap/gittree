use crate::StorageError;
use gittree_core::RepoMapping;

const HEX_LEN: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoMappingRecord {
    pub forgejo_owner: String,
    pub forgejo_repo: String,
    pub pubkey: Vec<u8>,
    pub identifier: String,
}

impl RepoMappingRecord {
    pub fn new(mapping: &RepoMapping) -> Result<Self, StorageError> {
        Ok(Self {
            forgejo_owner: mapping.forgejo.owner.clone(),
            forgejo_repo: mapping.forgejo.name.clone(),
            pubkey: decode_hex_32("pubkey", &mapping.pubkey)?,
            identifier: mapping.identifier.clone(),
        })
    }

    pub fn forgejo_full_name(&self) -> String {
        format!("{}/{}", self.forgejo_owner, self.forgejo_repo)
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
    use super::RepoMappingRecord;
    use gittree_core::RepoMapping;

    #[test]
    fn mapping_record_maps_fields() {
        let mapping = RepoMapping::new(
            "owner",
            "repo",
            "11".repeat(32),
            "repo",
        )
        .expect("mapping");
        let record = RepoMappingRecord::new(&mapping).expect("record");
        assert_eq!(record.forgejo_owner, "owner");
        assert_eq!(record.forgejo_repo, "repo");
        assert_eq!(record.identifier, "repo");
        assert_eq!(record.pubkey.len(), 32);
        assert_eq!(record.forgejo_full_name(), "owner/repo");
    }

    #[test]
    fn mapping_record_rejects_invalid_pubkey() {
        let mut mapping = RepoMapping::new(
            "owner",
            "repo",
            "11".repeat(32),
            "repo",
        )
        .expect("mapping");
        mapping.pubkey = "bad".to_string();
        assert!(RepoMappingRecord::new(&mapping).is_err());
    }

    #[test]
    fn mapping_record_rejects_non_hex_pubkey_with_expected_length() {
        let mut mapping = RepoMapping::new(
            "owner",
            "repo",
            "11".repeat(32),
            "repo",
        )
        .expect("mapping");
        mapping.pubkey = "zz".repeat(32);
        let err = RepoMappingRecord::new(&mapping).expect_err("non-hex must fail");
        assert!(matches!(
            err,
            crate::StorageError::InvalidHex { field: "pubkey", .. }
        ));
    }
}
