use gittree_core::{
    RelayCapabilitySet, RelayCompatibilityReport, RelayInfoDocument, capabilities_from_nip11,
};
use gittree_relay_adapter::RelayAdapter;
use serde::Serialize;
use url::Url;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RelayProbeResult {
    pub relay_url: String,
    pub nip11_url: Option<String>,
    pub report: RelayCompatibilityReport,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nip11: Option<RelayInfoDocument>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_probe: Option<ActiveProbeResult>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RelayProbeError {
    InvalidRelayUrl(String),
    Http(String),
    Parse(String),
}

impl std::fmt::Display for RelayProbeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RelayProbeError::InvalidRelayUrl(value) => write!(f, "invalid relay url: {value}"),
            RelayProbeError::Http(message) => write!(f, "relay probe http error: {message}"),
            RelayProbeError::Parse(message) => write!(f, "relay probe parse error: {message}"),
        }
    }
}

impl std::error::Error for RelayProbeError {}

pub trait RelayProbeClient {
    fn fetch_nip11(&self, url: &str) -> Result<Option<String>, RelayProbeError>;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ActiveProbeResult {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Default)]
pub struct HttpRelayProbeClient {
    client: reqwest::blocking::Client,
}

impl HttpRelayProbeClient {
    pub fn new() -> Result<Self, RelayProbeError> {
        let client = reqwest::blocking::Client::builder()
            .user_agent("gittree-relay-probe/0.1")
            .build()
            .map_err(|err| RelayProbeError::Http(err.to_string()))?;
        Ok(Self { client })
    }
}

impl RelayProbeClient for HttpRelayProbeClient {
    fn fetch_nip11(&self, url: &str) -> Result<Option<String>, RelayProbeError> {
        let response = self
            .client
            .get(url)
            .header("Accept", "application/nostr+json")
            .send()
            .map_err(|err| RelayProbeError::Http(err.to_string()))?;

        if response.status().as_u16() == 404 {
            return Ok(None);
        }

        if !response.status().is_success() {
            return Err(RelayProbeError::Http(format!(
                "status {}",
                response.status()
            )));
        }

        response
            .text()
            .map(Some)
            .map_err(|err| RelayProbeError::Http(err.to_string()))
    }
}

pub fn resolve_nip11_url(relay_url: &str) -> Result<String, RelayProbeError> {
    let mut url = Url::parse(relay_url)
        .map_err(|_| RelayProbeError::InvalidRelayUrl(relay_url.to_string()))?;
    match url.scheme() {
        "wss" => {
            url.set_scheme("https")
                .map_err(|_| RelayProbeError::InvalidRelayUrl(relay_url.to_string()))?;
        }
        "ws" => {
            url.set_scheme("http")
                .map_err(|_| RelayProbeError::InvalidRelayUrl(relay_url.to_string()))?;
        }
        "https" | "http" => {}
        _ => return Err(RelayProbeError::InvalidRelayUrl(relay_url.to_string())),
    }
    url.set_path("/");
    url.set_query(None);
    url.set_fragment(None);
    Ok(url.to_string())
}

pub fn probe_relay(
    relay_url: &str,
    client: &dyn RelayProbeClient,
) -> Result<RelayProbeResult, RelayProbeError> {
    let nip11_url = resolve_nip11_url(relay_url)?;
    let mut warnings = Vec::new();
    let mut nip11 = None;
    let mut supported = Vec::new();

    match client.fetch_nip11(&nip11_url)? {
        Some(body) => {
            let doc = RelayInfoDocument::from_json_str(&body)
                .map_err(|err| RelayProbeError::Parse(err.to_string()))?;
            supported = capabilities_from_nip11(&doc);
            nip11 = Some(doc);
        }
        None => {
            warnings.push("nip-11 unavailable; falling back to active probes".to_string());
        }
    }

    let report = RelayCapabilitySet::default().evaluate(relay_url, &supported);
    Ok(RelayProbeResult {
        relay_url: relay_url.to_string(),
        nip11_url: Some(nip11_url),
        report,
        nip11,
        active_probe: None,
        warnings,
    })
}

pub async fn probe_relay_with_adapter(
    relay_url: &str,
    client: &dyn RelayProbeClient,
    adapter: &dyn RelayAdapter,
) -> Result<RelayProbeResult, RelayProbeError> {
    let mut result = probe_relay(relay_url, client)?;
    match adapter.probe_write_read().await {
        Ok(()) => {
            result.active_probe = Some(ActiveProbeResult { ok: true, error: None });
        }
        Err(err) => {
            result.active_probe = Some(ActiveProbeResult {
                ok: false,
                error: Some(err.to_string()),
            });
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::{
        RelayProbeClient, RelayProbeError, probe_relay, probe_relay_with_adapter,
        resolve_nip11_url,
    };
    use async_trait::async_trait;
    use gittree_core::RelayInfoDocument;
    use gittree_relay_adapter::{RelayAdapter, RelayAdapterError};
    use once_cell::sync::Lazy;

    struct StubProbeClient {
        response: Option<&'static str>,
    }

    impl RelayProbeClient for StubProbeClient {
        fn fetch_nip11(&self, _url: &str) -> Result<Option<String>, RelayProbeError> {
            Ok(self.response.map(|value| value.to_string()))
        }
    }

    static NIP11_BODY: Lazy<&'static str> =
        Lazy::new(|| r#"{"name":"relay","supported_nips":[1,11,34]}"#);

    #[test]
    fn resolve_nip11_url_converts_ws_scheme() {
        let url = resolve_nip11_url("wss://relay.example/path").expect("url");
        assert_eq!(url, "https://relay.example/");
    }

    #[test]
    fn probe_relay_uses_nip11_document() {
        let client = StubProbeClient {
            response: Some(&NIP11_BODY),
        };
        let result = probe_relay("wss://relay.example", &client).expect("probe");
        assert!(result.report.is_compatible());
        assert!(result.warnings.is_empty());
    }

    struct OkAdapter;

    #[async_trait]
    impl RelayAdapter for OkAdapter {
        async fn relay_info(&self) -> Result<Option<RelayInfoDocument>, RelayAdapterError> {
            Ok(None)
        }

        async fn probe_write_read(&self) -> Result<(), RelayAdapterError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn probe_relay_with_adapter_records_active_success() {
        let client = StubProbeClient {
            response: Some(&NIP11_BODY),
        };
        let result = probe_relay_with_adapter("wss://relay.example", &client, &OkAdapter)
            .await
            .expect("probe");
        assert!(result.active_probe.is_some());
        assert!(result.active_probe.unwrap().ok);
    }
}
