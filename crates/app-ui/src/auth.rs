use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use gittree_app_core::{AppCoreError, Nip98Event, nip98_sign_event};
#[cfg(target_family = "wasm")]
use gittree_app_core::{Nip98UnsignedEvent, nip98_unsigned_event};
use js_sys::Date;
use k256::schnorr::SigningKey;
#[cfg(target_family = "wasm")]
use nostr::signer::NostrSigner;
#[cfg(target_family = "wasm")]
use nostr::{Kind, PublicKey, Tag, Timestamp, UnsignedEvent};
#[cfg(target_family = "wasm")]
use nostr_browser_signer::{BrowserSigner, Error as Nip07Error};
use wasm_bindgen::prelude::*;
use web_sys::{Storage, Window};

const LOCAL_SECRET_KEY: &str = "gittree_local_secret";

#[derive(Debug)]
pub enum AuthError {
    MissingWindow,
    MissingStorage,
    MissingCrypto,
    InvalidSecretKey,
    MissingNip07,
    #[cfg(target_family = "wasm")]
    Nip07(String),
    EventEncoding(String),
    Js(String),
    Core(AppCoreError),
}

impl std::fmt::Display for AuthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AuthError::MissingWindow => write!(f, "missing window"),
            AuthError::MissingStorage => write!(f, "missing storage"),
            AuthError::MissingCrypto => write!(f, "missing crypto"),
            AuthError::InvalidSecretKey => write!(f, "invalid secret key"),
            AuthError::MissingNip07 => write!(f, "missing nip-07 provider"),
            #[cfg(target_family = "wasm")]
            AuthError::Nip07(message) => write!(f, "nip-07 error: {message}"),
            AuthError::EventEncoding(message) => write!(f, "event encoding error: {message}"),
            AuthError::Js(message) => write!(f, "js error: {message}"),
            AuthError::Core(err) => write!(f, "auth core error: {err}"),
        }
    }
}

impl std::error::Error for AuthError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            AuthError::Core(err) => Some(err),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct LocalKeyMaterial {
    pub pubkey: String,
    pub privkey: String,
}

pub fn unix_timestamp() -> i64 {
    (Date::now() / 1000.0).floor() as i64
}

pub fn auth_header(event: &Nip98Event) -> Result<String, AuthError> {
    let json =
        serde_json::to_vec(event).map_err(|err| AuthError::EventEncoding(err.to_string()))?;
    let token = BASE64_STANDARD.encode(json);
    Ok(format!("Nostr {token}"))
}

pub fn local_key_event(
    method: &str,
    url: &str,
    payload_sha256: Option<&str>,
    created_at: i64,
) -> Result<Nip98Event, AuthError> {
    let secret = local_secret_key()?;
    nip98_sign_event(&secret, method, url, payload_sha256, created_at).map_err(AuthError::Core)
}

pub fn local_secret_key() -> Result<[u8; 32], AuthError> {
    let storage = local_storage()?;
    if let Some(value) = storage
        .get_item(LOCAL_SECRET_KEY)
        .map_err(|err| AuthError::Js(js_error(err)))?
    {
        return parse_secret_hex(&value);
    }

    let secret = generate_secret_key()?;
    storage
        .set_item(LOCAL_SECRET_KEY, &hex::encode(secret))
        .map_err(|err| AuthError::Js(js_error(err)))?;
    Ok(secret)
}

pub fn local_key_material() -> Result<LocalKeyMaterial, AuthError> {
    let secret = local_secret_key()?;
    let pubkey = pubkey_from_secret(&secret)?;
    Ok(LocalKeyMaterial {
        pubkey,
        privkey: hex::encode(secret),
    })
}

#[cfg(target_family = "wasm")]
pub async fn nip07_pubkey() -> Result<String, AuthError> {
    let signer = browser_signer()?;
    let pubkey = signer.get_public_key().await.map_err(nip07_error)?;
    Ok(pubkey.to_hex())
}

#[cfg(not(target_family = "wasm"))]
pub async fn nip07_pubkey() -> Result<String, AuthError> {
    Err(AuthError::MissingNip07)
}

