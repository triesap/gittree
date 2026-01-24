use async_trait::async_trait;
use gittree_core::RelayInfoDocument;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayAdapterConfig {
    pub relay_url: String,
}

impl RelayAdapterConfig {
    pub fn new(relay_url: impl Into<String>) -> Self {
        Self {
            relay_url: relay_url.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RelayAdapterError {
    Unsupported(String),
    Transport(String),
}

impl std::fmt::Display for RelayAdapterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RelayAdapterError::Unsupported(message) => write!(f, "unsupported: {message}"),
            RelayAdapterError::Transport(message) => write!(f, "transport error: {message}"),
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

#[cfg(test)]
mod tests {
    use super::{NostrRsRelayAdapter, RelayAdapter, RelayAdapterConfig, RelayAdapterError};

    #[tokio::test]
    async fn nostr_rs_adapter_reports_unsupported() {
        let adapter = NostrRsRelayAdapter::new(RelayAdapterConfig::new("wss://relay.example"));
        let err = adapter.relay_info().await.unwrap_err();
        assert!(matches!(err, RelayAdapterError::Unsupported(_)));
    }
}
