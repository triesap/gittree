use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use gittree_core::kinds::KIND_GIT_REPO_ANNOUNCEMENT;
use gittree_core::{RepoAnnouncement, RelayInfoDocument};
use rand::rngs::OsRng;
use secp256k1::{Keypair, Message, Secp256k1, SecretKey, XOnlyPublicKey};
use serde::Serialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::time::{timeout, Instant};
use tokio_tungstenite::tungstenite::Message as WsMessage;
use url::Url;

const DEFAULT_ADAPTER_TIMEOUT_SECS: u64 = 5;
const PROBE_SUB_PREFIX: &str = "gittree-probe";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayAdapterConfig {
    pub relay_url: String,
    pub timeout: Duration,
    pub secret_key: Option<String>,
}

impl RelayAdapterConfig {
    pub fn new(relay_url: impl Into<String>) -> Self {
        Self {
            relay_url: relay_url.into(),
            timeout: Duration::from_secs(DEFAULT_ADAPTER_TIMEOUT_SECS),
            secret_key: None,
        }
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn with_secret_key(mut self, secret_key: impl Into<String>) -> Self {
        self.secret_key = Some(secret_key.into());
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RelayAdapterError {
    Unsupported(String),
    InvalidConfig(String),
    Transport(String),
    Protocol(String),
}

impl std::fmt::Display for RelayAdapterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RelayAdapterError::Unsupported(message) => write!(f, "unsupported: {message}"),
            RelayAdapterError::InvalidConfig(message) => write!(f, "invalid config: {message}"),
            RelayAdapterError::Transport(message) => write!(f, "transport error: {message}"),
            RelayAdapterError::Protocol(message) => write!(f, "protocol error: {message}"),
        }
    }
}

impl std::error::Error for RelayAdapterError {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SignedNostrEvent {
    pub id: String,
    pub pubkey: String,
    pub created_at: i64,
    pub kind: u32,
    pub tags: Vec<Vec<String>>,
    pub content: String,
    pub sig: String,
}

impl SignedNostrEvent {
    pub fn from_announcement(
        announcement: &RepoAnnouncement,
        secret_key: &SecretKey,
    ) -> Result<Self, RelayAdapterError> {
        Self::from_announcement_with_created_at(announcement, secret_key, unix_timestamp())
    }

    pub fn from_announcement_with_created_at(
        announcement: &RepoAnnouncement,
        secret_key: &SecretKey,
        created_at: i64,
    ) -> Result<Self, RelayAdapterError> {
        announcement
            .validate()
            .map_err(|err| RelayAdapterError::InvalidConfig(err.to_string()))?;
        let tags = announcement.to_tags();
        Self::signed(
            created_at,
            KIND_GIT_REPO_ANNOUNCEMENT.0,
            tags,
            String::new(),
            secret_key,
        )
    }

    pub fn signed(
        created_at: i64,
        kind: u32,
        tags: Vec<Vec<String>>,
        content: String,
        secret_key: &SecretKey,
    ) -> Result<Self, RelayAdapterError> {
        let secp = Secp256k1::new();
        let keypair = Keypair::from_secret_key(&secp, secret_key);
        let (pubkey, _) = XOnlyPublicKey::from_keypair(&keypair);
        let pubkey_hex = hex::encode(pubkey.serialize());

        let event_id = build_event_id(&pubkey_hex, created_at, kind, &tags, &content)?;
        let sig = sign_event_id(&secp, &keypair, &event_id)?;

        Ok(Self {
            id: event_id,
            pubkey: pubkey_hex,
            created_at,
            kind,
            tags,
            content,
            sig,
        })
    }
}

#[async_trait]
pub trait RelayAdapter: Send + Sync {
    async fn relay_info(&self) -> Result<Option<RelayInfoDocument>, RelayAdapterError>;
    async fn probe_write_read(&self) -> Result<(), RelayAdapterError>;
    async fn publish_event(&self, event: &SignedNostrEvent) -> Result<(), RelayAdapterError>;
}

#[derive(Debug, Clone)]
pub struct NostrRsRelayAdapter {
    config: RelayAdapterConfig,
}

impl NostrRsRelayAdapter {
    pub fn new(config: RelayAdapterConfig) -> Self {
        Self { config }
    }

