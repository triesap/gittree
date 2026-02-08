#![forbid(unsafe_code)]

use bech32::{Bech32, Hrp};
use serde::{Deserialize, Serialize};

mod nip98;

pub use nip98::{
    nip98_event_id,
    nip98_payload_hash,
    nip98_sign_event,
    nip98_unsigned_event,
    Nip98Event,
    Nip98UnsignedEvent,
    NIP98_KIND,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoListItem {
    pub npub: String,
    pub identifier: String,
    pub forgejo: String,
    pub clone_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoDetail {
    pub npub: String,
    pub identifier: String,
    pub forgejo: String,
    pub clone_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoListResponse {
    pub items: Vec<RepoListItem>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProfileVisibility {
    Private,
    Public,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Profile {
    pub pubkey: String,
    pub username: String,
    pub display_name: Option<String>,
    pub bio: Option<String>,
    pub avatar_url: Option<String>,
    pub website_url: Option<String>,
    pub location: Option<String>,
    pub visibility: ProfileVisibility,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ProfileUpdate {
    pub display_name: Option<String>,
    pub bio: Option<String>,
    pub avatar_url: Option<String>,
    pub website_url: Option<String>,
    pub location: Option<String>,
    pub visibility: Option<ProfileVisibility>,
}

impl RepoListItem {
    pub fn new(npub: String, identifier: String, forgejo: String, clone_url: String) -> Self {
        Self {
            npub,
            identifier,
            forgejo,
            clone_url,
        }
    }
}

impl RepoDetail {
    pub fn new(npub: String, identifier: String, forgejo: String, clone_url: String) -> Self {
        Self {
            npub,
            identifier,
            forgejo,
            clone_url,
        }
    }
}

impl From<RepoListItem> for RepoDetail {
    fn from(value: RepoListItem) -> Self {
        Self::new(value.npub, value.identifier, value.forgejo, value.clone_url)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppCoreError {
    InvalidPubkey,
    InvalidSecretKey,
    InvalidEventEncoding(String),
    InvalidSignature,
}

impl std::fmt::Display for AppCoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AppCoreError::InvalidPubkey => write!(f, "invalid pubkey"),
            AppCoreError::InvalidSecretKey => write!(f, "invalid secret key"),
            AppCoreError::InvalidEventEncoding(message) => {
                write!(f, "invalid event encoding: {message}")
            }
            AppCoreError::InvalidSignature => write!(f, "invalid signature"),
        }
    }
}

impl std::error::Error for AppCoreError {}

pub fn normalize_identifier(identifier: &str) -> &str {
    identifier.strip_suffix(".git").unwrap_or(identifier)
}

pub fn clone_url(public_git_url: &str, npub: &str, identifier: &str) -> String {
    format!(
        "{}/{npub}/{}.git",
        public_git_url.trim_end_matches('/'),
        identifier
    )
}

pub fn npub_from_bytes(bytes: &[u8]) -> Result<String, AppCoreError> {
    let hrp = Hrp::parse("npub").map_err(|_| AppCoreError::InvalidPubkey)?;
    bech32::encode::<Bech32>(hrp, bytes).map_err(|_| AppCoreError::InvalidPubkey)
}

pub fn pubkey_bytes_from_npub(npub: &str) -> Result<Vec<u8>, AppCoreError> {
    let (hrp, data) = bech32::decode(npub).map_err(|_| AppCoreError::InvalidPubkey)?;
    if hrp.as_str() != "npub" || data.len() != 32 {
        return Err(AppCoreError::InvalidPubkey);
    }
    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::{
        clone_url, normalize_identifier, npub_from_bytes, pubkey_bytes_from_npub,
        ProfileVisibility,
    };

    #[test]
    fn normalize_identifier_strips_git_suffix() {
        assert_eq!(normalize_identifier("demo.git"), "demo");
        assert_eq!(normalize_identifier("demo"), "demo");
    }

    #[test]
    fn clone_url_trims_trailing_slash() {
        let url = clone_url("http://localhost:8085/", "npub1", "demo");
        assert_eq!(url, "http://localhost:8085/npub1/demo.git");
    }

    #[test]
    fn npub_from_bytes_returns_npub_prefix() {
        let npub = npub_from_bytes(&[0u8; 32]).expect("npub");
        assert!(npub.starts_with("npub1"));
    }

    #[test]
    fn pubkey_bytes_from_npub_round_trips() {
        let bytes = [3u8; 32];
        let npub = npub_from_bytes(&bytes).expect("npub");
        let decoded = pubkey_bytes_from_npub(&npub).expect("decoded");
        assert_eq!(decoded, bytes);
    }

    #[test]
    fn profile_visibility_serializes_to_strings() {
        let json = serde_json::to_string(&ProfileVisibility::Private).expect("json");
        assert_eq!(json, "\"private\"");
    }
}