#[cfg(target_family = "wasm")]
pub async fn nip07_sign_nip98(
    pubkey: String,
    method: &str,
    url: &str,
    payload_sha256: Option<&str>,
    created_at: i64,
) -> Result<Nip98Event, AuthError> {
    let signer = browser_signer()?;
    let unsigned = nip98_unsigned_event(pubkey, method, url, payload_sha256, created_at);
    let unsigned = nip98_unsigned_to_nostr(&unsigned)?;
    let signed = signer.sign_event(unsigned).await.map_err(nip07_error)?;
    Ok(nip98_event_from_nostr(signed))
}

#[cfg(not(target_family = "wasm"))]
pub async fn nip07_sign_nip98(
    _pubkey: String,
    _method: &str,
    _url: &str,
    _payload_sha256: Option<&str>,
    _created_at: i64,
) -> Result<Nip98Event, AuthError> {
    Err(AuthError::MissingNip07)
}

#[cfg(target_family = "wasm")]
pub fn nip07_available() -> bool {
    browser_signer().is_ok()
}

#[cfg(not(target_family = "wasm"))]
pub fn nip07_available() -> bool {
    false
}

fn local_storage() -> Result<Storage, AuthError> {
    let window = window_ref()?;
    window
        .local_storage()
        .map_err(|err| AuthError::Js(js_error(err)))?
        .ok_or(AuthError::MissingStorage)
}

fn window_ref() -> Result<Window, AuthError> {
    web_sys::window().ok_or(AuthError::MissingWindow)
}

fn generate_secret_key() -> Result<[u8; 32], AuthError> {
    let window = window_ref()?;
    let crypto = window.crypto().map_err(|_| AuthError::MissingCrypto)?;
    let mut bytes = [0u8; 32];
    crypto
        .get_random_values_with_u8_array(&mut bytes)
        .map_err(|err| AuthError::Js(js_error(err)))?;
    Ok(bytes)
}

fn pubkey_from_secret(secret: &[u8; 32]) -> Result<String, AuthError> {
    let signing_key = SigningKey::from_bytes(secret).map_err(|_| AuthError::InvalidSecretKey)?;
    let verifying_key = signing_key.verifying_key();
    Ok(hex::encode(verifying_key.to_bytes()))
}

fn parse_secret_hex(value: &str) -> Result<[u8; 32], AuthError> {
    let bytes = hex::decode(value).map_err(|_| AuthError::InvalidSecretKey)?;
    if bytes.len() != 32 {
        return Err(AuthError::InvalidSecretKey);
    }
    let mut secret = [0u8; 32];
    secret.copy_from_slice(&bytes);
    Ok(secret)
}

fn js_error(value: JsValue) -> String {
    value.as_string().unwrap_or_else(|| format!("{:?}", value))
}

#[cfg(target_family = "wasm")]
fn browser_signer() -> Result<BrowserSigner, AuthError> {
    BrowserSigner::new().map_err(|err| match err {
        Nip07Error::NoGlobalWindowObject | Nip07Error::NamespaceNotFound(_) => {
            AuthError::MissingNip07
        }
        _ => AuthError::Nip07(err.to_string()),
    })
}

#[cfg(target_family = "wasm")]
fn nip07_error(error: impl std::fmt::Display) -> AuthError {
    AuthError::Nip07(error.to_string())
}

#[cfg(target_family = "wasm")]
fn nip98_unsigned_to_nostr(unsigned: &Nip98UnsignedEvent) -> Result<UnsignedEvent, AuthError> {
    let pubkey =
        PublicKey::from_hex(&unsigned.pubkey).map_err(|err| AuthError::Nip07(err.to_string()))?;
    if unsigned.created_at < 0 {
        return Err(AuthError::Nip07("invalid created_at".to_string()));
    }
    let created_at = Timestamp::from_secs(unsigned.created_at as u64);
    let kind = Kind::from_u16(unsigned.kind as u16);
    let tags = unsigned
        .tags
        .iter()
        .map(|tag| Tag::parse(tag.clone()).map_err(|err| AuthError::Nip07(err.to_string())))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(UnsignedEvent::new(
        pubkey,
        created_at,
        kind,
        tags,
        unsigned.content.clone(),
    ))
}