    pub fn relay_url(&self) -> &str {
        &self.config.relay_url
    }
}

#[async_trait]
impl RelayAdapter for NostrRsRelayAdapter {
    async fn relay_info(&self) -> Result<Option<RelayInfoDocument>, RelayAdapterError> {
        Err(RelayAdapterError::Unsupported(
            "nostr-rs-relay adapter not enabled".to_string(),
        ))
    }

    async fn probe_write_read(&self) -> Result<(), RelayAdapterError> {
        Err(RelayAdapterError::Unsupported(
            "nostr-rs-relay adapter not enabled".to_string(),
        ))
    }

    async fn publish_event(&self, _event: &SignedNostrEvent) -> Result<(), RelayAdapterError> {
        Err(RelayAdapterError::Unsupported(
            "nostr-rs-relay adapter not enabled".to_string(),
        ))
    }
}

#[derive(Debug, Clone)]
pub struct WebhookRelayAdapter {
    config: RelayAdapterConfig,
}

impl WebhookRelayAdapter {
    pub fn new(config: RelayAdapterConfig) -> Self {
        Self { config }
    }

    pub fn relay_url(&self) -> &str {
        &self.config.relay_url
    }
}

#[async_trait]
impl RelayAdapter for WebhookRelayAdapter {
    async fn relay_info(&self) -> Result<Option<RelayInfoDocument>, RelayAdapterError> {
        Err(RelayAdapterError::Unsupported(
            "webhook adapter not configured".to_string(),
        ))
    }

    async fn probe_write_read(&self) -> Result<(), RelayAdapterError> {
        Err(RelayAdapterError::Unsupported(
            "webhook adapter not configured".to_string(),
        ))
    }

    async fn publish_event(&self, _event: &SignedNostrEvent) -> Result<(), RelayAdapterError> {
        Err(RelayAdapterError::Unsupported(
            "webhook adapter not configured".to_string(),
        ))
    }
}

#[derive(Debug, Clone)]
pub struct WebsocketRelayAdapter {
    config: RelayAdapterConfig,
}

impl WebsocketRelayAdapter {
    pub fn new(config: RelayAdapterConfig) -> Self {
        Self { config }
    }

    pub fn relay_url(&self) -> &str {
        &self.config.relay_url
    }

    fn normalized_url(&self) -> Result<Url, RelayAdapterError> {
        normalize_ws_url(&self.config.relay_url)
    }

    fn load_secret_key(&self) -> Result<SecretKey, RelayAdapterError> {
        match &self.config.secret_key {
            Some(hex_key) => {
                let bytes = hex::decode(hex_key).map_err(|_| {
                    RelayAdapterError::InvalidConfig("secret key must be hex".to_string())
                })?;
                SecretKey::from_slice(&bytes).map_err(|err| {
                    RelayAdapterError::InvalidConfig(format!("invalid secret key: {err}"))
                })
            }
            None => {
                let mut rng = OsRng;
                Ok(SecretKey::new(&mut rng))
            }
        }
    }
}

#[async_trait]
impl RelayAdapter for WebsocketRelayAdapter {
    async fn relay_info(&self) -> Result<Option<RelayInfoDocument>, RelayAdapterError> {
        Ok(None)
    }

