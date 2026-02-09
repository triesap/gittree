use crate::StorageError;

const HEX_32_LEN: usize = 64;
const MAX_HOST_LEN: usize = 255;
const MAX_NAME_LEN: usize = 120;
const MAX_DESC_LEN: usize = 500;
const MAX_URL_LEN: usize = 300;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayTenantRecord {
    pub id: String,
    pub host: String,
    pub relay_pubkey: Vec<u8>,
    pub relay_secret: Vec<u8>,
    pub relay_secret_nonce: Vec<u8>,
    pub relay_secret_kid: String,
    pub name: Option<String>,
    pub description: Option<String>,
    pub icon: Option<String>,
    pub banner: Option<String>,
    pub contact: Option<String>,
    pub auth_required: bool,
    pub public_read: bool,
    pub public_write: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

impl RelayTenantRecord {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: impl Into<String>,
        host: impl Into<String>,
        relay_pubkey: &str,
        relay_secret: Vec<u8>,
        relay_secret_nonce: Vec<u8>,
        relay_secret_kid: impl Into<String>,
        name: Option<String>,
        description: Option<String>,
        icon: Option<String>,
        banner: Option<String>,
        contact: Option<String>,
        auth_required: bool,
        public_read: bool,
        public_write: bool,
        created_at: i64,
        updated_at: i64,
    ) -> Result<Self, StorageError> {
        let id = id.into();
        let host = host.into().trim().to_ascii_lowercase();
        let relay_secret_kid = relay_secret_kid.into();

        if id.trim().is_empty() {
            return Err(StorageError::InvalidField {
                field: "tenant_id",
                value: "empty".to_string(),
            });
        }
        if host.is_empty() || host.len() > MAX_HOST_LEN || host.contains(' ') {
            return Err(StorageError::InvalidField {
                field: "host",
                value: host,
            });
        }
        if relay_secret.is_empty() {
            return Err(StorageError::InvalidField {
                field: "relay_secret",
                value: "empty".to_string(),
            });
        }
        if relay_secret_nonce.is_empty() {
            return Err(StorageError::InvalidField {
                field: "relay_secret_nonce",
                value: "empty".to_string(),
            });
        }
        if relay_secret_kid.trim().is_empty() {
            return Err(StorageError::InvalidField {
                field: "relay_secret_kid",
                value: relay_secret_kid,
            });
        }
        if updated_at < created_at {
            return Err(StorageError::InvalidField {
                field: "updated_at",
                value: updated_at.to_string(),
            });
        }

        Ok(Self {
            id,
            host,
            relay_pubkey: decode_hex_32("relay_pubkey", relay_pubkey)?,
            relay_secret,
            relay_secret_nonce,
            relay_secret_kid,
            name: normalize_optional("name", name, MAX_NAME_LEN)?,
            description: normalize_optional("description", description, MAX_DESC_LEN)?,
            icon: normalize_optional_url("icon", icon, MAX_URL_LEN)?,
            banner: normalize_optional_url("banner", banner, MAX_URL_LEN)?,
            contact: normalize_optional("contact", contact, MAX_URL_LEN)?,
            auth_required,
            public_read,
            public_write,
            created_at,
            updated_at,
        })
    }
}

fn decode_hex_32(field: &'static str, value: &str) -> Result<Vec<u8>, StorageError> {
    if value.len() != HEX_32_LEN {
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

fn normalize_optional(
    field: &'static str,
    value: Option<String>,
    max_len: usize,
) -> Result<Option<String>, StorageError> {
    let value = value.map(|value| value.trim().to_string());
    match value {
        None => Ok(None),
        Some(value) => {
            if value.is_empty() {
                return Ok(None);
            }
            if value.len() > max_len {
                return Err(StorageError::InvalidField { field, value });
            }
            Ok(Some(value))
        }
    }
}

fn normalize_optional_url(
    field: &'static str,
    value: Option<String>,
    max_len: usize,
) -> Result<Option<String>, StorageError> {
    let value = normalize_optional(field, value, max_len)?;
    if let Some(url) = value.as_ref() {
        let lower = url.to_ascii_lowercase();
        if !(lower.starts_with("http://") || lower.starts_with("https://")) {
            return Err(StorageError::InvalidField {
                field,
                value: url.clone(),
            });
        }
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::RelayTenantRecord;

    #[test]
    fn tenant_record_maps_fields() {
        let record = RelayTenantRecord::new(
            "tenant-1",
            "org.relay.gittr.ee",
            &"11".repeat(32),
            vec![1, 2, 3],
            vec![4, 5, 6],
            "v1",
            Some("Org Relay".to_string()),
            None,
            Some("https://example.com/icon.png".to_string()),
            None,
            Some("support".to_string()),
            true,
            false,
            false,
            10,
            10,
        )
        .expect("record");
        assert_eq!(record.host, "org.relay.gittr.ee");
        assert_eq!(record.name.as_deref(), Some("Org Relay"));
        assert_eq!(record.contact.as_deref(), Some("support"));
        assert!(record.auth_required);
    }

    #[test]
    fn tenant_record_rejects_empty_host() {
        assert!(RelayTenantRecord::new(
            "tenant",
            " ",
            &"11".repeat(32),
            vec![1],
            vec![2],
            "v1",
            None,
            None,
            None,
            None,
            None,
            true,
            false,
            false,
            1,
            1,
        )
        .is_err());
    }

    #[test]
    fn tenant_record_rejects_invalid_pubkey() {
        assert!(RelayTenantRecord::new(
            "tenant",
            "org.relay",
            "bad",
            vec![1],
            vec![2],
            "v1",
            None,
            None,
            None,
            None,
            None,
            true,
            false,
            false,
            1,
            1,
        )
        .is_err());
    }
}
