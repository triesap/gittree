use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine;
use gittree_app_core::{
    nip98_sign_event,
    nip98_unsigned_event,
    AppCoreError,
    Nip98Event,
    Nip98UnsignedEvent,
};
use js_sys::Date;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;
use web_sys::{Storage, Window};

const LOCAL_SECRET_KEY: &str = "gittree_local_secret";

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = ["window", "nostr"], js_name = getPublicKey)]
    fn nostr_get_public_key() -> js_sys::Promise;

    #[wasm_bindgen(js_namespace = ["window", "nostr"], js_name = signEvent)]
    fn nostr_sign_event(event: JsValue) -> js_sys::Promise;
}

#[derive(Debug)]
pub enum AuthError {
    MissingWindow,
    MissingStorage,
    MissingCrypto,
    InvalidSecretKey,
    MissingNip07,
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

pub fn unix_timestamp() -> i64 {
    (Date::now() / 1000.0).floor() as i64
}

pub fn auth_header(event: &Nip98Event) -> Result<String, AuthError> {
    let json = serde_json::to_vec(event)
        .map_err(|err| AuthError::EventEncoding(err.to_string()))?;
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
    nip98_sign_event(&secret, method, url, payload_sha256, created_at)
        .map_err(AuthError::Core)
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

pub async fn nip07_pubkey() -> Result<String, AuthError> {
    ensure_nostr()?;
    let value = JsFuture::from(nostr_get_public_key())
        .await
        .map_err(|err| AuthError::Js(js_error(err)))?;
    value
        .as_string()
        .ok_or(AuthError::EventEncoding("missing pubkey".to_string()))
}

pub async fn nip07_sign_nip98(
    pubkey: String,
    method: &str,
    url: &str,
    payload_sha256: Option<&str>,
    created_at: i64,
) -> Result<Nip98Event, AuthError> {
    ensure_nostr()?;
    let unsigned = nip98_unsigned_event(pubkey, method, url, payload_sha256, created_at);
    nip07_sign_event(unsigned).await
}

async fn nip07_sign_event(event: Nip98UnsignedEvent) -> Result<Nip98Event, AuthError> {
    let value = serde_wasm_bindgen::to_value(&event)
        .map_err(|err| AuthError::EventEncoding(err.to_string()))?;
    let signed = JsFuture::from(nostr_sign_event(value))
        .await
        .map_err(|err| AuthError::Js(js_error(err)))?;
    serde_wasm_bindgen::from_value(signed)
        .map_err(|err| AuthError::EventEncoding(err.to_string()))
}

fn ensure_nostr() -> Result<(), AuthError> {
    if nostr_available() {
        Ok(())
    } else {
        Err(AuthError::MissingNip07)
    }
}

fn nostr_available() -> bool {
    let window = match window_ref() {
        Ok(window) => window,
        Err(_) => return false,
    };
    js_sys::Reflect::has(&window, &JsValue::from_str("nostr")).unwrap_or(false)
}

#[cfg(target_arch = "wasm32")]
pub fn nip07_available() -> bool {
    nostr_available()
}

#[cfg(not(target_arch = "wasm32"))]
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
    value
        .as_string()
        .unwrap_or_else(|| format!("{:?}", value))
}

#[cfg(test)]
mod tests {
    use super::nip07_available;

    #[test]
    fn nip07_available_defaults_false_on_native() {
        assert!(!nip07_available());
    }
}
