#![forbid(unsafe_code)]

use gittree_core::nip34_common::is_hex_len;
use secp256k1::{Message, Secp256k1, XOnlyPublicKey};
use serde::{Deserialize, Serialize};
use sha2::Digest;

const HEX_EVENT_ID_LEN: usize = 64;
const HEX_PUBKEY_LEN: usize = 64;
const HEX_SIG_LEN: usize = 128;

pub const NIP98_KIND: u32 = 27_235;

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Nip98Request<'a> {
    pub method: &'a str,
    pub url: &'a str,
    pub payload_sha256: Option<&'a str>,
    pub now: i64,
    pub max_skew_seconds: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Nip98Auth {
    pub pubkey: String,
    pub event_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Nip98Error {
    InvalidKind(u32),
    MissingTag(&'static str),
    InvalidHex { field: &'static str, value: String },
    InvalidMethod { expected: String, found: String },
    InvalidUrl { expected: String, found: String },
    PayloadMismatch { expected: String, found: String },
    TimeSkew {
        created_at: i64,
        now: i64,
        max_skew: i64,
    },
    InvalidEventId { expected: String, found: String },
    InvalidSignature,
    InvalidEventEncoding(String),
    InvalidPublicKey,
    InvalidSignatureEncoding,
    InvalidEventIdEncoding,
}

impl std::fmt::Display for Nip98Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Nip98Error::InvalidKind(kind) => write!(f, "invalid kind: {kind}"),
            Nip98Error::MissingTag(tag) => write!(f, "missing tag: {tag}"),
            Nip98Error::InvalidHex { field, value } => {
                write!(f, "invalid hex for {field}: {value}")
            }
            Nip98Error::InvalidMethod { expected, found } => write!(
                f,
                "method mismatch (expected {expected}, got {found})"
            ),
            Nip98Error::InvalidUrl { expected, found } => {
                write!(f, "url mismatch (expected {expected}, got {found})")
            }
            Nip98Error::PayloadMismatch { expected, found } => write!(
                f,
                "payload mismatch (expected {expected}, got {found})"
            ),
            Nip98Error::TimeSkew {
                created_at,
                now,
                max_skew,
            } => write!(
                f,
                "created_at outside skew (created_at {created_at}, now {now}, max {max_skew})"
            ),
            Nip98Error::InvalidEventId { expected, found } => write!(
                f,
                "event id mismatch (expected {expected}, got {found})"
            ),
            Nip98Error::InvalidSignature => write!(f, "invalid signature"),
            Nip98Error::InvalidEventEncoding(message) => {
                write!(f, "invalid event encoding: {message}")
            }
            Nip98Error::InvalidPublicKey => write!(f, "invalid public key"),
            Nip98Error::InvalidSignatureEncoding => write!(f, "invalid signature encoding"),
            Nip98Error::InvalidEventIdEncoding => write!(f, "invalid event id encoding"),
        }
    }
}

impl std::error::Error for Nip98Error {}

pub fn validate_nip98(
    event: &Nip98Event,
    request: &Nip98Request<'_>,
) -> Result<Nip98Auth, Nip98Error> {
    if event.kind != NIP98_KIND {
        return Err(Nip98Error::InvalidKind(event.kind));
    }
    require_hex("event.id", &event.id, HEX_EVENT_ID_LEN)?;
    require_hex("event.pubkey", &event.pubkey, HEX_PUBKEY_LEN)?;
    require_hex("event.sig", &event.sig, HEX_SIG_LEN)?;

    let url = tag_value(&event.tags, "u").ok_or(Nip98Error::MissingTag("u"))?;
    if url != request.url {
        return Err(Nip98Error::InvalidUrl {
            expected: request.url.to_string(),
            found: url.to_string(),
        });
    }

    let method = tag_value(&event.tags, "method")
        .ok_or(Nip98Error::MissingTag("method"))?;
    if !method.eq_ignore_ascii_case(request.method) {
        return Err(Nip98Error::InvalidMethod {
            expected: request.method.to_string(),
            found: method.to_string(),
        });
    }

    if let Some(expected_payload) = request.payload_sha256 {
        let payload = tag_value(&event.tags, "payload")
            .ok_or(Nip98Error::MissingTag("payload"))?;
        if payload != expected_payload {
            return Err(Nip98Error::PayloadMismatch {
                expected: expected_payload.to_string(),
                found: payload.to_string(),
            });
        }
    }

    let diff = (request.now as i128 - event.created_at as i128).abs();
    if diff > request.max_skew_seconds as i128 {
        return Err(Nip98Error::TimeSkew {
            created_at: event.created_at,
            now: request.now,
            max_skew: request.max_skew_seconds,
        });
    }

    let expected_id = build_event_id(event)?;
    if !expected_id.eq_ignore_ascii_case(&event.id) {
        return Err(Nip98Error::InvalidEventId {
            expected: expected_id,
            found: event.id.clone(),
        });
    }

    verify_signature(event)?;

    Ok(Nip98Auth {
        pubkey: event.pubkey.clone(),
        event_id: event.id.clone(),
    })
}