    async fn probe_write_read(&self) -> Result<(), RelayAdapterError> {
        let url = self.normalized_url()?;
        let (stream, _) = tokio_tungstenite::connect_async(url)
            .await
            .map_err(|err| RelayAdapterError::Transport(err.to_string()))?;
        let (mut write, mut read) = stream.split();

        let secret_key = self.load_secret_key()?;
        let event = build_probe_event(&self.config.relay_url, &secret_key)?;
        let event_json = serde_json::to_string(&event)
            .map_err(|err| RelayAdapterError::Protocol(err.to_string()))?;
        let event_message = format!("[\"EVENT\",{event_json}]");
        write
            .send(WsMessage::Text(event_message))
            .await
            .map_err(|err| RelayAdapterError::Transport(err.to_string()))?;

        wait_for_ok(&mut read, &event.id, self.config.timeout).await?;

        let sub_id = format!("{PROBE_SUB_PREFIX}-{}", &event.id[..8]);
        let filter = json!({"ids":[event.id]});
        let req_message = format!("[\"REQ\",\"{sub_id}\",{filter}]");
        write
            .send(WsMessage::Text(req_message))
            .await
            .map_err(|err| RelayAdapterError::Transport(err.to_string()))?;

        wait_for_event(&mut read, &sub_id, &event.id, self.config.timeout).await?;
        let close_message = format!("[\"CLOSE\",\"{sub_id}\"]");
        let _ = write.send(WsMessage::Text(close_message)).await;
        Ok(())
    }

    async fn publish_event(&self, event: &SignedNostrEvent) -> Result<(), RelayAdapterError> {
        let url = self.normalized_url()?;
        let (stream, _) = tokio_tungstenite::connect_async(url)
            .await
            .map_err(|err| RelayAdapterError::Transport(err.to_string()))?;
        let (mut write, mut read) = stream.split();
        let event_json = serde_json::to_string(event)
            .map_err(|err| RelayAdapterError::Protocol(err.to_string()))?;
        let event_message = format!("[\"EVENT\",{event_json}]");
        write
            .send(WsMessage::Text(event_message))
            .await
            .map_err(|err| RelayAdapterError::Transport(err.to_string()))?;
        wait_for_ok(&mut read, &event.id, self.config.timeout).await?;
        Ok(())
    }
}

fn normalize_ws_url(input: &str) -> Result<Url, RelayAdapterError> {
    let mut url = Url::parse(input)
        .map_err(|_| RelayAdapterError::InvalidConfig("invalid relay url".to_string()))?;
    match url.scheme() {
        "wss" | "ws" => {}
        "https" => {
            url.set_scheme("wss")
                .map_err(|_| RelayAdapterError::InvalidConfig("invalid relay url".to_string()))?;
        }
        "http" => {
            url.set_scheme("ws")
                .map_err(|_| RelayAdapterError::InvalidConfig("invalid relay url".to_string()))?;
        }
        _ => {
            return Err(RelayAdapterError::InvalidConfig(
                "unsupported relay scheme".to_string(),
            ))
        }
    }
    Ok(url)
}

fn build_probe_event(
    relay_url: &str,
    secret_key: &SecretKey,
) -> Result<SignedNostrEvent, RelayAdapterError> {
    let now = unix_timestamp();
    let identifier = format!("probe-{now}");
    let announcement = RepoAnnouncement {
        identifier: identifier.clone(),
        name: Some("gittree probe".to_string()),
        description: None,
        root_commit: None,
        clone: vec![format!("https://example.invalid/{identifier}.git")],
        web: Vec::new(),
        relays: vec![relay_url.to_string()],
        blossoms: Vec::new(),
        hashtags: Vec::new(),
        maintainers: Vec::new(),
    };
    SignedNostrEvent::from_announcement_with_created_at(&announcement, secret_key, now)
}

fn build_event_id(
    pubkey: &str,
    created_at: i64,
    kind: u32,
    tags: &[Vec<String>],
    content: &str,
) -> Result<String, RelayAdapterError> {
    let payload = json!([0, pubkey, created_at, kind, tags, content]);
    let serialized = serde_json::to_string(&payload)
        .map_err(|err| RelayAdapterError::Protocol(err.to_string()))?;
    let mut hasher = Sha256::new();
    hasher.update(serialized.as_bytes());
    let digest = hasher.finalize();
    Ok(hex::encode(digest))
}

fn sign_event_id(
    secp: &Secp256k1<secp256k1::All>,
    keypair: &Keypair,
    event_id: &str,
) -> Result<String, RelayAdapterError> {
    let bytes = hex::decode(event_id).map_err(|_| {
        RelayAdapterError::Protocol("failed to decode event id".to_string())
    })?;
    let msg = Message::from_digest_slice(&bytes)
        .map_err(|_| RelayAdapterError::Protocol("invalid event id".to_string()))?;
    let sig = secp.sign_schnorr(&msg, keypair);
    Ok(hex::encode(sig.as_ref()))
}

fn unix_timestamp() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

async fn wait_for_ok<S>(
    read: &mut S,
    event_id: &str,
    timeout_duration: Duration,
) -> Result<(), RelayAdapterError>
where
    S: StreamExt<Item = Result<WsMessage, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    let deadline = Instant::now() + timeout_duration;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(RelayAdapterError::Transport("probe ok timeout".to_string()));
        }
        let msg = timeout(remaining, read.next())
            .await
            .map_err(|_| RelayAdapterError::Transport("probe ok timeout".to_string()))?
            .ok_or_else(|| RelayAdapterError::Transport("relay closed".to_string()))?
            .map_err(|err| RelayAdapterError::Transport(err.to_string()))?;
        if let Some((ok_id, ok, reason)) = parse_ok_message(&msg)? {
            if ok_id == event_id {
                if ok {
                    return Ok(());
                }
                return Err(RelayAdapterError::Protocol(format!(
                    "event rejected: {}",
                    reason.unwrap_or_else(|| "unknown".to_string())
                )));
            }
        }
    }
}

