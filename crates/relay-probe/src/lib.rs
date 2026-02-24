use gittree_core::{
    ActiveProbeEvidence, RelayCapability, RelayCapabilitySet, RelayCompatibilityReport,
    RelayInfoDocument, capabilities_from_nip11, merge_active_probe_evidence,
};
use gittree_relay_adapter::RelayAdapter;
use serde::Serialize;
use url::Url;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RelayProbeResult {
    pub relay_url: String,
    pub nip11_url: Option<String>,
    pub nip11_available: bool,
    pub report: RelayCompatibilityReport,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub observed_capabilities: Vec<RelayCapability>,
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
        Self::with_user_agent("gittree-relay-probe/0.1")
    }

    fn with_user_agent(user_agent: &str) -> Result<Self, RelayProbeError> {
        let client = reqwest::blocking::Client::builder()
            .user_agent(user_agent)
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
            let _ = url.set_scheme("https");
        }
        "ws" => {
            let _ = url.set_scheme("http");
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
    let mut nip11_available = false;

    match client.fetch_nip11(&nip11_url)? {
        Some(body) => {
            let doc = RelayInfoDocument::from_json_str(&body)
                .map_err(|err| RelayProbeError::Parse(err.to_string()))?;
            supported = capabilities_from_nip11(&doc);
            nip11 = Some(doc);
            nip11_available = true;
        }
        None => {
            warnings.push("nip-11 unavailable; falling back to active probes".to_string());
        }
    }

    let report = RelayCapabilitySet::default().evaluate(relay_url, &supported);
    Ok(RelayProbeResult {
        relay_url: relay_url.to_string(),
        nip11_url: Some(nip11_url),
        nip11_available,
        report,
        observed_capabilities: supported,
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
    let result = probe_relay(relay_url, client)?;
    Ok(probe_relay_with_adapter_result(result, adapter).await)
}

pub async fn probe_relay_with_adapter_result(
    mut result: RelayProbeResult,
    adapter: &dyn RelayAdapter,
) -> RelayProbeResult {
    let relay_url = result.relay_url.clone();
    match adapter.probe_write_read().await {
        Ok(()) => {
            result.active_probe = Some(ActiveProbeResult {
                ok: true,
                error: None,
            });
            let mut supported = result.observed_capabilities.clone();
            merge_active_probe_evidence(&mut supported, ActiveProbeEvidence::success());
            result.report = RelayCapabilitySet::default().evaluate(relay_url, &supported);
            result.observed_capabilities = supported;
        }
        Err(err) => {
            result.active_probe = Some(ActiveProbeResult {
                ok: false,
                error: Some(err.to_string()),
            });
            merge_active_probe_evidence(
                &mut result.observed_capabilities,
                ActiveProbeEvidence::failure(),
            );
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::{
        HttpRelayProbeClient, RelayProbeClient, RelayProbeError, probe_relay,
        probe_relay_with_adapter, resolve_nip11_url,
    };
    use async_trait::async_trait;
    use gittree_core::RelayInfoDocument;
    use gittree_relay_adapter::{RelayAdapter, RelayAdapterError, SignedNostrEvent};
    use once_cell::sync::Lazy;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    struct StubProbeClient {
        response: Option<&'static str>,
    }

    impl RelayProbeClient for StubProbeClient {
        fn fetch_nip11(&self, _url: &str) -> Result<Option<String>, RelayProbeError> {
            Ok(self.response.map(|value| value.to_string()))
        }
    }

    struct ErrorProbeClient;

    impl RelayProbeClient for ErrorProbeClient {
        fn fetch_nip11(&self, _url: &str) -> Result<Option<String>, RelayProbeError> {
            Err(RelayProbeError::Http("boom".to_string()))
        }
    }

    static NIP11_BODY: Lazy<&'static str> =
        Lazy::new(|| r#"{"name":"relay","supported_nips":[1,11,34]}"#);

    fn spawn_http_server(status_line: &str, body: &str) -> (String, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("addr");
        let response = format!(
            "{status_line}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut request_buf = [0_u8; 1024];
            let _ = stream.read(&mut request_buf);
            stream
                .write_all(response.as_bytes())
                .expect("write response");
            stream.flush().expect("flush response");
        });
        (format!("http://{address}/"), handle)
    }

    fn spawn_raw_http_server(response: &[u8]) -> (String, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("addr");
        let response = response.to_vec();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut request_buf = [0_u8; 1024];
            let _ = stream.read(&mut request_buf);
            stream.write_all(&response).expect("write response");
            stream.flush().expect("flush response");
        });
        (format!("http://{address}/"), handle)
    }

    fn assert_invalid_relay_url(err: RelayProbeError) {
        if !matches!(err, RelayProbeError::InvalidRelayUrl(_)) {
            panic!("expected invalid relay url error, got {err}");
        }
    }

    fn assert_parse_error(err: RelayProbeError) {
        if !matches!(err, RelayProbeError::Parse(_)) {
            panic!("expected parse error, got {err}");
        }
    }

    fn assert_http_error(err: RelayProbeError) {
        if !matches!(err, RelayProbeError::Http(_)) {
            panic!("expected http error, got {err}");
        }
    }

    #[test]
    #[should_panic(expected = "expected invalid relay url error")]
    fn assert_invalid_relay_url_panics_for_non_invalid_variant() {
        assert_invalid_relay_url(RelayProbeError::Http("boom".to_string()));
    }

    #[test]
    #[should_panic(expected = "expected parse error")]
    fn assert_parse_error_panics_for_non_parse_variant() {
        assert_parse_error(RelayProbeError::Http("boom".to_string()));
    }

    #[test]
    #[should_panic(expected = "expected http error")]
    fn assert_http_error_panics_for_non_http_variant() {
        assert_http_error(RelayProbeError::Parse("boom".to_string()));
    }

    #[test]
    fn resolve_nip11_url_converts_ws_scheme() {
        let url = resolve_nip11_url("wss://relay.example/path").expect("url");
        assert_eq!(url, "https://relay.example/");
    }

    #[test]
    fn resolve_nip11_url_handles_other_schemes_and_rejects_invalid() {
        assert_eq!(
            resolve_nip11_url("ws://relay.example/path?x=1#frag").expect("url"),
            "http://relay.example/"
        );
        assert_eq!(
            resolve_nip11_url("https://relay.example/path").expect("url"),
            "https://relay.example/"
        );
        let invalid_scheme = resolve_nip11_url("ftp://relay.example").expect_err("invalid");
        assert_invalid_relay_url(invalid_scheme);
        let invalid_url = resolve_nip11_url("::not-a-url::").expect_err("invalid");
        assert_invalid_relay_url(invalid_url);
    }

    #[test]
    fn resolve_nip11_url_handles_non_hierarchical_ws_urls() {
        assert_eq!(
            resolve_nip11_url("wss:relay.example").expect("wss url"),
            "https://relay.example/"
        );
        assert_eq!(
            resolve_nip11_url("ws:relay.example").expect("ws url"),
            "http://relay.example/"
        );
    }

    #[test]
    fn probe_relay_uses_nip11_document() {
        let client = StubProbeClient {
            response: Some(&NIP11_BODY),
        };
        let result = probe_relay("wss://relay.example", &client).expect("probe");
        assert!(result.report.is_compatible());
        assert!(result.warnings.is_empty());
        assert!(result.nip11_available);
        assert!(
            result
                .observed_capabilities
                .contains(&gittree_core::RelayCapability::Nip34)
        );
    }

    #[test]
    fn probe_relay_warns_when_nip11_unavailable_and_propagates_errors() {
        let no_nip11 = StubProbeClient { response: None };
        let result = probe_relay("wss://relay.example", &no_nip11).expect("probe");
        assert!(!result.nip11_available);
        assert!(result.nip11.is_none());
        assert_eq!(
            result.warnings,
            vec!["nip-11 unavailable; falling back to active probes".to_string()]
        );

        let parse_err_client = StubProbeClient {
            response: Some("{bad-json"),
        };
        let parse_err = probe_relay("wss://relay.example", &parse_err_client).expect_err("parse");
        assert_parse_error(parse_err);

        let fetch_err = probe_relay("wss://relay.example", &ErrorProbeClient).expect_err("http");
        assert_http_error(fetch_err);
    }

    #[test]
    fn probe_relay_rejects_invalid_relay_url() {
        let client = StubProbeClient {
            response: Some(&NIP11_BODY),
        };
        let err = probe_relay("::invalid::", &client).expect_err("invalid relay url");
        assert_invalid_relay_url(err);
    }

    #[test]
    fn relay_probe_error_display_messages_are_stable() {
        assert_eq!(
            format!("{}", RelayProbeError::InvalidRelayUrl("bad".to_string())),
            "invalid relay url: bad"
        );
        assert_eq!(
            format!("{}", RelayProbeError::Http("timeout".to_string())),
            "relay probe http error: timeout"
        );
        assert_eq!(
            format!("{}", RelayProbeError::Parse("bad json".to_string())),
            "relay probe parse error: bad json"
        );
    }

    #[test]
    fn http_relay_probe_client_fetch_nip11_handles_statuses() {
        let client = HttpRelayProbeClient::new().expect("client");

        let (ok_url, ok_handle) = spawn_http_server(
            "HTTP/1.1 200 OK",
            r#"{"name":"relay","supported_nips":[1,11,34]}"#,
        );
        let ok = client.fetch_nip11(&ok_url).expect("ok response");
        ok_handle.join().expect("join");
        assert!(ok.is_some());

        let (not_found_url, not_found_handle) =
            spawn_http_server("HTTP/1.1 404 Not Found", "not found");
        let not_found = client
            .fetch_nip11(&not_found_url)
            .expect("404 should map to none");
        not_found_handle.join().expect("join");
        assert!(not_found.is_none());

        let (error_url, error_handle) =
            spawn_http_server("HTTP/1.1 500 Internal Server Error", "error");
        let error = client.fetch_nip11(&error_url).expect_err("status error");
        error_handle.join().expect("join");
        assert!(matches!(error, RelayProbeError::Http(message) if message.contains("status 500")));
    }

    #[test]
    fn http_relay_probe_client_new_rejects_invalid_user_agent() {
        let err = HttpRelayProbeClient::with_user_agent("gittree-relay-probe\n")
            .expect_err("invalid user-agent should fail");
        assert_http_error(err);
    }

    #[test]
    fn http_relay_probe_client_fetch_nip11_maps_body_read_errors() {
        let client = HttpRelayProbeClient::new().expect("client");
        let raw = b"HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\nconnection: close\r\n\r\nzz\r\nbody\r\n0\r\n\r\n";
        let (url, handle) = spawn_raw_http_server(raw);
        let err = client.fetch_nip11(&url).expect_err("invalid chunked body");
        handle.join().expect("join");
        assert_http_error(err);
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

        async fn publish_event(
            &self,
            _event: &gittree_relay_adapter::SignedNostrEvent,
        ) -> Result<(), RelayAdapterError> {
            Ok(())
        }
    }

    struct FailingAdapter;

    #[async_trait]
    impl RelayAdapter for FailingAdapter {
        async fn relay_info(&self) -> Result<Option<RelayInfoDocument>, RelayAdapterError> {
            Ok(None)
        }

        async fn probe_write_read(&self) -> Result<(), RelayAdapterError> {
            Err(RelayAdapterError::Protocol("probe failed".to_string()))
        }

        async fn publish_event(
            &self,
            _event: &gittree_relay_adapter::SignedNostrEvent,
        ) -> Result<(), RelayAdapterError> {
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
        assert!(result.report.is_compatible());
    }

    #[tokio::test]
    async fn probe_relay_with_adapter_records_active_failure() {
        let client = StubProbeClient {
            response: Some(&NIP11_BODY),
        };
        let result = probe_relay_with_adapter("wss://relay.example", &client, &FailingAdapter)
            .await
            .expect("probe");
        let active = result.active_probe.expect("active probe");
        assert!(!active.ok);
        assert!(
            active
                .error
                .expect("error")
                .contains("protocol error: probe failed")
        );
    }

    #[tokio::test]
    async fn probe_relay_with_adapter_rejects_invalid_relay_url() {
        let client = StubProbeClient {
            response: Some(&NIP11_BODY),
        };
        let err = probe_relay_with_adapter("::invalid::", &client, &OkAdapter)
            .await
            .expect_err("invalid relay url");
        assert_invalid_relay_url(err);
    }

    #[tokio::test]
    async fn relay_adapter_trait_methods_are_callable_in_tests() {
        let info = OkAdapter.relay_info().await.expect("relay info");
        assert!(info.is_none());

        let event = SignedNostrEvent {
            id: "id".to_string(),
            pubkey: "pubkey".to_string(),
            created_at: 0,
            kind: 1,
            tags: Vec::new(),
            content: String::new(),
            sig: "sig".to_string(),
        };
        OkAdapter.publish_event(&event).await.expect("publish");

        let failing_info = FailingAdapter.relay_info().await.expect("relay info");
        assert!(failing_info.is_none());
        FailingAdapter.publish_event(&event).await.expect("publish");
    }
}
