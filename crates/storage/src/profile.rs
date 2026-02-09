use crate::StorageError;

const HEX_LEN: usize = 64;
const MAX_DISPLAY_NAME: usize = 80;
const MAX_BIO: usize = 500;
const MAX_AVATAR_URL: usize = 300;
const MAX_WEBSITE_URL: usize = 300;
const MAX_LOCATION: usize = 120;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileVisibility {
    Private,
    Public,
}

impl ProfileVisibility {
    pub fn as_str(&self) -> &'static str {
        match self {
            ProfileVisibility::Private => "private",
            ProfileVisibility::Public => "public",
        }
    }

    pub fn parse(value: &str) -> Result<Self, StorageError> {
        match value.trim().to_ascii_lowercase().as_str() {
            "private" => Ok(ProfileVisibility::Private),
            "public" => Ok(ProfileVisibility::Public),
            _ => Err(StorageError::InvalidField {
                field: "visibility",
                value: value.to_string(),
            }),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileRecord {
    pub pubkey: Vec<u8>,
    pub display_name: Option<String>,
    pub bio: Option<String>,
    pub avatar_url: Option<String>,
    pub website_url: Option<String>,
    pub location: Option<String>,
    pub visibility: ProfileVisibility,
    pub created_at: i64,
    pub updated_at: i64,
}

impl ProfileRecord {
    pub fn new(
        pubkey: &str,
        display_name: Option<String>,
        bio: Option<String>,
        avatar_url: Option<String>,
        website_url: Option<String>,
        location: Option<String>,
        visibility: ProfileVisibility,
        created_at: i64,
        updated_at: i64,
    ) -> Result<Self, StorageError> {
        if updated_at < created_at {
            return Err(StorageError::InvalidField {
                field: "updated_at",
                value: updated_at.to_string(),
            });
        }

        Ok(Self {
            pubkey: decode_hex_32("pubkey", pubkey)?,
            display_name: normalize_optional("display_name", display_name, MAX_DISPLAY_NAME)?,
            bio: normalize_optional("bio", bio, MAX_BIO)?,
            avatar_url: normalize_optional_url("avatar_url", avatar_url, MAX_AVATAR_URL)?,
            website_url: normalize_optional_url("website_url", website_url, MAX_WEBSITE_URL)?,
            location: normalize_optional("location", location, MAX_LOCATION)?,
            visibility,
            created_at,
            updated_at,
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
    use super::{ProfileRecord, ProfileVisibility, MAX_DISPLAY_NAME};
    use crate::StorageError;

    #[test]
    fn profile_record_maps_fields() {
        let record = ProfileRecord::new(
            &"11".repeat(32),
            Some("  Ada ".to_string()),
            Some(" builder ".to_string()),
            Some("https://example.com/avatar.png".to_string()),
            Some("https://example.com".to_string()),
            Some(" Earth ".to_string()),
            ProfileVisibility::Private,
            10,
            20,
        )
        .expect("record");
        assert_eq!(record.display_name.as_deref(), Some("Ada"));
        assert_eq!(record.bio.as_deref(), Some("builder"));
        assert_eq!(record.location.as_deref(), Some("Earth"));
        assert_eq!(record.visibility, ProfileVisibility::Private);
    }

    #[test]
    fn profile_record_rejects_invalid_pubkey() {
        assert!(ProfileRecord::new(
            "bad",
            None,
            None,
            None,
            None,
            None,
            ProfileVisibility::Private,
            10,
            20,
        )
        .is_err());
    }

    #[test]
    fn profile_record_rejects_empty_values() {
        let record = ProfileRecord::new(
            &"11".repeat(32),
            Some(" ".to_string()),
            None,
            None,
            None,
            Some("".to_string()),
            ProfileVisibility::Private,
            10,
            20,
        )
        .expect("record");
        assert!(record.display_name.is_none());
        assert!(record.location.is_none());
    }

    #[test]
    fn profile_record_rejects_long_display_name() {
        let display_name = "a".repeat(MAX_DISPLAY_NAME + 1);
        let err = ProfileRecord::new(
            &"11".repeat(32),
            Some(display_name.clone()),
            None,
            None,
            None,
            None,
            ProfileVisibility::Private,
            10,
            20,
        )
        .unwrap_err();
        assert!(format!("{err}").contains("display_name"));
    }

    #[test]
    fn profile_visibility_parses_values() {
        assert_eq!(
            super::ProfileVisibility::parse("private").expect("private"),
            ProfileVisibility::Private
        );
        assert_eq!(
            super::ProfileVisibility::parse("PUBLIC").expect("public"),
            ProfileVisibility::Public
        );
    }

    #[test]
    fn profile_record_rejects_invalid_avatar_url_scheme() {
        let err = ProfileRecord::new(
            &"11".repeat(32),
            None,
            None,
            Some("ftp://example.com/avatar.png".to_string()),
            None,
            None,
            ProfileVisibility::Private,
            10,
            20,
        )
        .unwrap_err();
        assert!(matches!(err, StorageError::InvalidField { field, .. } if field == "avatar_url"));
    }

    #[test]
    fn profile_record_rejects_invalid_website_url_scheme() {
        let err = ProfileRecord::new(
            &"11".repeat(32),
            None,
            None,
            None,
            Some("gopher://example.com".to_string()),
            None,
            ProfileVisibility::Private,
            10,
            20,
        )
        .unwrap_err();
        assert!(matches!(err, StorageError::InvalidField { field, .. } if field == "website_url"));
    }
}
