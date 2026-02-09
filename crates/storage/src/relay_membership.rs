use crate::StorageError;

const HEX_32_LEN: usize = 64;
const MAX_ROLE_LEN: usize = 40;
const MAX_STATUS_LEN: usize = 40;
const MAX_INVITE_CODE_LEN: usize = 120;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayMembershipRecord {
    pub tenant_id: String,
    pub pubkey: Vec<u8>,
    pub role: String,
    pub status: String,
    pub created_at: i64,
    pub updated_at: i64,
}

impl RelayMembershipRecord {
    pub fn new(
        tenant_id: impl Into<String>,
        pubkey: &str,
        role: impl Into<String>,
        status: impl Into<String>,
        created_at: i64,
        updated_at: i64,
    ) -> Result<Self, StorageError> {
        let tenant_id = tenant_id.into();
        let role = role.into();
        let status = status.into();

        if tenant_id.trim().is_empty() {
            return Err(StorageError::InvalidField {
                field: "tenant_id",
                value: "empty".to_string(),
            });
        }
        if role.trim().is_empty() || role.len() > MAX_ROLE_LEN {
            return Err(StorageError::InvalidField { field: "role", value: role });
        }
        if status.trim().is_empty() || status.len() > MAX_STATUS_LEN {
            return Err(StorageError::InvalidField {
                field: "status",
                value: status,
            });
        }
        if updated_at < created_at {
            return Err(StorageError::InvalidField {
                field: "updated_at",
                value: updated_at.to_string(),
            });
        }

        Ok(Self {
            tenant_id,
            pubkey: decode_hex_32("pubkey", pubkey)?,
            role,
            status,
            created_at,
            updated_at,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayInviteRecord {
    pub tenant_id: String,
    pub invite_code: String,
    pub role: String,
    pub inviter_pubkey: Vec<u8>,
    pub invitee_pubkey: Option<Vec<u8>>,
    pub expires_at: Option<i64>,
    pub created_at: i64,
}

impl RelayInviteRecord {
    pub fn new(
        tenant_id: impl Into<String>,
        invite_code: impl Into<String>,
        role: impl Into<String>,
        inviter_pubkey: &str,
        invitee_pubkey: Option<&str>,
        expires_at: Option<i64>,
        created_at: i64,
    ) -> Result<Self, StorageError> {
        let tenant_id = tenant_id.into();
        let invite_code = invite_code.into();
        let role = role.into();

        if tenant_id.trim().is_empty() {
            return Err(StorageError::InvalidField {
                field: "tenant_id",
                value: "empty".to_string(),
            });
        }
        if invite_code.trim().is_empty() || invite_code.len() > MAX_INVITE_CODE_LEN {
            return Err(StorageError::InvalidField {
                field: "invite_code",
                value: invite_code,
            });
        }
        if role.trim().is_empty() || role.len() > MAX_ROLE_LEN {
            return Err(StorageError::InvalidField { field: "role", value: role });
        }
        if let Some(expires_at) = expires_at {
            if expires_at < created_at {
                return Err(StorageError::InvalidField {
                    field: "expires_at",
                    value: expires_at.to_string(),
                });
            }
        }

        Ok(Self {
            tenant_id,
            invite_code,
            role,
            inviter_pubkey: decode_hex_32("inviter_pubkey", inviter_pubkey)?,
            invitee_pubkey: match invitee_pubkey {
                Some(pubkey) => Some(decode_hex_32("invitee_pubkey", pubkey)?),
                None => None,
            },
            expires_at,
            created_at,
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

#[cfg(test)]
mod tests {
    use super::{RelayInviteRecord, RelayMembershipRecord};

    #[test]
    fn membership_record_maps_fields() {
        let record = RelayMembershipRecord::new(
            "tenant",
            &"11".repeat(32),
            "member",
            "active",
            10,
            20,
        )
        .expect("record");
        assert_eq!(record.tenant_id, "tenant");
        assert_eq!(record.role, "member");
        assert_eq!(record.status, "active");
    }

    #[test]
    fn invite_record_maps_fields() {
        let record = RelayInviteRecord::new(
            "tenant",
            "invite",
            "member",
            &"22".repeat(32),
            Some(&"33".repeat(32)),
            Some(50),
            10,
        )
        .expect("record");
        assert_eq!(record.invite_code, "invite");
        assert_eq!(record.role, "member");
        assert_eq!(record.expires_at, Some(50));
    }
}