fn require_hex(field: &'static str, value: &str, len: usize) -> Result<(), Nip98Error> {
    if !is_hex_len(value, len) {
        return Err(Nip98Error::InvalidHex {
            field,
            value: value.to_string(),
        });
    }
    Ok(())
}

fn tag_value<'a>(tags: &'a [Vec<String>], key: &str) -> Option<&'a str> {
    tags.iter()
        .find_map(|tag| match tag.as_slice() {
            [tag_key, tag_value, ..] if tag_key == key => Some(tag_value.as_str()),
            _ => None,
        })
}

fn build_event_id(event: &Nip98Event) -> Result<String, Nip98Error> {
    let payload = serde_json::json!([
        0,
        event.pubkey,
        event.created_at,
        event.kind,
        event.tags,
        event.content
    ]);
    let serialized =
        serde_json::to_string(&payload).map_err(|err| Nip98Error::InvalidEventEncoding(err.to_string()))?;
    let mut hasher = sha2::Sha256::new();
    hasher.update(serialized.as_bytes());
    let digest = hasher.finalize();
    Ok(hex::encode(digest))
}

fn verify_signature(event: &Nip98Event) -> Result<(), Nip98Error> {
    let event_id = hex::decode(&event.id).map_err(|_| Nip98Error::InvalidEventIdEncoding)?;
    let msg = Message::from_digest_slice(&event_id).map_err(|_| Nip98Error::InvalidEventIdEncoding)?;
    let sig_bytes = hex::decode(&event.sig).map_err(|_| Nip98Error::InvalidSignatureEncoding)?;
    let sig = secp256k1::schnorr::Signature::from_slice(&sig_bytes)
        .map_err(|_| Nip98Error::InvalidSignatureEncoding)?;
    let pubkey_bytes = hex::decode(&event.pubkey).map_err(|_| Nip98Error::InvalidPublicKey)?;
    let pubkey = XOnlyPublicKey::from_slice(&pubkey_bytes)
        .map_err(|_| Nip98Error::InvalidPublicKey)?;
    let secp = Secp256k1::verification_only();
    secp.verify_schnorr(&sig, &msg, &pubkey)
        .map_err(|_| Nip98Error::InvalidSignature)
}

#[cfg(test)]
mod tests {
    use super::{Nip98Event, Nip98Request, Nip98Error, NIP98_KIND, validate_nip98};
    use secp256k1::{Keypair, Secp256k1, SecretKey, XOnlyPublicKey, Message};
    use sha2::Digest;

    const NOW: i64 = 1_700_000_000;

    fn build_event(
        url: &str,
        method: &str,
        created_at: i64,
        payload: Option<&str>,
    ) -> (Nip98Event, String) {
        let secp = Secp256k1::new();
        let secret_key = SecretKey::from_slice(&[1u8; 32]).expect("secret");
        let keypair = Keypair::from_secret_key(&secp, &secret_key);
        let (pubkey, _) = XOnlyPublicKey::from_keypair(&keypair);
        let pubkey_hex = hex::encode(pubkey.serialize());

        let mut tags = vec![
            vec!["u".to_string(), url.to_string()],
            vec!["method".to_string(), method.to_string()],
        ];
        if let Some(payload_hash) = payload {
            tags.push(vec!["payload".to_string(), payload_hash.to_string()]);
        }

        let mut event = Nip98Event {
            id: String::new(),
            pubkey: pubkey_hex,
            created_at,
            kind: NIP98_KIND,
            tags,
            content: String::new(),
            sig: String::new(),
        };

        let event_id = build_event_id(&event);
        let sig = sign_event_id(&event_id, &keypair, &secp);
        event.id = event_id.clone();
        event.sig = sig;

        (event, event_id)
    }

