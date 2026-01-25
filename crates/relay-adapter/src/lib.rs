use async_trait::async_trait;
use gittree_core::RelayInfoDocument;
use std::time::Duration;
use url::Url;

const DEFAULT_ADAPTER_TIMEOUT_SECS: u64 = 5;

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

#[async_trait]
pub trait RelayAdapter: Send + Sync {
    async fn relay_info(&self) -> Result<Option<RelayInfoDocument>, RelayAdapterError>;
    async fn probe_write_read(&self) -> Result<(), RelayAdapterError>;
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
}

#[async_trait]
impl RelayAdapter for WebsocketRelayAdapter {
    async fn relay_info(&self) -> Result<Option<RelayInfoDocument>, RelayAdapterError> {
        Ok(None)
    }

    async fn probe_write_read(&self) -> Result<(), RelayAdapterError> {
        let _ = self.normalized_url()?;
        Err(RelayAdapterError::Unsupported(
            "websocket adapter not enabled".to_string(),
        ))
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

#[cfg(test)]
mod tests {
    use super::{
        NostrRsRelayAdapter, RelayAdapter, RelayAdapterConfig, RelayAdapterError,
        WebsocketRelayAdapter, normalize_ws_url,
    };

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
    async fn nostr_rs_adapter_reports_unsupported() {
        let adapter = NostrRsRelayAdapter::new(RelayAdapterConfig::new("wss://relay.example"));
        let err = adapter.relay_info().await.unwrap_err();
        assert!(matches!(err, RelayAdapterError::Unsupported(_)));
    }

    #[tokio::test]
    async fn websocket_adapter_rejects_invalid_url() {
        let adapter = WebsocketRelayAdapter::new(RelayAdapterConfig::new("ftp://relay.example"));
        let err = adapter.probe_write_read().await.unwrap_err();
        assert!(matches!(err, RelayAdapterError::InvalidConfig(_)));
    }
}
