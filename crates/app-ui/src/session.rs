use gittree_app_core::npub_from_bytes;
use serde::{Deserialize, Serialize};
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::JsValue;
#[cfg(target_arch = "wasm32")]
use web_sys::{Storage, Window};

#[cfg(target_arch = "wasm32")]
const SESSION_STORAGE_KEY: &str = "gittree_auth_session";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AuthSource {
    Nip07,
    Local,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthSession {
    pub pubkey: String,
    pub npub: String,
    pub source: AuthSource,
}

#[derive(Debug)]
pub enum SessionError {
    InvalidPubkey,
    Serialization(String),
    #[cfg(target_arch = "wasm32")]
    MissingWindow,
    #[cfg(target_arch = "wasm32")]
    MissingStorage,
    #[cfg(target_arch = "wasm32")]
    Js(String),
}

impl std::fmt::Display for SessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SessionError::InvalidPubkey => write!(f, "invalid pubkey"),
            SessionError::Serialization(message) => write!(f, "serialization error: {message}"),
            #[cfg(target_arch = "wasm32")]
            SessionError::MissingWindow => write!(f, "missing window"),
            #[cfg(target_arch = "wasm32")]
            SessionError::MissingStorage => write!(f, "missing storage"),
            #[cfg(target_arch = "wasm32")]
            SessionError::Js(message) => write!(f, "js error: {message}"),
        }
    }
}

impl std::error::Error for SessionError {}

impl AuthSession {
    pub fn from_pubkey_hex(pubkey: &str, source: AuthSource) -> Result<Self, SessionError> {
        let normalized = pubkey.trim().to_lowercase();
        let bytes = parse_pubkey_bytes(&normalized)?;
        let npub = npub_from_bytes(&bytes).map_err(|_| SessionError::InvalidPubkey)?;
        Ok(Self {
            pubkey: normalized,
            npub,
            source,
        })
    }

    pub fn to_json_string(&self) -> Result<String, SessionError> {
        serde_json::to_string(self).map_err(|err| SessionError::Serialization(err.to_string()))
    }
}

pub fn store_session(session: &AuthSession) -> Result<(), SessionError> {
    let payload = session.to_json_string()?;
    store_payload(&payload)
}

#[cfg(target_arch = "wasm32")]
fn store_payload(payload: &str) -> Result<(), SessionError> {
    let storage = local_storage()?;
    storage
        .set_item(SESSION_STORAGE_KEY, payload)
        .map_err(|err| SessionError::Js(js_error(err)))
}

#[cfg(not(target_arch = "wasm32"))]
fn store_payload(_payload: &str) -> Result<(), SessionError> {
    Ok(())
}

fn parse_pubkey_bytes(value: &str) -> Result<[u8; 32], SessionError> {
    let bytes = hex::decode(value).map_err(|_| SessionError::InvalidPubkey)?;
    if bytes.len() != 32 {
        return Err(SessionError::InvalidPubkey);
    }
    let mut output = [0u8; 32];
    output.copy_from_slice(&bytes);
    Ok(output)
}

#[cfg(target_arch = "wasm32")]
fn local_storage() -> Result<Storage, SessionError> {
    let window = window_ref()?;
    window
        .local_storage()
        .map_err(|err| SessionError::Js(js_error(err)))?
        .ok_or(SessionError::MissingStorage)
}

#[cfg(target_arch = "wasm32")]
fn window_ref() -> Result<Window, SessionError> {
    web_sys::window().ok_or(SessionError::MissingWindow)
}

#[cfg(target_arch = "wasm32")]
fn js_error(value: JsValue) -> String {
    value
        .as_string()
        .unwrap_or_else(|| format!("{:?}", value))
}

#[cfg(test)]
mod tests {
    use super::{AuthSession, AuthSource, SessionError};

    #[test]
    fn session_from_pubkey_hex_builds_npub() {
        let session = AuthSession::from_pubkey_hex(&"11".repeat(32), AuthSource::Local)
            .expect("session");
        assert_eq!(session.pubkey, "11".repeat(32));
        assert!(session.npub.starts_with("npub1"));
        assert_eq!(session.source, AuthSource::Local);
    }

    #[test]
    fn session_from_pubkey_hex_rejects_invalid_hex() {
        let result = AuthSession::from_pubkey_hex("zz", AuthSource::Nip07);
        assert!(matches!(result, Err(SessionError::InvalidPubkey)));
    }

    #[test]
    fn session_serializes_to_json() {
        let session = AuthSession::from_pubkey_hex(&"22".repeat(32), AuthSource::Nip07)
            .expect("session");
        let json = session.to_json_string().expect("json");
        assert!(json.contains("\"pubkey\""));
        assert!(json.contains("\"npub\""));
        assert!(json.contains("\"source\""));
    }
}
