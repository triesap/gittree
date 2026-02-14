use gittree_core::{AdmissionDecision, EventFilter};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use async_trait::async_trait;
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdmissionHookConfig {
    pub endpoint: String,
    pub timeout: Duration,
    pub fallback: AdmissionFallback,
}

impl AdmissionHookConfig {
    pub fn new(
        endpoint: impl Into<String>,
        timeout: Duration,
        fallback: AdmissionFallback,
    ) -> Self {
        Self {
            endpoint: endpoint.into(),
            timeout,
            fallback,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdmissionFallback {
    Accept,
    Reject,
}

impl AdmissionFallback {
    fn decision(self, error: &AdmissionHookError) -> AdmissionDecision {
        match self {
            AdmissionFallback::Accept => AdmissionDecision::Accept,
            AdmissionFallback::Reject => AdmissionDecision::Reject {
                reason: format!("admission unavailable: {error}"),
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayEvent {
    pub kind: u64,
    pub pubkey: String,
    pub event_id: String,
    pub tags: Vec<Vec<String>>,
    pub relay_url: Option<String>,
    pub source_ip: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AdmissionRequestPayload {
    pub kind: u64,
    pub pubkey: String,
    pub event_id: String,
    pub tags: Vec<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub relay_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_ip: Option<String>,
}

impl TryFrom<&RelayEvent> for AdmissionRequestPayload {
    type Error = AdmissionRequestError;

    fn try_from(event: &RelayEvent) -> Result<Self, Self::Error> {
        if event.pubkey.is_empty() {
            return Err(AdmissionRequestError::MissingField("pubkey"));
        }

        if event.event_id.is_empty() {
            return Err(AdmissionRequestError::MissingField("event_id"));
        }

        if event.tags.iter().any(|tag| tag.is_empty()) {
            return Err(AdmissionRequestError::InvalidTag);
        }

        Ok(Self {
            kind: event.kind,
            pubkey: event.pubkey.clone(),
            event_id: event.event_id.clone(),
            tags: event.tags.clone(),
            relay_url: event.relay_url.clone(),
            source_ip: event.source_ip.clone(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum AdmissionDecisionPayload {
    Accept,
    Reject { reason: String },
    RequiresRelatedEvents { filters: Vec<AdmissionFilter> },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdmissionFilter {
    pub ids: Vec<String>,
    pub kinds: Vec<u32>,
    pub authors: Vec<String>,
    pub tags: BTreeMap<String, Vec<String>>,
    pub limit: Option<u64>,
}

impl From<AdmissionFilter> for EventFilter {
    fn from(filter: AdmissionFilter) -> Self {
        Self {
            ids: filter.ids,
            kinds: filter.kinds,
            authors: filter.authors,
            tags: filter.tags,
            limit: filter.limit,
        }
    }
}

impl From<&EventFilter> for AdmissionFilter {
    fn from(filter: &EventFilter) -> Self {
        Self {
            ids: filter.ids.clone(),
            kinds: filter.kinds.clone(),
            authors: filter.authors.clone(),
            tags: filter.tags.clone(),
            limit: filter.limit,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdmissionRequestError {
    MissingField(&'static str),
    InvalidTag,
    InvalidKind(u64),
}

impl std::fmt::Display for AdmissionRequestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AdmissionRequestError::MissingField(field) => {
                write!(f, "missing admission field {field}")
            }
            AdmissionRequestError::InvalidTag => write!(f, "invalid admission tag"),
            AdmissionRequestError::InvalidKind(kind) => {
                write!(f, "invalid admission kind {kind}")
            }
        }
    }
}

impl std::error::Error for AdmissionRequestError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdmissionHookError {
    Request(AdmissionRequestError),
    Serialize(String),
    Decode(String),
    Transport(String),
    InvalidDecision(String),
}

impl std::fmt::Display for AdmissionHookError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AdmissionHookError::Request(err) => write!(f, "invalid request: {err}"),
            AdmissionHookError::Serialize(err) => write!(f, "serialize error: {err}"),
            AdmissionHookError::Decode(err) => write!(f, "decode error: {err}"),
            AdmissionHookError::Transport(err) => write!(f, "transport error: {err}"),
            AdmissionHookError::InvalidDecision(err) => write!(f, "invalid decision: {err}"),
        }
    }
}

impl std::error::Error for AdmissionHookError {}

#[async_trait]
pub trait AdmissionTransport: Send + Sync {
    async fn send(
        &self,
        endpoint: &str,
        request: &AdmissionRequestPayload,
    ) -> Result<AdmissionDecisionPayload, AdmissionHookError>;
}

#[async_trait]
pub trait AdmissionDecider: Send + Sync {
    async fn decide(&self, event: &RelayEvent) -> AdmissionDecision;
}

pub struct HttpAdmissionTransport {
    client: reqwest::Client,
}

impl HttpAdmissionTransport {
    pub fn new(timeout: Duration) -> Result<Self, AdmissionHookError> {
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .map_err(|err| AdmissionHookError::Transport(err.to_string()))?;
        Ok(Self { client })
    }
}

#[async_trait]
impl AdmissionTransport for HttpAdmissionTransport {
    async fn send(
        &self,
        endpoint: &str,
        request: &AdmissionRequestPayload,
    ) -> Result<AdmissionDecisionPayload, AdmissionHookError> {
        let response = self
            .client
            .post(endpoint)
            .json(request)
            .send()
            .await
            .map_err(|err| AdmissionHookError::Transport(err.to_string()))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(AdmissionHookError::Transport(format!(
                "admission error {status}: {body}"
            )));
        }

        response
            .json::<AdmissionDecisionPayload>()
            .await
            .map_err(|err| AdmissionHookError::Decode(err.to_string()))
    }
}

pub struct AdmissionHookClient<T: AdmissionTransport> {
    config: AdmissionHookConfig,
    transport: T,
}

impl AdmissionHookClient<HttpAdmissionTransport> {
    pub fn new_http(config: AdmissionHookConfig) -> Result<Self, AdmissionHookError> {
        let transport = HttpAdmissionTransport::new(config.timeout)?;
        Ok(Self { config, transport })
    }
}

impl<T: AdmissionTransport> AdmissionHookClient<T> {
    pub fn new(config: AdmissionHookConfig, transport: T) -> Self {
        Self { config, transport }
    }

    pub async fn decide(&self, event: &RelayEvent) -> AdmissionDecision {
        let request = match AdmissionRequestPayload::try_from(event) {
            Ok(request) => request,
            Err(err) => {
                return self
                    .config
                    .fallback
                    .decision(&AdmissionHookError::Request(err));
            }
        };

        match self.transport.send(&self.config.endpoint, &request).await {
            Ok(payload) => match admission_decision_from_payload(payload) {
                Ok(decision) => decision,
                Err(err) => self.config.fallback.decision(&err),
            },
            Err(err) => self.config.fallback.decision(&err),
        }
    }
}

#[async_trait]
impl<T: AdmissionTransport> AdmissionDecider for AdmissionHookClient<T> {
    async fn decide(&self, event: &RelayEvent) -> AdmissionDecision {
        AdmissionHookClient::decide(self, event).await
    }
}

fn admission_decision_from_payload(
    payload: AdmissionDecisionPayload,
) -> Result<AdmissionDecision, AdmissionHookError> {
    match payload {
        AdmissionDecisionPayload::Accept => Ok(AdmissionDecision::Accept),
        AdmissionDecisionPayload::Reject { reason } => {
            if reason.trim().is_empty() {
                return Err(AdmissionHookError::InvalidDecision(
                    "missing reject reason".to_string(),
                ));
            }
            Ok(AdmissionDecision::Reject { reason })
        }
        AdmissionDecisionPayload::RequiresRelatedEvents { filters } => {
            if filters.is_empty() {
                return Err(AdmissionHookError::InvalidDecision(
                    "missing related filters".to_string(),
                ));
            }
            Ok(AdmissionDecision::RequiresRelatedEvents {
                filters: filters.into_iter().map(EventFilter::from).collect(),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::admission_decision_from_payload;
    use super::AdmissionDecisionPayload;
    use super::AdmissionRequestError;
    use super::AdmissionFallback;
    use super::AdmissionFilter;
    use super::AdmissionHookClient;
    use super::AdmissionHookConfig;
    use super::AdmissionHookError;
    use super::AdmissionRequestPayload;
    use super::AdmissionTransport;
    use super::HttpAdmissionTransport;
    use super::RelayEvent;
    use axum::Json;
    use axum::Router;
    use axum::http::StatusCode;
    use axum::routing::post;
    use async_trait::async_trait;
    use gittree_core::AdmissionDecision;
    use gittree_core::EventFilter;
    use std::future::IntoFuture;
    use std::collections::BTreeMap;
    use std::sync::Mutex;
    use std::time::Duration;
    use tokio::net::TcpListener;

    struct MockTransport {
        calls: Mutex<Vec<(String, AdmissionRequestPayload)>>,
        result: Mutex<Result<AdmissionDecisionPayload, AdmissionHookError>>,
    }

    impl MockTransport {
        fn with_result(result: Result<AdmissionDecisionPayload, AdmissionHookError>) -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
                result: Mutex::new(result),
            }
        }
    }

    #[async_trait]
    impl AdmissionTransport for MockTransport {
        async fn send(
            &self,
            endpoint: &str,
            request: &AdmissionRequestPayload,
        ) -> Result<AdmissionDecisionPayload, AdmissionHookError> {
            let mut calls = self.calls.lock().expect("calls lock");
            calls.push((endpoint.to_string(), request.clone()));
            let result = self.result.lock().expect("result lock");
            result.clone()
        }
    }

    fn sample_event() -> RelayEvent {
        RelayEvent {
            kind: 30333,
            pubkey: "pubkey".to_string(),
            event_id: "eventid".to_string(),
            tags: vec![vec!["a".to_string(), "b".to_string()]],
            relay_url: Some("wss://relay.example".to_string()),
            source_ip: Some("127.0.0.1".to_string()),
        }
    }

    fn sample_request_payload() -> AdmissionRequestPayload {
        AdmissionRequestPayload::try_from(&sample_event()).expect("request payload")
    }

    async fn spawn_error_server(status: StatusCode, body: &'static str) -> String {
        let app = Router::new().route(
            "/decide",
            post(move || async move {
                (
                    status,
                    [(axum::http::header::CONTENT_TYPE, "text/plain")],
                    body,
                )
            }),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("local addr");
        tokio::spawn(axum::serve(listener, app).into_future());
        format!("http://{addr}/decide")
    }

    async fn spawn_accept_server() -> String {
        let app = Router::new().route(
            "/decide",
            post(|| async { Json(AdmissionDecisionPayload::Accept) }),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("local addr");
        tokio::spawn(axum::serve(listener, app).into_future());
        format!("http://{addr}/decide")
    }

    #[tokio::test]
    async fn client_sends_expected_request() {
        let transport = MockTransport::with_result(Ok(AdmissionDecisionPayload::Accept));
        let config = AdmissionHookConfig::new(
            "http://admission.local/decide",
            Duration::from_secs(1),
            AdmissionFallback::Reject,
        );
        let client = AdmissionHookClient::new(config, transport);
        let event = sample_event();

        let decision = client.decide(&event).await;
        assert!(matches!(decision, gittree_core::AdmissionDecision::Accept));

        let calls = client.transport.calls.lock().expect("calls lock");
        assert_eq!(calls.len(), 1);
        let (endpoint, payload) = &calls[0];
        assert_eq!(endpoint, "http://admission.local/decide");
        assert_eq!(payload.kind, event.kind);
        assert_eq!(payload.pubkey, event.pubkey);
        assert_eq!(payload.event_id, event.event_id);
        assert_eq!(payload.tags, event.tags);
        assert_eq!(payload.relay_url, event.relay_url);
        assert_eq!(payload.source_ip, event.source_ip);
    }

    #[tokio::test]
    async fn client_falls_back_on_transport_error() {
        let transport =
            MockTransport::with_result(Err(AdmissionHookError::Transport("timeout".to_string())));
        let config = AdmissionHookConfig::new(
            "http://admission.local/decide",
            Duration::from_secs(1),
            AdmissionFallback::Reject,
        );
        let client = AdmissionHookClient::new(config, transport);
        let event = sample_event();

        let decision = client.decide(&event).await;
        match decision {
            gittree_core::AdmissionDecision::Reject { reason } => {
                assert!(reason.contains("admission unavailable"));
            }
            _ => panic!("expected reject fallback"),
        }
    }

    #[tokio::test]
    async fn client_maps_requires_related_filters() {
        let mut tags = BTreeMap::new();
        tags.insert("e".to_string(), vec!["event".to_string()]);
        let filter = AdmissionFilter {
            ids: vec!["event".to_string()],
            kinds: vec![1],
            authors: vec!["pub".to_string()],
            tags,
            limit: Some(1),
        };
        let transport =
            MockTransport::with_result(Ok(AdmissionDecisionPayload::RequiresRelatedEvents {
                filters: vec![filter],
            }));
        let config = AdmissionHookConfig::new(
            "http://admission.local/decide",
            Duration::from_secs(1),
            AdmissionFallback::Reject,
        );
        let client = AdmissionHookClient::new(config, transport);
        let event = sample_event();

        let decision = client.decide(&event).await;
        match decision {
            gittree_core::AdmissionDecision::RequiresRelatedEvents { filters } => {
                assert_eq!(filters.len(), 1);
                assert_eq!(filters[0].authors, vec!["pub".to_string()]);
                assert_eq!(filters[0].ids, vec!["event".to_string()]);
                assert_eq!(filters[0].limit, Some(1));
            }
            _ => panic!("expected related-event decision"),
        }
    }

    #[tokio::test]
    async fn client_reject_fallback_on_invalid_event_request() {
        let transport = MockTransport::with_result(Ok(AdmissionDecisionPayload::Accept));
        let config = AdmissionHookConfig::new(
            "http://admission.local/decide",
            Duration::from_secs(1),
            AdmissionFallback::Reject,
        );
        let client = AdmissionHookClient::new(config, transport);
        let mut event = sample_event();
        event.pubkey = String::new();

        let decision = client.decide(&event).await;
        assert!(matches!(decision, AdmissionDecision::Reject { .. }));
        if let AdmissionDecision::Reject { reason } = decision {
            assert!(reason.contains("admission unavailable"));
            assert!(reason.contains("invalid request"));
        }

        let calls = client.transport.calls.lock().expect("calls lock");
        assert!(calls.is_empty());
    }

    #[tokio::test]
    async fn client_accept_fallback_on_invalid_decision_payload() {
        let transport = MockTransport::with_result(Ok(AdmissionDecisionPayload::Reject {
            reason: " ".to_string(),
        }));
        let config = AdmissionHookConfig::new(
            "http://admission.local/decide",
            Duration::from_secs(1),
            AdmissionFallback::Accept,
        );
        let client = AdmissionHookClient::new(config, transport);
        let event = sample_event();

        let decision = client.decide(&event).await;
        assert!(matches!(decision, gittree_core::AdmissionDecision::Accept));
    }

    #[test]
    fn decision_payload_validation_rejects_empty_reason_and_filters() {
        let reject = admission_decision_from_payload(AdmissionDecisionPayload::Reject {
            reason: String::new(),
        });
        assert!(matches!(reject, Err(AdmissionHookError::InvalidDecision(_))));

        let related = admission_decision_from_payload(
            AdmissionDecisionPayload::RequiresRelatedEvents { filters: Vec::new() },
        );
        assert!(matches!(related, Err(AdmissionHookError::InvalidDecision(_))));
    }

    #[test]
    fn request_payload_validation_rejects_missing_event_id_and_invalid_tag() {
        let mut event = sample_event();
        event.event_id = String::new();
        let missing_event_id = AdmissionRequestPayload::try_from(&event).unwrap_err();
        assert!(matches!(
            missing_event_id,
            AdmissionRequestError::MissingField("event_id")
        ));

        let mut invalid_tag_event = sample_event();
        invalid_tag_event.tags = vec![Vec::new()];
        let invalid_tag = AdmissionRequestPayload::try_from(&invalid_tag_event).unwrap_err();
        assert!(matches!(invalid_tag, AdmissionRequestError::InvalidTag));
    }

    #[test]
    fn admission_filter_from_event_filter_clones_all_fields() {
        let mut tags = BTreeMap::new();
        tags.insert("e".to_string(), vec!["event-id".to_string()]);
        let filter = EventFilter {
            ids: vec!["1".to_string(), "2".to_string()],
            kinds: vec![1, 30000],
            authors: vec!["author".to_string()],
            tags: tags.clone(),
            limit: Some(5),
        };
        let payload = AdmissionFilter::from(&filter);
        assert_eq!(payload.ids, filter.ids);
        assert_eq!(payload.kinds, filter.kinds);
        assert_eq!(payload.authors, filter.authors);
        assert_eq!(payload.tags, tags);
        assert_eq!(payload.limit, Some(5));

        let round_trip: EventFilter = payload.into();
        assert_eq!(round_trip.limit, Some(5));
        assert_eq!(round_trip.tags["e"], vec!["event-id".to_string()]);
    }

    #[test]
    fn admission_error_display_variants_are_stable() {
        assert_eq!(
            AdmissionRequestError::InvalidTag.to_string(),
            "invalid admission tag"
        );
        assert_eq!(
            AdmissionRequestError::InvalidKind(7).to_string(),
            "invalid admission kind 7"
        );
        assert_eq!(
            AdmissionHookError::Serialize("boom".to_string()).to_string(),
            "serialize error: boom"
        );
        assert_eq!(
            AdmissionHookError::Decode("bad json".to_string()).to_string(),
            "decode error: bad json"
        );
        assert_eq!(
            AdmissionHookError::Transport("timeout".to_string()).to_string(),
            "transport error: timeout"
        );
        assert_eq!(
            AdmissionHookError::InvalidDecision("missing".to_string()).to_string(),
            "invalid decision: missing"
        );
    }

    #[test]
    fn decision_payload_reject_with_reason_maps_to_reject_decision() {
        let decision = admission_decision_from_payload(AdmissionDecisionPayload::Reject {
            reason: "denied".to_string(),
        })
        .expect("decision");
        assert_eq!(
            decision,
            AdmissionDecision::Reject {
                reason: "denied".to_string(),
            }
        );
    }

    #[tokio::test]
    async fn http_transport_maps_connection_errors() {
        let transport = HttpAdmissionTransport::new(Duration::from_millis(50)).expect("transport");
        let err = transport
            .send("http://127.0.0.1:1/decide", &sample_request_payload())
            .await
            .unwrap_err();
        assert!(matches!(err, AdmissionHookError::Transport(_)));
    }

    #[tokio::test]
    async fn http_transport_maps_non_success_statuses() {
        let endpoint = spawn_error_server(StatusCode::FORBIDDEN, "denied").await;
        let transport = HttpAdmissionTransport::new(Duration::from_secs(1)).expect("transport");
        let err = transport
            .send(&endpoint, &sample_request_payload())
            .await
            .unwrap_err();
        match err {
            AdmissionHookError::Transport(message) => {
                assert!(message.contains("admission error 403 Forbidden"));
                assert!(message.contains("denied"));
            }
            other => panic!("expected transport error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn http_transport_maps_decode_errors() {
        let endpoint = spawn_error_server(StatusCode::OK, "not-json").await;
        let transport = HttpAdmissionTransport::new(Duration::from_secs(1)).expect("transport");
        let err = transport
            .send(&endpoint, &sample_request_payload())
            .await
            .unwrap_err();
        assert!(matches!(err, AdmissionHookError::Decode(_)));
    }

    #[tokio::test]
    async fn new_http_client_constructs_transport_and_accepts() {
        let endpoint = spawn_accept_server().await;
        let config = AdmissionHookConfig::new(
            endpoint,
            Duration::from_secs(1),
            AdmissionFallback::Reject,
        );
        let client = AdmissionHookClient::new_http(config).expect("client");
        let decision = client.decide(&sample_event()).await;
        assert!(matches!(decision, AdmissionDecision::Accept));
    }

    async fn decide_via_trait(
        decider: &(dyn super::AdmissionDecider + Send + Sync),
        event: &RelayEvent,
    ) -> AdmissionDecision {
        decider.decide(event).await
    }

    #[tokio::test]
    async fn admission_decider_trait_dispatches_to_client() {
        let transport = MockTransport::with_result(Ok(AdmissionDecisionPayload::Accept));
        let config = AdmissionHookConfig::new(
            "http://admission.local/decide",
            Duration::from_secs(1),
            AdmissionFallback::Reject,
        );
        let client = AdmissionHookClient::new(config, transport);
        let decision = decide_via_trait(&client, &sample_event()).await;
        assert!(matches!(decision, AdmissionDecision::Accept));
    }

    #[tokio::test]
    async fn admission_decider_trait_dispatches_to_http_client() {
        let endpoint = spawn_accept_server().await;
        let config = AdmissionHookConfig::new(
            endpoint,
            Duration::from_secs(1),
            AdmissionFallback::Reject,
        );
        let client = AdmissionHookClient::new_http(config).expect("client");
        let decision = decide_via_trait(&client, &sample_event()).await;
        assert!(matches!(decision, AdmissionDecision::Accept));
    }
}