    fn build_event_id(event: &Nip98Event) -> String {
        let payload = serde_json::json!([
            0,
            event.pubkey,
            event.created_at,
            event.kind,
            event.tags,
            event.content
        ]);
        let serialized = serde_json::to_string(&payload).expect("serialize");
        let mut hasher = sha2::Sha256::new();
        hasher.update(serialized.as_bytes());
        let digest = hasher.finalize();
        hex::encode(digest)
    }

    fn sign_event_id(event_id: &str, keypair: &Keypair, secp: &Secp256k1<secp256k1::All>) -> String {
        let bytes = hex::decode(event_id).expect("decode");
        let msg = Message::from_digest_slice(&bytes).expect("msg");
        let sig = secp.sign_schnorr(&msg, keypair);
        hex::encode(sig.as_ref())
    }

    #[test]
    fn validates_signed_event() {
        let (event, _) = build_event("https://gittr.ee/v1/signup", "POST", NOW, None);
        let request = Nip98Request {
            method: "POST",
            url: "https://gittr.ee/v1/signup",
            payload_sha256: None,
            now: NOW,
            max_skew_seconds: 60,
        };
        let auth = validate_nip98(&event, &request).expect("valid");
        assert_eq!(auth.pubkey, event.pubkey);
        assert_eq!(auth.event_id, event.id);
    }

    #[test]
    fn rejects_method_mismatch() {
        let (event, _) = build_event("https://gittr.ee/v1/signup", "POST", NOW, None);
        let request = Nip98Request {
            method: "GET",
            url: "https://gittr.ee/v1/signup",
            payload_sha256: None,
            now: NOW,
            max_skew_seconds: 60,
        };
        let err = validate_nip98(&event, &request).unwrap_err();
        assert!(matches!(err, Nip98Error::InvalidMethod { .. }));
    }

    #[test]
    fn rejects_url_mismatch() {
        let (event, _) = build_event("https://gittr.ee/v1/signup", "POST", NOW, None);
        let request = Nip98Request {
            method: "POST",
            url: "https://gittr.ee/v1/other",
            payload_sha256: None,
            now: NOW,
            max_skew_seconds: 60,
        };
        let err = validate_nip98(&event, &request).unwrap_err();
        assert!(matches!(err, Nip98Error::InvalidUrl { .. }));
    }

    #[test]
    fn rejects_payload_mismatch() {
        let payload_hash = "11".repeat(32);
        let (event, _) =
            build_event("https://gittr.ee/v1/signup", "POST", NOW, Some(&payload_hash));
        let expected = "22".repeat(32);
        let request = Nip98Request {
            method: "POST",
            url: "https://gittr.ee/v1/signup",
            payload_sha256: Some(expected.as_str()),
            now: NOW,
            max_skew_seconds: 60,
        };
        let err = validate_nip98(&event, &request).unwrap_err();
        assert!(matches!(err, Nip98Error::PayloadMismatch { .. }));
    }

    #[test]
    fn rejects_time_skew() {
        let (event, _) = build_event("https://gittr.ee/v1/signup", "POST", NOW - 1000, None);
        let request = Nip98Request {
            method: "POST",
            url: "https://gittr.ee/v1/signup",
            payload_sha256: None,
            now: NOW,
            max_skew_seconds: 60,
        };
        let err = validate_nip98(&event, &request).unwrap_err();
        assert!(matches!(err, Nip98Error::TimeSkew { .. }));
    }

    #[test]
    fn rejects_event_id_mismatch() {
        let (mut event, _) = build_event("https://gittr.ee/v1/signup", "POST", NOW, None);
        event.id = "11".repeat(32);
        let request = Nip98Request {
            method: "POST",
            url: "https://gittr.ee/v1/signup",
            payload_sha256: None,
            now: NOW,
            max_skew_seconds: 60,
        };
        let err = validate_nip98(&event, &request).unwrap_err();
        assert!(matches!(err, Nip98Error::InvalidEventId { .. }));
    }
}
