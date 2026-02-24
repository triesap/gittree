#![forbid(unsafe_code)]

use bech32::{Bech32, Hrp};
use serde::{Deserialize, Serialize};

mod nip98;

pub use nip98::{
    NIP98_KIND, Nip98Event, Nip98UnsignedEvent, nip98_event_id, nip98_payload_hash,
    nip98_sign_event, nip98_unsigned_event,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedNostrEvent {
    pub id: String,
    pub pubkey: String,
    pub created_at: i64,
    pub kind: u32,
    pub tags: Vec<Vec<String>>,
    pub content: String,
    pub sig: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoCreateRequest {
    pub event: SignedNostrEvent,
    pub private: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoCreateResponse {
    pub owner: String,
    pub name: String,
    pub html_url: Option<String>,
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
    let hrp = Hrp::parse("npub").expect("npub hrp is a valid bech32 human-readable prefix");
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
        AppCoreError, ProfileVisibility, RepoCreateRequest, RepoCreateResponse, RepoDetail,
        RepoListItem, SignedNostrEvent, clone_url, normalize_identifier, npub_from_bytes,
        pubkey_bytes_from_npub,
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
    fn repo_create_request_round_trips() {
        let event = SignedNostrEvent {
            id: "11".repeat(32),
            pubkey: "22".repeat(32),
            created_at: 1_700_000_000,
            kind: 30_617,
            tags: vec![vec!["d".to_string(), "demo".to_string()]],
            content: String::new(),
            sig: "33".repeat(64),
        };
        let request = RepoCreateRequest {
            event,
            private: Some(true),
        };
        let json = serde_json::to_string(&request).expect("json");
        let decoded: RepoCreateRequest = serde_json::from_str(&json).expect("decode");
        assert_eq!(request, decoded);
    }

    #[test]
    fn repo_create_response_round_trips() {
        let response = RepoCreateResponse {
            owner: "alice".to_string(),
            name: "demo".to_string(),
            html_url: Some("http://localhost/demo".to_string()),
        };
        let json = serde_json::to_string(&response).expect("json");
        let decoded: RepoCreateResponse = serde_json::from_str(&json).expect("decode");
        assert_eq!(response, decoded);
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

    #[test]
    fn repo_list_item_and_detail_constructors_round_trip() {
        let item = RepoListItem::new(
            "npub1demo".to_string(),
            "demo".to_string(),
            "alice/demo".to_string(),
            "https://gittr.ee/npub1demo/demo.git".to_string(),
        );
        let detail = RepoDetail::from(item.clone());
        assert_eq!(
            detail,
            RepoDetail::new(item.npub, item.identifier, item.forgejo, item.clone_url)
        );
    }

    #[test]
    fn app_core_error_display_is_stable() {
        let invalid_pubkey = AppCoreError::InvalidPubkey;
        assert_eq!(invalid_pubkey.to_string(), "invalid pubkey");

        let invalid_secret = AppCoreError::InvalidSecretKey;
        assert_eq!(invalid_secret.to_string(), "invalid secret key");

        let invalid_encoding = AppCoreError::InvalidEventEncoding("bad json".to_string());
        assert_eq!(
            invalid_encoding.to_string(),
            "invalid event encoding: bad json"
        );

        let invalid_sig = AppCoreError::InvalidSignature;
        assert_eq!(invalid_sig.to_string(), "invalid signature");
    }

    #[test]
    fn pubkey_bytes_from_npub_rejects_wrong_payload_length() {
        let short = npub_from_bytes(&[9u8; 31]).expect("npub");
        let err = pubkey_bytes_from_npub(&short).expect_err("invalid length");
        assert_eq!(err.to_string(), "invalid pubkey");
    }

    #[test]
    fn pubkey_bytes_from_npub_rejects_wrong_hrp() {
        let npub = npub_from_bytes(&[7u8; 32]).expect("npub");
        let nsec = npub.replacen("npub1", "nsec1", 1);
        let err = pubkey_bytes_from_npub(&nsec).expect_err("invalid hrp");
        assert_eq!(err.to_string(), "invalid pubkey");
    }
}