#[cfg(target_family = "wasm")]
fn nip98_event_from_nostr(event: nostr::Event) -> Nip98Event {
    let tags = event
        .tags
        .into_iter()
        .map(|tag| tag.to_vec())
        .collect::<Vec<_>>();
    Nip98Event {
        id: event.id.to_hex(),
        pubkey: event.pubkey.to_hex(),
        created_at: event.created_at.as_secs() as i64,
        kind: event.kind.as_u16() as u32,
        tags,
        content: event.content,
        sig: hex::encode(event.sig.as_ref()),
    }
}

#[cfg(test)]
mod tests {
    #[cfg(target_arch = "wasm32")]
    use super::js_error;
    use super::{
        AuthError, auth_header, nip07_available, nip07_pubkey, nip07_sign_nip98, parse_secret_hex,
        pubkey_from_secret,
    };
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
    use gittree_app_core::{AppCoreError, Nip98Event};
    use std::error::Error;
    #[cfg(target_arch = "wasm32")]
    use wasm_bindgen::JsValue;

    #[test]
    fn nip07_available_defaults_false_on_native() {
        assert!(!nip07_available());
    }

    #[test]
    fn pubkey_from_secret_returns_hex() {
        let pubkey = pubkey_from_secret(&[1u8; 32]).expect("pubkey");
        assert_eq!(pubkey.len(), 64);
        assert!(pubkey.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[tokio::test]
    async fn nip07_pubkey_returns_missing_on_native() {
        let error = nip07_pubkey().await.expect_err("missing nip-07");
        assert!(matches!(error, AuthError::MissingNip07));
    }

    #[tokio::test]
    async fn nip07_sign_nip98_returns_missing_on_native() {
        let error = nip07_sign_nip98(
            "11".repeat(32),
            "GET",
            "http://localhost:8089/v1/health",
            None,
            1_700_000_000,
        )
        .await
        .expect_err("missing nip-07");
        assert!(matches!(error, AuthError::MissingNip07));
    }

    #[test]
    fn auth_header_encodes_event_json() {
        let event = sample_event();
        let header = auth_header(&event).expect("auth header");
        assert!(header.starts_with("Nostr "));
        let encoded = header.strip_prefix("Nostr ").expect("prefix");
        let decoded = BASE64_STANDARD.decode(encoded).expect("decode");
        let parsed: Nip98Event = serde_json::from_slice(&decoded).expect("json");
        assert_eq!(parsed.id, event.id);
        assert_eq!(parsed.kind, event.kind);
    }

    #[test]
    fn parse_secret_hex_parses_valid_secret() {
        let secret = parse_secret_hex(&"11".repeat(32)).expect("secret");
        assert_eq!(secret, [0x11u8; 32]);
    }

    #[test]
    fn parse_secret_hex_rejects_invalid_values() {
        assert!(matches!(
            parse_secret_hex("zz"),
            Err(AuthError::InvalidSecretKey)
        ));
        assert!(matches!(
            parse_secret_hex("11"),
            Err(AuthError::InvalidSecretKey)
        ));
    }

    #[test]
    fn auth_error_display_and_source_are_wired() {
        let core = AuthError::Core(AppCoreError::InvalidSignature);
        assert_eq!(core.to_string(), "auth core error: invalid signature");
        assert!(core.source().is_some());

        let missing = AuthError::MissingNip07;
        assert_eq!(missing.to_string(), "missing nip-07 provider");
        assert!(missing.source().is_none());
    }

    #[cfg(target_arch = "wasm32")]
    #[test]
    fn js_error_prefers_string_values() {
        assert_eq!(js_error(JsValue::from_str("boom")), "boom");
    }

    fn sample_event() -> Nip98Event {
        Nip98Event {
            id: "11".repeat(32),
            pubkey: "22".repeat(32),
            created_at: 1_700_000_000,
            kind: 27_235,
            tags: vec![vec![
                "u".to_string(),
                "http://localhost:8089/v1/health".to_string(),
            ]],
            content: String::new(),
            sig: "33".repeat(64),
        }
    }
}
