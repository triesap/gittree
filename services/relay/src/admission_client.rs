use gittree_core::{AdmissionDecision, EventFilter};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
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

pub trait AdmissionTransport: Send + Sync {
    fn send(
        &self,
        endpoint: &str,
        request: &AdmissionRequestPayload,
    ) -> Result<AdmissionDecisionPayload, AdmissionHookError>;
}

pub struct HttpAdmissionTransport {
    client: reqwest::blocking::Client,
}

impl HttpAdmissionTransport {
    pub fn new(timeout: Duration) -> Result<Self, AdmissionHookError> {
        let client = reqwest::blocking::Client::builder()
            .timeout(timeout)
            .build()
            .map_err(|err| AdmissionHookError::Transport(err.to_string()))?;
        Ok(Self { client })
    }
}

impl AdmissionTransport for HttpAdmissionTransport {
    fn send(
        &self,
        endpoint: &str,
        request: &AdmissionRequestPayload,
    ) -> Result<AdmissionDecisionPayload, AdmissionHookError> {
        let response = self
            .client
            .post(endpoint)
            .json(request)
            .send()
            .map_err(|err| AdmissionHookError::Transport(err.to_string()))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().unwrap_or_default();
            return Err(AdmissionHookError::Transport(format!(
                "admission error {status}: {body}"
            )));
        }

        response
            .json::<AdmissionDecisionPayload>()
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

    pub fn decide(&self, event: &RelayEvent) -> AdmissionDecision {
        let request = match AdmissionRequestPayload::try_from(event) {
            Ok(request) => request,
            Err(err) => {
                return self
                    .config
                    .fallback
                    .decision(&AdmissionHookError::Request(err));
            }
        };

        match self.transport.send(&self.config.endpoint, &request) {
            Ok(payload) => match admission_decision_from_payload(payload) {
                Ok(decision) => decision,
                Err(err) => self.config.fallback.decision(&err),
            },
            Err(err) => self.config.fallback.decision(&err),
        }
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
    use super::AdmissionDecisionPayload;
    use super::AdmissionFallback;
    use super::AdmissionFilter;
    use super::AdmissionHookClient;
    use super::AdmissionHookConfig;
    use super::AdmissionHookError;
    use super::AdmissionRequestPayload;
    use super::AdmissionTransport;
    use super::RelayEvent;
    use std::collections::BTreeMap;
    use std::sync::Mutex;
    use std::time::Duration;

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

    impl AdmissionTransport for MockTransport {
        fn send(
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

    #[test]
    fn client_sends_expected_request() {
        let transport = MockTransport::with_result(Ok(AdmissionDecisionPayload::Accept));
        let config = AdmissionHookConfig::new(
            "http://admission.local/decide",
            Duration::from_secs(1),
            AdmissionFallback::Reject,
        );
        let client = AdmissionHookClient::new(config, transport);
        let event = sample_event();

        let decision = client.decide(&event);
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

    #[test]
    fn client_falls_back_on_transport_error() {
        let transport =
            MockTransport::with_result(Err(AdmissionHookError::Transport("timeout".to_string())));
        let config = AdmissionHookConfig::new(
            "http://admission.local/decide",
            Duration::from_secs(1),
            AdmissionFallback::Reject,
        );
        let client = AdmissionHookClient::new(config, transport);
        let event = sample_event();

        let decision = client.decide(&event);
        match decision {
            gittree_core::AdmissionDecision::Reject { reason } => {
                assert!(reason.contains("admission unavailable"));
            }
            _ => panic!("expected reject fallback"),
        }
    }

    #[test]
    fn client_maps_requires_related_filters() {
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

        let decision = client.decide(&event);
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
}
