use crate::AppCoreError;
use k256::schnorr::signature::hazmat::PrehashSigner;
use k256::schnorr::SigningKey;
use serde::{Deserialize, Serialize};
use sha2::Digest;

pub const NIP98_KIND: u32 = 27_235;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Nip98UnsignedEvent {
    pub pubkey: String,
    pub created_at: i64,
    pub kind: u32,
    pub tags: Vec<Vec<String>>,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Nip98Event {
    pub id: String,
    pub pubkey: String,
    pub created_at: i64,
    pub kind: u32,
    pub tags: Vec<Vec<String>>,
    pub content: String,
    pub sig: String,
}

pub fn nip98_payload_hash(payload: &[u8]) -> Option<String> {
    if payload.is_empty() {
        return None;
    }
    let mut hasher = sha2::Sha256::new();
    hasher.update(payload);
    let digest = hasher.finalize();
    Some(hex::encode(digest))
}

pub fn nip98_unsigned_event(
    pubkey: impl Into<String>,
    method: &str,
    url: &str,
    payload_sha256: Option<&str>,
    created_at: i64,
) -> Nip98UnsignedEvent {
    Nip98UnsignedEvent {
        pubkey: pubkey.into(),
        created_at,
        kind: NIP98_KIND,
        tags: nip98_tags(method, url, payload_sha256),
        content: String::new(),
    }
}

pub fn nip98_event_id(unsigned: &Nip98UnsignedEvent) -> Result<String, AppCoreError> {
    let bytes = nip98_event_id_bytes(unsigned)?;
    Ok(hex::encode(bytes))
}

pub fn nip98_sign_event(
    secret_key: &[u8; 32],
    method: &str,
    url: &str,
    payload_sha256: Option<&str>,
    created_at: i64,
) -> Result<Nip98Event, AppCoreError> {
    let signing_key = SigningKey::from_bytes(secret_key)
        .map_err(|_| AppCoreError::InvalidSecretKey)?;
    let verifying_key = signing_key.verifying_key();
    let pubkey_hex = hex::encode(verifying_key.to_bytes());
    let unsigned = nip98_unsigned_event(pubkey_hex, method, url, payload_sha256, created_at);
    let id_bytes = nip98_event_id_bytes(&unsigned)?;
    let sig = signing_key
        .sign_prehash(&id_bytes)
        .map_err(|_| AppCoreError::InvalidSignature)?;
    Ok(Nip98Event {
        id: hex::encode(id_bytes),
        pubkey: unsigned.pubkey,
        created_at: unsigned.created_at,
        kind: unsigned.kind,
        tags: unsigned.tags,
        content: unsigned.content,
        sig: hex::encode(sig.to_bytes()),
    })
}

fn nip98_tags(method: &str, url: &str, payload_sha256: Option<&str>) -> Vec<Vec<String>> {
    let mut tags = vec![
        vec!["u".to_string(), url.to_string()],
        vec!["method".to_string(), method.to_string()],
    ];
    if let Some(payload) = payload_sha256 {
        tags.push(vec!["payload".to_string(), payload.to_string()]);
    }
    tags
}

fn nip98_event_id_bytes(unsigned: &Nip98UnsignedEvent) -> Result<[u8; 32], AppCoreError> {
    let payload = serde_json::json!([
        0,
        unsigned.pubkey,
        unsigned.created_at,
        unsigned.kind,
        unsigned.tags,
        unsigned.content,
    ]);
    let serialized = serde_json::to_string(&payload)
        .map_err(|err| AppCoreError::InvalidEventEncoding(err.to_string()))?;
    let mut hasher = sha2::Sha256::new();
    hasher.update(serialized.as_bytes());
    let digest = hasher.finalize();
    let mut bytes = [0u8; 32];
    bytes.copy_from_slice(&digest);
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::{nip98_payload_hash, nip98_sign_event, NIP98_KIND};
    use gittree_nostr_auth::{validate_nip98, Nip98Event as AuthEvent, Nip98Request};

    const NOW: i64 = 1_700_000_000;

    #[test]
    fn sign_event_validates_with_auth_service() {
        let secret = [1u8; 32];
        let event = nip98_sign_event(
            &secret,
            "POST",
            "http://localhost:8089/v1/signup",
            None,
            NOW,
        )
        .expect("event");
        assert_eq!(event.kind, NIP98_KIND);

        let auth_event = AuthEvent {
            id: event.id.clone(),
            pubkey: event.pubkey.clone(),
            created_at: event.created_at,
            kind: event.kind,
            tags: event.tags.clone(),
            content: event.content.clone(),
            sig: event.sig.clone(),
        };
        let request = Nip98Request {
            method: "POST",
            url: "http://localhost:8089/v1/signup",
            payload_sha256: None,
            now: NOW,
            max_skew_seconds: 60,
        };
        let auth = validate_nip98(&auth_event, &request).expect("valid");
        assert_eq!(auth.pubkey, event.pubkey);
    }

    #[test]
    fn payload_hash_returns_none_for_empty() {
        assert!(nip98_payload_hash(&[]).is_none());
    }
}
