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

async fn map_transport<E, Fut>(operation: Fut) -> Result<(), RelayAdapterError>
where
    E: std::fmt::Display,
    Fut: std::future::IntoFuture<Output = Result<(), E>>,
{
    operation
        .into_future()
        .await
        .map_err(|err| RelayAdapterError::Transport(err.to_string()))
}

fn to_protocol_json<T: Serialize>(value: &T) -> Result<String, RelayAdapterError> {
    serde_json::to_string(value).map_err(|err| RelayAdapterError::Protocol(err.to_string()))
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
        let event_json = to_protocol_json(&event)?;
        let event_message = format!("[\"EVENT\",{event_json}]");
        map_transport(write.send(WsMessage::Text(event_message))).await?;

        wait_for_ok(&mut read, &event.id, self.config.timeout).await?;

        let sub_id = format!("{PROBE_SUB_PREFIX}-{}", &event.id[..8]);
        let filter = json!({"ids":[event.id]});
        let req_message = format!("[\"REQ\",\"{sub_id}\",{filter}]");
        map_transport(write.send(WsMessage::Text(req_message))).await?;

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
        let event_json = to_protocol_json(event)?;
        let event_message = format!("[\"EVENT\",{event_json}]");
        map_transport(write.send(WsMessage::Text(event_message))).await?;
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
        WebsocketRelayAdapter, build_event_id, build_probe_event, normalize_ws_url,
        parse_eose_message, parse_event_message, parse_ok_message, sign_event_id, wait_for_event,
        wait_for_ok,
    };
    use futures_util::{SinkExt, StreamExt};
    use futures_util::stream;
    use gittree_core::RepoAnnouncement;
    use serde_json::json;
    use secp256k1::{Message, Secp256k1, SecretKey, XOnlyPublicKey};
    use std::time::Duration;
    use tokio_tungstenite::tungstenite::Message as WsMessage;

    fn sample_announcement() -> RepoAnnouncement {
        RepoAnnouncement {
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
        }
    }

    #[test]
    fn signed_event_from_announcement_uses_default_timestamp() {
        let announcement = sample_announcement();
        let secret_key = SecretKey::from_slice(&[3u8; 32]).expect("secret");

        let event =
            SignedNostrEvent::from_announcement(&announcement, &secret_key).expect("event");

        assert_eq!(event.kind, gittree_core::kinds::KIND_GIT_REPO_ANNOUNCEMENT.0);
        assert!(event.created_at > 0, "created at");
        assert!(!event.id.is_empty(), "event id");
        assert!(!event.sig.is_empty(), "signature");
    }

    #[tokio::test]
    async fn map_transport_maps_ok_result() {
        super::map_transport::<&'static str, _>(async { Ok::<(), &'static str>(()) })
            .await
            .expect("ok");
    }

    #[tokio::test]
    async fn map_transport_maps_error_result() {
        let err = super::map_transport::<&'static str, _>(async {
            Err::<(), &'static str>("boom")
        })
        .await
        .expect_err("error");

        let RelayAdapterError::Transport(message) = err else {
            panic!("expected transport error, got {err}");
        };
        assert!(message.contains("boom"), "{message}");
    }

    #[test]
    fn to_protocol_json_maps_serialization_errors() {
        #[derive(Debug)]
        struct AlwaysFail;

        impl serde::Serialize for AlwaysFail {
            fn serialize<S>(&self, _serializer: S) -> Result<S::Ok, S::Error>
            where
                S: serde::Serializer,
            {
                Err(serde::ser::Error::custom("boom"))
            }
        }

        let err = super::to_protocol_json(&AlwaysFail).expect_err("serialize");
        assert!(matches!(err, RelayAdapterError::Protocol(_)));
    }

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

    #[test]
    fn normalize_ws_url_accepts_ws() {
        let url = normalize_ws_url("ws://relay.example/").expect("url");
        assert_eq!(url.as_str(), "ws://relay.example/");
    }

    #[test]
    fn normalize_ws_url_converts_http() {
        let url = normalize_ws_url("http://relay.example").expect("url");
        assert_eq!(url.as_str(), "ws://relay.example/");
    }

    #[test]
    fn normalize_ws_url_rejects_unsupported_scheme() {
        let err = normalize_ws_url("ftp://relay.example").unwrap_err();
        assert!(matches!(err, RelayAdapterError::InvalidConfig(_)));
    }

    #[test]
    fn normalize_ws_url_rejects_invalid_input() {
        let err = normalize_ws_url("not a url").unwrap_err();
        assert!(matches!(err, RelayAdapterError::InvalidConfig(_)));
    }

    #[test]
    fn relay_adapter_error_display_variants_are_stable() {
        let unsupported = RelayAdapterError::Unsupported("x".to_string());
        assert!(unsupported.to_string().contains("unsupported"));
        let invalid = RelayAdapterError::InvalidConfig("x".to_string());
        assert!(invalid.to_string().contains("invalid config"));
        let transport = RelayAdapterError::Transport("x".to_string());
        assert!(transport.to_string().contains("transport error"));
        let protocol = RelayAdapterError::Protocol("x".to_string());
        assert!(protocol.to_string().contains("protocol error"));
    }

    #[test]
    fn relay_adapter_config_builder_applies_values() {
        let config = RelayAdapterConfig::new("wss://relay.example")
            .with_timeout(Duration::from_secs(9))
            .with_secret_key("ab".repeat(32));
        assert_eq!(config.relay_url, "wss://relay.example");
        assert_eq!(config.timeout, Duration::from_secs(9));
        assert!(config.secret_key.is_some());
    }

    #[tokio::test]
    async fn nostr_rs_adapter_methods_return_unsupported() {
        let adapter = super::NostrRsRelayAdapter::new(RelayAdapterConfig::new("wss://relay.example"));
        assert_eq!(adapter.relay_url(), "wss://relay.example");
        assert!(matches!(
            adapter.relay_info().await.unwrap_err(),
            RelayAdapterError::Unsupported(_)
        ));
        assert!(matches!(
            adapter.probe_write_read().await.unwrap_err(),
            RelayAdapterError::Unsupported(_)
        ));
        let event = SignedNostrEvent {
            id: "evt".to_string(),
            pubkey: "pub".to_string(),
            created_at: 1,
            kind: 1,
            tags: Vec::new(),
            content: String::new(),
            sig: "sig".to_string(),
        };
        assert!(matches!(
            adapter.publish_event(&event).await.unwrap_err(),
            RelayAdapterError::Unsupported(_)
        ));
    }

    #[tokio::test]
    async fn webhook_adapter_methods_return_unsupported() {
        let adapter = super::WebhookRelayAdapter::new(RelayAdapterConfig::new("https://relay.example"));
        assert_eq!(adapter.relay_url(), "https://relay.example");
        assert!(matches!(
            adapter.relay_info().await.unwrap_err(),
            RelayAdapterError::Unsupported(_)
        ));
        assert!(matches!(
            adapter.probe_write_read().await.unwrap_err(),
            RelayAdapterError::Unsupported(_)
        ));
        let event = SignedNostrEvent {
            id: "evt".to_string(),
            pubkey: "pub".to_string(),
            created_at: 1,
            kind: 1,
            tags: Vec::new(),
            content: String::new(),
            sig: "sig".to_string(),
        };
        assert!(matches!(
            adapter.publish_event(&event).await.unwrap_err(),
            RelayAdapterError::Unsupported(_)
        ));
    }

    #[tokio::test]
    async fn websocket_adapter_rejects_invalid_url() {
        let adapter = WebsocketRelayAdapter::new(RelayAdapterConfig::new("ftp://relay.example"));
        let err = adapter.probe_write_read().await.unwrap_err();
        assert!(matches!(err, RelayAdapterError::InvalidConfig(_)));
    }

    #[test]
    fn signed_event_has_valid_signature() {
        let secret_key = SecretKey::from_slice(&[7u8; 32]).expect("secret");
        let announcement = sample_announcement();
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

    #[test]
    fn signed_event_rejects_invalid_announcement() {
        let secret_key = SecretKey::from_slice(&[7u8; 32]).expect("secret");
        let mut announcement = sample_announcement();
        announcement.clone.clear();
        let err = SignedNostrEvent::from_announcement_with_created_at(&announcement, &secret_key, 1)
            .unwrap_err();
        assert!(matches!(err, RelayAdapterError::InvalidConfig(_)));
    }

    #[test]
    fn build_event_id_is_deterministic() {
        let tags = vec![vec!["d".to_string(), "repo".to_string()]];
        let first =
            build_event_id("abcd", 100, 30617, &tags, "").expect("event id");
        let second =
            build_event_id("abcd", 100, 30617, &tags, "").expect("event id");
        assert_eq!(first, second);
    }

    #[test]
    fn sign_event_id_rejects_invalid_hex() {
        let secret_key = SecretKey::from_slice(&[9u8; 32]).expect("secret");
        let secp = Secp256k1::new();
        let keypair = secp256k1::Keypair::from_secret_key(&secp, &secret_key);
        let err = sign_event_id(&secp, &keypair, "zz").unwrap_err();
        assert!(matches!(err, RelayAdapterError::Protocol(_)));
    }

    #[test]
    fn build_probe_event_embeds_relay_target() {
        let secret_key = SecretKey::from_slice(&[8u8; 32]).expect("secret");
        let event = build_probe_event("wss://relay.example", &secret_key).expect("probe");
        assert!(!event.tags.is_empty());
        let relay_tag_present = event
            .tags
            .iter()
            .any(|tag| tag.iter().any(|value| value == "wss://relay.example"));
        assert!(relay_tag_present);
    }

    #[test]
    fn parse_ok_message_parses_result() {
        let message = WsMessage::Text("[\"OK\",\"abc\",true,\"ok\"]".to_string());
        let parsed = parse_ok_message(&message).expect("parsed");
        assert_eq!(
            parsed,
            Some(("abc".to_string(), true, Some("ok".to_string())))
        );
    }

    #[test]
    fn parse_ok_message_ignores_non_ok_messages() {
        let message = WsMessage::Text("[\"EVENT\",\"sub\",{}]".to_string());
        let parsed = parse_ok_message(&message).expect("parsed");
        assert!(parsed.is_none());
    }

    #[test]
    fn parse_ok_message_rejects_invalid_json() {
        let message = WsMessage::Text("{".to_string());
        let err = parse_ok_message(&message).unwrap_err();
        assert!(matches!(err, RelayAdapterError::Protocol(_)));
    }

    #[test]
    fn parse_event_message_parses_event() {
        let message = WsMessage::Text(
            json!(["EVENT", "sub-1", {"id":"evt-1","kind":1}]).to_string(),
        );
        let parsed = parse_event_message(&message).expect("parsed").expect("event");
        assert_eq!(parsed.0, "sub-1");
        assert_eq!(parsed.1.get("id").and_then(|value| value.as_str()), Some("evt-1"));
    }

    #[test]
    fn parse_event_message_rejects_invalid_json() {
        let message = WsMessage::Text("{".to_string());
        let err = parse_event_message(&message).unwrap_err();
        assert!(matches!(err, RelayAdapterError::Protocol(_)));
    }

    #[test]
    fn parse_event_and_eose_ignore_non_text_messages() {
        let binary = WsMessage::Binary(vec![1, 2, 3]);
        assert!(parse_event_message(&binary).expect("event").is_none());
        assert!(parse_eose_message(&binary).expect("eose").is_none());
    }

    #[test]
    fn parse_eose_message_parses_sub_id() {
        let message = WsMessage::Text("[\"EOSE\",\"sub-1\"]".to_string());
        let parsed = parse_eose_message(&message).expect("parsed");
        assert_eq!(parsed.as_deref(), Some("sub-1"));
    }

    #[test]
    fn parse_eose_message_rejects_invalid_json() {
        let message = WsMessage::Text("{".to_string());
        let err = parse_eose_message(&message).unwrap_err();
        assert!(matches!(err, RelayAdapterError::Protocol(_)));
    }

    #[test]
    fn load_secret_key_rejects_non_hex_config() {
        let adapter = WebsocketRelayAdapter::new(
            RelayAdapterConfig::new("wss://relay.example").with_secret_key("not-hex"),
        );
        let err = adapter.load_secret_key().unwrap_err();
        assert!(matches!(err, RelayAdapterError::InvalidConfig(_)));
    }

    #[test]
    fn load_secret_key_accepts_explicit_secret() {
        let secret_hex = "0101010101010101010101010101010101010101010101010101010101010101";
        let adapter = WebsocketRelayAdapter::new(
            RelayAdapterConfig::new("wss://relay.example").with_secret_key(secret_hex),
        );
        let key = adapter.load_secret_key().expect("secret");
        assert_eq!(hex::encode(key.secret_bytes()), secret_hex);
    }

    #[test]
    fn load_secret_key_rejects_zero_secret_key() {
        let adapter = WebsocketRelayAdapter::new(
            RelayAdapterConfig::new("wss://relay.example")
                .with_secret_key("00".repeat(32)),
        );
        let err = adapter.load_secret_key().expect_err("invalid secret");
        assert!(matches!(err, RelayAdapterError::InvalidConfig(_)));
    }

    #[test]
    fn load_secret_key_generates_secret_when_missing() {
        let adapter = WebsocketRelayAdapter::new(RelayAdapterConfig::new("wss://relay.example"));
        let key = adapter.load_secret_key().expect("secret");
        assert_eq!(key.secret_bytes().len(), 32);
    }

    #[tokio::test]
    async fn websocket_adapter_publish_rejects_invalid_url() {
        let adapter = WebsocketRelayAdapter::new(RelayAdapterConfig::new("not-valid"));
        let event = SignedNostrEvent {
            id: "abc".to_string(),
            pubkey: "def".to_string(),
            created_at: 1,
            kind: 1,
            tags: Vec::new(),
            content: String::new(),
            sig: "123".to_string(),
        };
        let err = adapter.publish_event(&event).await.unwrap_err();
        assert!(matches!(err, RelayAdapterError::InvalidConfig(_)));
    }

    #[tokio::test]
    async fn websocket_adapter_probe_and_publish_map_connect_errors() {
        let adapter = WebsocketRelayAdapter::new(RelayAdapterConfig::new("ws://127.0.0.1:1"));
        let probe_err = adapter.probe_write_read().await.expect_err("connect error");
        assert!(matches!(probe_err, RelayAdapterError::Transport(_)));

        let event = SignedNostrEvent {
            id: "11".repeat(32),
            pubkey: "22".repeat(32),
            created_at: 1,
            kind: 1,
            tags: Vec::new(),
            content: String::new(),
            sig: "33".repeat(64),
        };
        let publish_err = adapter.publish_event(&event).await.expect_err("connect error");
        assert!(matches!(publish_err, RelayAdapterError::Transport(_)));
    }

    #[tokio::test]
    async fn websocket_adapter_probe_and_publish_round_trip() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind relay listener");
        let addr = listener.local_addr().expect("listener addr");
        let relay_url = format!("ws://{addr}");

        let server = tokio::spawn(async move {
            for connection_index in 0..2 {
                let (tcp, _) = listener.accept().await.expect("accept");
                let ws = tokio_tungstenite::accept_async(tcp).await.expect("handshake");
                let (mut write, mut read) = ws.split();

                let msg = read
                    .next()
                    .await
                    .expect("client message")
                    .expect("client message ok");
                let text = msg.into_text().expect("text");
                let value: serde_json::Value = serde_json::from_str(&text).expect("client json");
                let array = value.as_array().expect("client message array");
                assert_eq!(array.first().and_then(|v| v.as_str()), Some("EVENT"));
                let event = array.get(1).cloned().unwrap_or_else(|| json!({}));
                let event_id = event
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                assert!(!event_id.is_empty(), "event id");

                // Send an unrelated OK first to exercise the ignore path, then a matching OK.
                let wrong_ok = json!(["OK", "deadbeef", true, "ignored"]);
                write
                    .send(WsMessage::Text(wrong_ok.to_string()))
                    .await
                    .expect("send ok");
                let ok = json!(["OK", event_id, true, "accepted"]);
                write
                    .send(WsMessage::Text(ok.to_string()))
                    .await
                    .expect("send ok");

                if connection_index == 0 {
                    let msg = read
                        .next()
                        .await
                        .expect("client req")
                        .expect("client req ok");
                    let text = msg.into_text().expect("text");
                    let req: serde_json::Value = serde_json::from_str(&text).expect("req json");
                    let req_array = req.as_array().expect("req array");
                    assert_eq!(req_array.first().and_then(|v| v.as_str()), Some("REQ"));
                    let sub_id = req_array
                        .get(1)
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    assert!(!sub_id.is_empty(), "sub id");

                    // Send an EVENT with the correct sub id but the wrong event id first.
                    let wrong_event = json!({ "id": "00".repeat(32) });
                    let wrong_event_msg = json!(["EVENT", sub_id, wrong_event]);
                    write
                        .send(WsMessage::Text(wrong_event_msg.to_string()))
                        .await
                        .expect("send event");

                    // Follow up with the correct event.
                    let correct_event_msg = json!(["EVENT", sub_id, event]);
                    write
                        .send(WsMessage::Text(correct_event_msg.to_string()))
                        .await
                        .expect("send event");

                    tokio::time::sleep(Duration::from_millis(25)).await;
                }
            }
        });

        let adapter = WebsocketRelayAdapter::new(
            RelayAdapterConfig::new(&relay_url)
                .with_timeout(Duration::from_secs(2))
                .with_secret_key(&"01".repeat(32)),
        );

        adapter.probe_write_read().await.expect("probe");

        let secret_key = SecretKey::from_slice(&[7u8; 32]).expect("secret");
        let event = build_probe_event(&relay_url, &secret_key).expect("event");
        adapter.publish_event(&event).await.expect("publish");

        server.await.expect("server task");
    }

    #[tokio::test]
    async fn websocket_adapter_relay_info_returns_none() {
        let adapter = WebsocketRelayAdapter::new(RelayAdapterConfig::new("wss://relay.example"));
        assert_eq!(adapter.relay_url(), "wss://relay.example");
        let info = adapter.relay_info().await.expect("info");
        assert!(info.is_none());
    }

    #[tokio::test]
    async fn wait_for_ok_handles_success_and_rejection() {
        let mut accepted = stream::iter(vec![Ok::<WsMessage, tokio_tungstenite::tungstenite::Error>(
            WsMessage::Text("[\"OK\",\"evt\",true,\"accepted\"]".to_string()),
        )]);
        wait_for_ok(&mut accepted, "evt", Duration::from_secs(1))
            .await
            .expect("ok");

        let mut rejected = stream::iter(vec![Ok::<WsMessage, tokio_tungstenite::tungstenite::Error>(
            WsMessage::Text("[\"OK\",\"evt\",false,\"denied\"]".to_string()),
        )]);
        let err = wait_for_ok(&mut rejected, "evt", Duration::from_secs(1))
            .await
            .expect_err("rejected");
        assert!(matches!(err, RelayAdapterError::Protocol(_)));
    }

    #[tokio::test]
    async fn wait_for_ok_ignores_other_event_ids_until_match() {
        let mut stream = stream::iter(vec![
            Ok::<WsMessage, tokio_tungstenite::tungstenite::Error>(WsMessage::Text(
                "[\"OK\",\"other\",true,\"accepted\"]".to_string(),
            )),
            Ok::<WsMessage, tokio_tungstenite::tungstenite::Error>(WsMessage::Text(
                "[\"OK\",\"evt\",true,\"accepted\"]".to_string(),
            )),
        ]);
        wait_for_ok(&mut stream, "evt", Duration::from_secs(1))
            .await
            .expect("matching id should succeed");
    }

    #[tokio::test]
    async fn wait_for_ok_reports_closed_stream() {
        let mut empty = stream::iter(Vec::<Result<WsMessage, tokio_tungstenite::tungstenite::Error>>::new());
        let err = wait_for_ok(&mut empty, "evt", Duration::from_secs(1))
            .await
            .expect_err("closed");
        assert!(matches!(err, RelayAdapterError::Transport(_)));
    }

    #[tokio::test]
    async fn wait_for_ok_times_out_when_stream_never_yields() {
        let mut pending = stream::pending::<Result<WsMessage, tokio_tungstenite::tungstenite::Error>>();
        let err = wait_for_ok(&mut pending, "evt", Duration::from_millis(20))
            .await
            .expect_err("timeout");
        assert!(matches!(
            err,
            RelayAdapterError::Transport(message) if message == "probe ok timeout"
        ));
    }

    #[tokio::test]
    async fn wait_for_ok_with_zero_timeout_returns_immediately() {
        let mut stream = stream::iter(vec![Ok::<WsMessage, tokio_tungstenite::tungstenite::Error>(
            WsMessage::Text("[\"OK\",\"evt\",true]".to_string()),
        )]);
        let err = wait_for_ok(&mut stream, "evt", Duration::ZERO)
            .await
            .expect_err("timeout");
        assert!(matches!(
            err,
            RelayAdapterError::Transport(message) if message == "probe ok timeout"
        ));
    }

    #[tokio::test]
    async fn wait_for_event_handles_match_and_eose() {
        let mut found = stream::iter(vec![Ok::<WsMessage, tokio_tungstenite::tungstenite::Error>(
            WsMessage::Text(json!(["EVENT", "sub-1", {"id":"evt-1"}]).to_string()),
        )]);
        wait_for_event(&mut found, "sub-1", "evt-1", Duration::from_secs(1))
            .await
            .expect("event");

        let mut eose = stream::iter(vec![Ok::<WsMessage, tokio_tungstenite::tungstenite::Error>(
            WsMessage::Text("[\"EOSE\",\"sub-1\"]".to_string()),
        )]);
        let err = wait_for_event(&mut eose, "sub-1", "evt-1", Duration::from_secs(1))
            .await
            .expect_err("missing event");
        assert!(matches!(err, RelayAdapterError::Protocol(_)));
    }

    #[tokio::test]
    async fn wait_for_event_ignores_unrelated_messages_until_match() {
        let mut stream = stream::iter(vec![
            Ok::<WsMessage, tokio_tungstenite::tungstenite::Error>(WsMessage::Text(
                json!(["EVENT", "sub-other", {"id":"evt-1"}]).to_string(),
            )),
            Ok::<WsMessage, tokio_tungstenite::tungstenite::Error>(WsMessage::Text(
                json!(["EVENT", "sub-1", {"id":"evt-other"}]).to_string(),
            )),
            Ok::<WsMessage, tokio_tungstenite::tungstenite::Error>(WsMessage::Text(
                "[\"EOSE\",\"sub-other\"]".to_string(),
            )),
            Ok::<WsMessage, tokio_tungstenite::tungstenite::Error>(WsMessage::Text(
                json!(["EVENT", "sub-1", {"id":"evt-1"}]).to_string(),
            )),
        ]);
        wait_for_event(&mut stream, "sub-1", "evt-1", Duration::from_secs(1))
            .await
            .expect("matching event should be found");
    }

    #[tokio::test]
    async fn wait_for_event_reports_closed_and_timeout_paths() {
        let mut empty = stream::iter(Vec::<Result<WsMessage, tokio_tungstenite::tungstenite::Error>>::new());
        let closed = wait_for_event(&mut empty, "sub-1", "evt-1", Duration::from_secs(1))
            .await
            .expect_err("closed");
        assert!(matches!(
            closed,
            RelayAdapterError::Transport(message) if message == "relay closed"
        ));

        let mut pending = stream::pending::<Result<WsMessage, tokio_tungstenite::tungstenite::Error>>();
        let timeout_err = wait_for_event(&mut pending, "sub-1", "evt-1", Duration::from_millis(20))
            .await
            .expect_err("timeout");
        assert!(matches!(
            timeout_err,
            RelayAdapterError::Transport(message) if message == "probe event timeout"
        ));
    }

    #[tokio::test]
    async fn wait_for_event_with_zero_timeout_returns_immediately() {
        let mut stream = stream::iter(vec![Ok::<WsMessage, tokio_tungstenite::tungstenite::Error>(
            WsMessage::Text(json!(["EVENT", "sub-1", {"id":"evt-1"}]).to_string()),
        )]);
        let err = wait_for_event(&mut stream, "sub-1", "evt-1", Duration::ZERO)
            .await
            .expect_err("timeout");
        assert!(matches!(
            err,
            RelayAdapterError::Transport(message) if message == "probe event timeout"
        ));
    }

    #[test]
    fn parse_messages_ignore_non_array_json_values() {
        let ok = parse_ok_message(&WsMessage::Text("{}".to_string())).expect("ok");
        assert!(ok.is_none());
        let event = parse_event_message(&WsMessage::Text("{}".to_string())).expect("event");
        assert!(event.is_none());
        let eose = parse_eose_message(&WsMessage::Text("{}".to_string())).expect("eose");
        assert!(eose.is_none());
    }

    #[test]
    fn parse_ok_message_ignores_non_text_message() {
        let binary = WsMessage::Binary(vec![1, 2, 3]);
        let parsed = parse_ok_message(&binary).expect("parsed");
        assert!(parsed.is_none());
    }
}
