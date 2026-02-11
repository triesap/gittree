use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::str::FromStr;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NostrEvent {
    pub id: String,
    pub pubkey: String,
    pub created_at: i64,
    pub kind: u32,
    pub tags: Vec<Vec<String>>,
    pub content: String,
    pub sig: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventError {
    Canonicalization,
    InvalidId,
    InvalidSignature,
    InvalidPubkey,
}

impl std::fmt::Display for EventError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EventError::Canonicalization => write!(f, "failed to canonicalize event"),
            EventError::InvalidId => write!(f, "event id mismatch"),
            EventError::InvalidSignature => write!(f, "invalid event signature"),
            EventError::InvalidPubkey => write!(f, "invalid event pubkey"),
        }
    }
}

impl std::error::Error for EventError {}

impl NostrEvent {
    pub fn canonical_json(&self) -> Result<String, EventError> {
        let payload = json!([0, self.pubkey, self.created_at, self.kind, self.tags, self.content]);
        serde_json::to_string(&payload).map_err(|_| EventError::Canonicalization)
    }

    pub fn compute_id(&self) -> Result<String, EventError> {
        let canonical = self.canonical_json()?;
        let digest = Sha256::digest(canonical.as_bytes());
        Ok(hex::encode(digest))
    }

    pub fn verify(&self) -> Result<(), EventError> {
        let canonical = self.canonical_json()?;
        let digest = Sha256::digest(canonical.as_bytes());
        let computed_id = hex::encode(digest);

        if self.id != computed_id {
            return Err(EventError::InvalidId);
        }

        let sig =
            secp256k1::schnorr::Signature::from_str(&self.sig).map_err(|_| {
                EventError::InvalidSignature
            })?;
        let pubkey = secp256k1::XOnlyPublicKey::from_str(&self.pubkey)
            .map_err(|_| EventError::InvalidPubkey)?;
        let msg = secp256k1::Message::from_digest_slice(&digest)
            .map_err(|_| EventError::InvalidSignature)?;
        let secp = secp256k1::Secp256k1::verification_only();
        secp.verify_schnorr(&sig, &msg, &pubkey)
            .map_err(|_| EventError::InvalidSignature)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{EventError, NostrEvent};
    use secp256k1::{Keypair, Secp256k1, SecretKey};
    use std::error::Error as _;

    fn sample_event() -> NostrEvent {
        NostrEvent {
            id: String::new(),
            pubkey: "f".repeat(64),
            created_at: 1,
            kind: 1,
            tags: vec![vec!["e".to_string(), "1".to_string()], vec!["p".to_string(), "2".to_string()]],
            content: "hello".to_string(),
            sig: String::new(),
        }
    }

    #[test]
    fn canonical_json_matches_expected() {
        let event = sample_event();
        let canonical = event.canonical_json().expect("canonical");
        assert_eq!(
            canonical,
            r#"[0,"ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",1,1,[["e","1"],["p","2"]],"hello"]"#
        );
    }

    #[test]
    fn compute_id_matches_expected() {
        let event = sample_event();
        let id = event.compute_id().expect("id");
        assert_eq!(
            id,
            "758cdd74b47b71b1a82bfe0e9ba72aa73eb553190945fe06c3379cbb1c4a4f7f"
        );
    }

    #[test]
    fn verify_accepts_valid_signature() {
        let secp = Secp256k1::new();
        let secret_key = SecretKey::from_slice(&[0x11; 32]).expect("secret");
        let keypair = Keypair::from_secret_key(&secp, &secret_key);
        let (pubkey, _) = secp256k1::XOnlyPublicKey::from_keypair(&keypair);

        let mut event = sample_event();
        event.pubkey = hex::encode(pubkey.serialize());
        event.id = event.compute_id().expect("id");

        let id_bytes = hex::decode(&event.id).expect("id bytes");
        let msg = secp256k1::Message::from_digest_slice(&id_bytes).expect("msg");
        let sig = secp.sign_schnorr(&msg, &keypair);
        event.sig = hex::encode(sig.as_ref());

        event.verify().expect("valid");
    }

    #[test]
    fn verify_rejects_invalid_id() {
        let mut event = sample_event();
        event.id = "00".to_string();
        event.sig = "00".repeat(64);
        let err = event.verify().unwrap_err();
        assert_eq!(err, EventError::InvalidId);
    }

    #[test]
    fn verify_rejects_invalid_signature() {
        let secp = Secp256k1::new();
        let secret_key = SecretKey::from_slice(&[0x22; 32]).expect("secret");
        let keypair = Keypair::from_secret_key(&secp, &secret_key);
        let (pubkey, _) = secp256k1::XOnlyPublicKey::from_keypair(&keypair);

        let mut event = sample_event();
        event.pubkey = hex::encode(pubkey.serialize());
        event.id = event.compute_id().expect("id");
        event.sig = "not-a-schnorr-signature".to_string();

        let err = event.verify().expect_err("invalid signature");
        assert_eq!(err, EventError::InvalidSignature);
    }

    #[test]
    fn verify_rejects_invalid_pubkey() {
        let secp = Secp256k1::new();
        let secret_key = SecretKey::from_slice(&[0x33; 32]).expect("secret");
        let keypair = Keypair::from_secret_key(&secp, &secret_key);

        let mut event = sample_event();
        event.pubkey = "g".repeat(64);
        event.id = event.compute_id().expect("id");

        let id_bytes = hex::decode(&event.id).expect("id bytes");
        let msg = secp256k1::Message::from_digest_slice(&id_bytes).expect("msg");
        let sig = secp.sign_schnorr(&msg, &keypair);
        event.sig = hex::encode(sig.as_ref());

        let err = event.verify().expect_err("invalid pubkey");
        assert_eq!(err, EventError::InvalidPubkey);
    }

    #[test]
    fn event_error_display_and_source_are_stable() {
        assert_eq!(
            EventError::Canonicalization.to_string(),
            "failed to canonicalize event"
        );
        assert_eq!(EventError::InvalidId.to_string(), "event id mismatch");
        assert_eq!(
            EventError::InvalidSignature.to_string(),
            "invalid event signature"
        );
        assert_eq!(EventError::InvalidPubkey.to_string(), "invalid event pubkey");
        assert!(EventError::InvalidId.source().is_none());
    }
}