async fn wait_for_event<S>(
    read: &mut S,
    sub_id: &str,
    event_id: &str,
    timeout_duration: Duration,
) -> Result<(), RelayAdapterError>
where
    S: StreamExt<Item = Result<WsMessage, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    let deadline = Instant::now() + timeout_duration;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(RelayAdapterError::Transport("probe event timeout".to_string()));
        }
        let msg = timeout(remaining, read.next())
            .await
            .map_err(|_| RelayAdapterError::Transport("probe event timeout".to_string()))?
            .ok_or_else(|| RelayAdapterError::Transport("relay closed".to_string()))?
            .map_err(|err| RelayAdapterError::Transport(err.to_string()))?;
        if let Some((event_sub_id, event)) = parse_event_message(&msg)? {
            if event_sub_id == sub_id {
                if let Some(id) = event.get("id").and_then(|value| value.as_str()) {
                    if id == event_id {
                        return Ok(());
                    }
                }
            }
        }
        if let Some(eose_sub_id) = parse_eose_message(&msg)? {
            if eose_sub_id == sub_id {
                return Err(RelayAdapterError::Protocol(
                    "probe event not found".to_string(),
                ));
            }
        }
    }
}

fn parse_ok_message(
    message: &WsMessage,
) -> Result<Option<(String, bool, Option<String>)>, RelayAdapterError> {
    let WsMessage::Text(text) = message else {
        return Ok(None);
    };
    let value: serde_json::Value = serde_json::from_str(text)
        .map_err(|err| RelayAdapterError::Protocol(err.to_string()))?;
    let Some(array) = value.as_array() else {
        return Ok(None);
    };
    if array.len() < 3 || array.first().and_then(|v| v.as_str()) != Some("OK") {
        return Ok(None);
    }
    let event_id = array.get(1).and_then(|v| v.as_str()).unwrap_or("").to_string();
    let ok = array.get(2).and_then(|v| v.as_bool()).unwrap_or(false);
    let reason = array.get(3).and_then(|v| v.as_str()).map(|v| v.to_string());
    Ok(Some((event_id, ok, reason)))
}

