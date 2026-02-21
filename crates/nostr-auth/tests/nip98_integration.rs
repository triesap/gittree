use gittree_nostr_auth::{NIP98_KIND, Nip98Error, Nip98Event, Nip98Request, validate_nip98};
use secp256k1::{Keypair, Message, Secp256k1, SecretKey, XOnlyPublicKey};
use sha2::Digest;

const NOW: i64 = 1_700_000_000;

fn build_event(url: &str, method: &str, created_at: i64) -> (Nip98Event, Keypair) {
    let secp = Secp256k1::new();
    let secret_key = SecretKey::from_slice(&[1u8; 32]).expect("secret");
    let keypair = Keypair::from_secret_key(&secp, &secret_key);
    let (pubkey, _) = XOnlyPublicKey::from_keypair(&keypair);
    let pubkey_hex = hex::encode(pubkey.serialize());

    let mut event = Nip98Event {
        id: String::new(),
        pubkey: pubkey_hex,
        created_at,
        kind: NIP98_KIND,
        tags: vec![
            vec!["u".to_string(), url.to_string()],
            vec!["method".to_string(), method.to_string()],
        ],
        content: String::new(),
        sig: String::new(),
    };

    let event_id = build_event_id(&event);
    event.id = event_id.clone();
    event.sig = sign_event_id(&event_id, &keypair, &secp);
    (event, keypair)
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
    let msg = Message::from_digest_slice(&bytes).expect("message");
    let sig = secp.sign_schnorr_no_aux_rand(&msg, keypair);
    hex::encode(sig.as_ref())
}

fn signup_request(method: &'static str) -> Nip98Request<'static> {
    Nip98Request {
        method,
        url: "https://gittr.ee/v1/signup",
        payload_sha256: None,
        now: NOW,
        max_skew_seconds: 60,
    }
}

#[test]
fn validate_nip98_accepts_valid_signed_event_integration() {
    let (event, _) = build_event("https://gittr.ee/v1/signup", "POST", NOW);
    let auth = validate_nip98(&event, &signup_request("POST")).expect("valid auth");
    assert_eq!(auth.pubkey, event.pubkey);
    assert_eq!(auth.event_id, event.id);
}

#[test]
fn validate_nip98_rejects_method_and_skew_integration() {
    let (event, _) = build_event("https://gittr.ee/v1/signup", "POST", NOW - 1000);

    let method_err = validate_nip98(&event, &signup_request("GET")).expect_err("method mismatch");
    assert!(matches!(method_err, Nip98Error::InvalidMethod { .. }));

    let skew_err = validate_nip98(&event, &signup_request("POST")).expect_err("time skew");
    assert!(matches!(skew_err, Nip98Error::TimeSkew { .. }));
}

#[test]
fn validate_nip98_rejects_invalid_pubkey_integration() {
    let (mut event, _) = build_event("https://gittr.ee/v1/signup", "POST", NOW);
    event.pubkey = "ff".repeat(32);
    event.id = build_event_id(&event);
    event.sig = "11".repeat(64);

    let err = validate_nip98(&event, &signup_request("POST")).expect_err("invalid pubkey");
    assert!(matches!(err, Nip98Error::InvalidPublicKey));
}

#[test]
fn validate_nip98_rejects_signature_mismatch_integration() {
    let (mut event, _) = build_event("https://gittr.ee/v1/signup", "POST", NOW);
    let secp = Secp256k1::new();
    let other_key = SecretKey::from_slice(&[2u8; 32]).expect("secret");
    let other_pair = Keypair::from_secret_key(&secp, &other_key);
    event.sig = sign_event_id(&event.id, &other_pair, &secp);

    let err = validate_nip98(&event, &signup_request("POST")).expect_err("invalid signature");
    assert!(matches!(err, Nip98Error::InvalidSignature));
}