fn parse_event_message(
    message: &WsMessage,
) -> Result<Option<(String, serde_json::Value)>, RelayAdapterError> {
    let WsMessage::Text(text) = message else {
        return Ok(None);
    };
    let value: serde_json::Value = serde_json::from_str(text)
        .map_err(|err| RelayAdapterError::Protocol(err.to_string()))?;
    let Some(array) = value.as_array() else {
        return Ok(None);
    };
    if array.len() < 3 || array.first().and_then(|v| v.as_str()) != Some("EVENT") {
        return Ok(None);
    }
    let sub_id = array.get(1).and_then(|v| v.as_str()).unwrap_or("").to_string();
    let event = array.get(2).cloned().unwrap_or_else(|| json!({}));
    Ok(Some((sub_id, event)))
}

fn parse_eose_message(message: &WsMessage) -> Result<Option<String>, RelayAdapterError> {
    let WsMessage::Text(text) = message else {
        return Ok(None);
    };
    let value: serde_json::Value = serde_json::from_str(text)
        .map_err(|err| RelayAdapterError::Protocol(err.to_string()))?;
    let Some(array) = value.as_array() else {
        return Ok(None);
    };
    if array.len() < 2 || array.first().and_then(|v| v.as_str()) != Some("EOSE") {
        return Ok(None);
    }
    let sub_id = array.get(1).and_then(|v| v.as_str()).unwrap_or("").to_string();
    Ok(Some(sub_id))
}

#[cfg(test)]
mod tests {
    use super::{
        RelayAdapter, RelayAdapterConfig, RelayAdapterError, SignedNostrEvent,
        WebsocketRelayAdapter, normalize_ws_url,
    };
    use secp256k1::{Message, Secp256k1, SecretKey, XOnlyPublicKey};

    #[test]
    fn normalize_ws_url_converts_https() {
        let url = normalize_ws_url("https://relay.example").expect("url");
        assert_eq!(url.as_str(), "wss://relay.example/");
    }

    #[test]
    fn normalize_ws_url_accepts_wss() {
        let url = normalize_ws_url("wss://relay.example/").expect("url");
        assert_eq!(url.as_str(), "wss://relay.example/");
    }

    #[tokio::test]
    async fn websocket_adapter_rejects_invalid_url() {
        let adapter = WebsocketRelayAdapter::new(RelayAdapterConfig::new("ftp://relay.example"));
        let err = adapter.probe_write_read().await.unwrap_err();
        assert!(matches!(err, RelayAdapterError::InvalidConfig(_)));
    }

    #[test]
    fn signed_event_has_valid_signature() {
        let secret_key =
            SecretKey::from_slice(&[7u8; 32]).expect("secret");
        let announcement = gittree_core::RepoAnnouncement {
            identifier: "repo".to_string(),
            name: Some("repo".to_string()),
            description: None,
            root_commit: None,
            clone: vec![
                "https://relay.example/npub1gjttreegkzys8jlhdnfm3qe39h2gka79cpndd0jsms5fk7tuhcnsdw56jq/repo.git"
                    .to_string(),
            ],
            web: Vec::new(),
            relays: vec!["wss://relay.example".to_string()],
            blossoms: Vec::new(),
            hashtags: Vec::new(),
            maintainers: Vec::new(),
        };
        let event =
            SignedNostrEvent::from_announcement_with_created_at(&announcement, &secret_key, 123)
                .expect("event");
        let secp = Secp256k1::new();
        let keypair = secp256k1::Keypair::from_secret_key(&secp, &secret_key);
        let (pubkey, _) = XOnlyPublicKey::from_keypair(&keypair);
        assert_eq!(event.pubkey, hex::encode(pubkey.serialize()));
        let id_bytes = hex::decode(&event.id).expect("id bytes");
        let msg = Message::from_digest_slice(&id_bytes).expect("msg");
        let sig_bytes = hex::decode(&event.sig).expect("sig bytes");
        let sig = secp256k1::schnorr::Signature::from_slice(&sig_bytes).expect("sig");
        secp.verify_schnorr(&sig, &msg, &pubkey)
            .expect("signature valid");
    }
}
