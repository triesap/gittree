use crate::{
    AdmissionDecider, ClientMessage, EventStore, Filter, Notice, Policy, RelayEvent, RelayMetrics,
    ServerMessage, StoreError, StoreOutcome, SubscriptionId, SubscriptionRegistry,
    decode_client_message,
};
use gittree_core::AdmissionDecision;
use serde_json::Value;
use secp256k1::rand::{rngs::OsRng, RngCore};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::broadcast;

const AUTH_KIND: u32 = 22242;

struct AuthState {
    challenge: String,
    authenticated_pubkey: Option<String>,
}

impl AuthState {
    fn new() -> Self {
        Self {
            challenge: generate_challenge(),
            authenticated_pubkey: None,
        }
    }
}

struct RateLimitConfig {
    max_events_per_min: Option<u64>,
    max_requests_per_min: Option<u64>,
    window: Duration,
}

impl RateLimitConfig {
    fn from_policy(policy: &Policy) -> Option<Self> {
        if policy.max_events_per_min.is_none() && policy.max_requests_per_min.is_none() {
            return None;
        }
        Some(Self {
            max_events_per_min: policy.max_events_per_min,
            max_requests_per_min: policy.max_requests_per_min,
            window: Duration::from_secs(60),
        })
    }
}

#[derive(Clone, Copy)]
struct RateLimitCounter {
    count: u64,
    window_start: Instant,
}

struct RateLimiter {
    config: RateLimitConfig,
    events: RateLimitCounter,
    requests: RateLimitCounter,
}

impl RateLimiter {
    fn new(config: RateLimitConfig) -> Self {
        let now = Instant::now();
        Self {
            config,
            events: RateLimitCounter {
                count: 0,
                window_start: now,
            },
            requests: RateLimitCounter {
                count: 0,
                window_start: now,
            },
        }
    }

    fn check_event(&mut self) -> bool {
        let limit = self.config.max_events_per_min;
        let window = self.config.window;
        Self::hit_limit(&mut self.events, limit, window)
    }

    fn check_request(&mut self) -> bool {
        let limit = self.config.max_requests_per_min;
        let window = self.config.window;
        Self::hit_limit(&mut self.requests, limit, window)
    }

    fn hit_limit(
        counter: &mut RateLimitCounter,
        limit: Option<u64>,
        window: Duration,
    ) -> bool {
        let Some(limit) = limit else {
            return false;
        };
        if limit == 0 {
            return false;
        }
        let now = Instant::now();
        if now.duration_since(counter.window_start) >= window {
            counter.count = 0;
            counter.window_start = now;
        }
        if counter.count >= limit {
            return true;
        }
        counter.count += 1;
        false
    }
}

pub struct Session<S: EventStore> {
    registry: SubscriptionRegistry,
    store: S,
    policy: Policy,
    admission: Option<Arc<dyn AdmissionDecider>>,
    broadcast: Option<broadcast::Sender<crate::NostrEvent>>,
    auth: Option<AuthState>,
    rate_limiter: Option<RateLimiter>,
    metrics: Option<Arc<RelayMetrics>>,
}

impl<S: EventStore> Session<S> {
    pub fn new(store: S) -> Self {
        let policy = Policy::default();
        Self {
            registry: SubscriptionRegistry::default(),
            store,
            policy,
            admission: None,
            broadcast: None,
            auth: None,
            rate_limiter: RateLimitConfig::from_policy(&policy).map(RateLimiter::new),
            metrics: None,
        }
    }

    pub fn with_policy(store: S, policy: Policy) -> Self {
        Self {
            registry: SubscriptionRegistry::default(),
            store,
            policy,
            admission: None,
            broadcast: None,
            auth: None,
            rate_limiter: RateLimitConfig::from_policy(&policy).map(RateLimiter::new),
            metrics: None,
        }
    }

    pub fn with_admission(store: S, admission: Arc<dyn AdmissionDecider>) -> Self {
        let policy = Policy::default();
        Self {
            registry: SubscriptionRegistry::default(),
            store,
            policy,
            admission: Some(admission),
            broadcast: None,
            auth: None,
            rate_limiter: RateLimitConfig::from_policy(&policy).map(RateLimiter::new),
            metrics: None,
        }
    }

    pub fn with_policy_and_admission(
        store: S,
        policy: Policy,
        admission: Arc<dyn AdmissionDecider>,
    ) -> Self {
        Self {
            registry: SubscriptionRegistry::default(),
            store,
            policy,
            admission: Some(admission),
            broadcast: None,
            auth: None,
            rate_limiter: RateLimitConfig::from_policy(&policy).map(RateLimiter::new),
            metrics: None,
        }
    }

    pub fn with_policy_and_auth(store: S, policy: Policy, auth_required: bool) -> Self {
        Self {
            registry: SubscriptionRegistry::default(),
            store,
            policy,
            admission: None,
            broadcast: None,
            auth: auth_state(auth_required),
            rate_limiter: RateLimitConfig::from_policy(&policy).map(RateLimiter::new),
            metrics: None,
        }
    }

    pub fn with_broadcast(
        store: S,
        policy: Policy,
        admission: Option<Arc<dyn AdmissionDecider>>,
        broadcast: broadcast::Sender<crate::NostrEvent>,
        auth_required: bool,
    ) -> Self {
        Self {
            registry: SubscriptionRegistry::default(),
            store,
            policy,
            admission,
            broadcast: Some(broadcast),
            auth: auth_state(auth_required),
            rate_limiter: RateLimitConfig::from_policy(&policy).map(RateLimiter::new),
            metrics: None,
        }
    }

    pub fn with_metrics(mut self, metrics: Arc<RelayMetrics>) -> Self {
        self.metrics = Some(metrics);
        self
    }

    pub fn registry(&self) -> &SubscriptionRegistry {
        &self.registry
    }

    pub fn max_message_bytes(&self) -> Option<usize> {
        self.policy.max_message_bytes
    }

    pub fn auth_challenge(&self) -> Option<String> {
        self.auth.as_ref().map(|state| state.challenge.clone())
    }

    pub fn initial_messages(&self) -> Vec<ServerMessage> {
        match &self.auth {
            Some(state) => vec![ServerMessage::Auth {
                challenge: state.challenge.clone(),
            }],
            None => Vec::new(),
        }
    }

    pub fn dispatch_event(&self, event: &crate::NostrEvent) -> Vec<ServerMessage> {
        let event_value = match serde_json::to_value(event) {
            Ok(value) => value,
            Err(_) => return vec![Notice::message("failed to serialize event").into()],
        };

        let tags = match crate::TagIndex::from_tags(&event.tags) {
            Ok(tags) => tags,
            Err(_) => return vec![Notice::message("invalid event tags").into()],
        };

        let mut responses = Vec::new();
        for (sub_id, filters) in self.registry.iter() {
            if filters.is_empty() {
                continue;
            }
            let matches = filters.iter().any(|filter| filter.matches(event, &tags));
            if matches {
                responses.push(ServerMessage::Event {
                    subscription_id: sub_id.as_str().to_string(),
                    event: event_value.clone(),
                });
            }
        }
        responses
    }

    pub async fn handle_raw(&mut self, input: &str) -> Vec<ServerMessage> {
        match decode_client_message(input) {
            Ok(message) => self.handle_message(message).await,
            Err(err) => vec![Notice::from(err).into()],
        }
    }

    pub async fn handle_message(&mut self, message: ClientMessage) -> Vec<ServerMessage> {
        match message {
            ClientMessage::Req {
                subscription_id,
                filters,
            } => {
                self.record_message("REQ");
                if let Some(notice) = self.rate_limit_request() {
                    return vec![notice.into()];
                }
                let parsed = match parse_filters(&filters) {
                    Ok(filters) => filters,
                    Err(err) => return vec![Notice::from(err).into()],
                };
                if let Some(notice) = self.validate_filter_limits(&parsed) {
                    return vec![notice.into()];
                }

                let sub_id = SubscriptionId::new(subscription_id.clone());
                if let Some(max) = self.policy.max_subscriptions {
                    if !self.registry.contains(&sub_id) && self.registry.len() >= max {
                        return vec![Notice::message("too many subscriptions").into()];
                    }
                }

                let events = match self.store.query(&parsed).await {
                    Ok(events) => events,
                    Err(err) => return vec![Notice::from(err).into()],
                };

                self.registry.insert(sub_id.clone(), parsed.clone());

                let mut responses = Vec::new();
                for event in events {
                    let value = match serde_json::to_value(event) {
                        Ok(value) => value,
                        Err(_) => return vec![Notice::message("failed to serialize event").into()],
                    };
                    responses.push(ServerMessage::Event {
                        subscription_id: subscription_id.clone(),
                        event: value,
                    });
                }
                self.registry.mark_eose(&sub_id);
                responses.push(ServerMessage::Eose {
                    subscription_id,
                });
                responses
            }
            ClientMessage::Close { subscription_id } => {
                self.record_message("CLOSE");
                self.registry.remove(&SubscriptionId::new(subscription_id));
                Vec::new()
            }
            ClientMessage::Event(value) => {
                self.record_message("EVENT");
                self.handle_event(value).await
            }
            ClientMessage::Auth(value) => {
                self.record_message("AUTH");
                self.handle_auth(value).await
            }
            ClientMessage::Count {
                subscription_id,
                filters,
            } => {
                self.record_message("COUNT");
                if let Some(notice) = self.rate_limit_request() {
                    return vec![notice.into()];
                }
                let parsed = match parse_filters(&filters) {
                    Ok(filters) => filters,
                    Err(err) => return vec![Notice::from(err).into()],
                };
                if let Some(notice) = self.validate_filter_limits(&parsed) {
                    return vec![notice.into()];
                }
                let count = match self.store.query(&parsed).await {
                    Ok(events) => events.len() as u64,
                    Err(err) => return vec![Notice::from(err).into()],
                };
                vec![ServerMessage::Count {
                    subscription_id,
                    count,
                }]
            }
        }
    }

    fn validate_filter_limits(&self, filters: &[Filter]) -> Option<Notice> {
        let Some(max_limit) = self.policy.max_limit else {
            return None;
        };
        let exceeded = filters.iter().any(|filter| {
            filter
                .limit
                .map(|limit| limit > max_limit)
                .unwrap_or(false)
        });
        if exceeded {
            Some(Notice::message("limit too large"))
        } else {
            None
        }
    }

    fn rate_limit_event(&mut self) -> Option<Notice> {
        if let Some(limiter) = &mut self.rate_limiter {
            if limiter.check_event() {
                return Some(Notice::message("rate limited"));
            }
        }
        None
    }

    fn rate_limit_request(&mut self) -> Option<Notice> {
        if let Some(limiter) = &mut self.rate_limiter {
            if limiter.check_request() {
                return Some(Notice::message("rate limited"));
            }
        }
        None
    }

    fn record_message(&self, kind: &str) {
        if let Some(metrics) = &self.metrics {
            metrics.record_message(kind);
        }
    }

    fn record_event(&self, status: &str) {
        if let Some(metrics) = &self.metrics {
            metrics.record_event(status);
        }
    }

    async fn handle_event(&mut self, value: Value) -> Vec<ServerMessage> {
        let event: crate::NostrEvent = match serde_json::from_value(value) {
            Ok(event) => event,
            Err(_) => {
                self.record_event("rejected");
                return vec![Notice::message("invalid event").into()];
            }
        };
        if let Some(notice) = self.rate_limit_event() {
            self.record_event("rejected");
            return vec![notice.into()];
        }

        if let Err(err) = event.verify() {
            self.record_event("rejected");
            return vec![ServerMessage::Ok {
                event_id: event.id.clone(),
                accepted: false,
                message: err.to_string(),
            }];
        }

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        if let Err(err) = self.policy.validate_event(&event, now) {
            self.record_event("rejected");
            return vec![ServerMessage::Ok {
                event_id: event.id.clone(),
                accepted: false,
                message: err.to_string(),
            }];
        }

        if let Some(auth) = &self.auth {
            match auth.authenticated_pubkey.as_ref() {
                None => {
                    self.record_event("rejected");
                    return vec![ServerMessage::Ok {
                        event_id: event.id.clone(),
                        accepted: false,
                        message: "auth required".to_string(),
                    }];
                }
                Some(pubkey) if pubkey != &event.pubkey => {
                    self.record_event("rejected");
                    return vec![ServerMessage::Ok {
                        event_id: event.id.clone(),
                        accepted: false,
                        message: "auth pubkey mismatch".to_string(),
                    }];
                }
                Some(_) => {}
            }
        }

        if let Some(admission) = &self.admission {
            let relay_event = RelayEvent {
                kind: event.kind as u64,
                pubkey: event.pubkey.clone(),
                event_id: event.id.clone(),
                tags: event.tags.clone(),
                relay_url: None,
                source_ip: None,
            };
            let decision = admission.decide(&relay_event).await;
            match decision {
                AdmissionDecision::Accept => {}
                AdmissionDecision::Reject { reason } => {
                    self.record_event("rejected");
                    return vec![ServerMessage::Ok {
                        event_id: event.id.clone(),
                        accepted: false,
                        message: reason,
                    }];
                }
                AdmissionDecision::RequiresRelatedEvents { filters } => {
                    for filter in filters {
                        let relay_filter = filter_from_event_filter(&filter);
                        let related = match self.store.query(&[relay_filter]).await {
                            Ok(related) => related,
                            Err(err) => {
                                self.record_event("rejected");
                                return vec![Notice::from(err).into()];
                            }
                        };
                        if related.is_empty() {
                            self.record_event("rejected");
                            return vec![ServerMessage::Ok {
                                event_id: event.id.clone(),
                                accepted: false,
                                message: "missing related events".to_string(),
                            }];
                        }
                    }
                }
            }
        }

        let outcome = match self.store.insert(event.clone()).await {
            Ok(outcome) => outcome,
            Err(err) => {
                self.record_event("rejected");
                return vec![Notice::from(err).into()];
            }
        };

        if matches!(outcome, StoreOutcome::Inserted) {
            let _ = self.apply_retention(now).await;
        }

        let mut responses = Vec::new();
        let message = match outcome {
            StoreOutcome::Inserted => {
                self.record_event("accepted");
                "saved".to_string()
            }
            StoreOutcome::Duplicate => {
                self.record_event("duplicate");
                "duplicate".to_string()
            }
        };
        responses.push(ServerMessage::Ok {
            event_id: event.id.clone(),
            accepted: true,
            message,
        });

        if matches!(outcome, StoreOutcome::Inserted) {
            if let Some(broadcast) = &self.broadcast {
                let _ = broadcast.send(event.clone());
            }
        }

        responses.extend(self.dispatch_event(&event));

        responses
    }

    async fn apply_retention(&self, now: i64) -> Result<(), StoreError> {
        let Some(max_age) = self.policy.retention_max_age_seconds else {
            return Ok(());
        };
        if max_age <= 0 {
            return Ok(());
        }
        let cutoff = now.saturating_sub(max_age);
        let filter = Filter {
            ids: Vec::new(),
            authors: Vec::new(),
            kinds: Vec::new(),
            since: None,
            until: Some(cutoff),
            limit: Some(100),
            tags: BTreeMap::new(),
        };

        loop {
            let events = self.store.query(&[filter.clone()]).await?;
            if events.is_empty() {
                break;
            }
            for event in events {
                let _ = self.store.delete(&event.id).await?;
            }
        }
        Ok(())
    }

    async fn handle_auth(&mut self, value: Value) -> Vec<ServerMessage> {
        let event: crate::NostrEvent = match serde_json::from_value(value) {
            Ok(event) => event,
            Err(_) => return vec![Notice::message("invalid auth event").into()],
        };

        if event.kind != AUTH_KIND {
            return vec![ServerMessage::Ok {
                event_id: event.id.clone(),
                accepted: false,
                message: "invalid auth kind".to_string(),
            }];
        }

        if let Err(err) = event.verify() {
            return vec![ServerMessage::Ok {
                event_id: event.id.clone(),
                accepted: false,
                message: err.to_string(),
            }];
        }

        let Some(auth) = &mut self.auth else {
            return vec![ServerMessage::Ok {
                event_id: event.id.clone(),
                accepted: false,
                message: "auth not enabled".to_string(),
            }];
        };

        let challenge = match find_tag_value(&event.tags, "challenge") {
            Some(value) => value,
            None => {
                return vec![ServerMessage::Ok {
                    event_id: event.id.clone(),
                    accepted: false,
                    message: "missing challenge tag".to_string(),
                }];
            }
        };

        if challenge != auth.challenge {
            return vec![ServerMessage::Ok {
                event_id: event.id.clone(),
                accepted: false,
                message: "invalid challenge".to_string(),
            }];
        }

        auth.authenticated_pubkey = Some(event.pubkey.clone());

        vec![ServerMessage::Ok {
            event_id: event.id.clone(),
            accepted: true,
            message: "authenticated".to_string(),
        }]
    }
}

fn auth_state(required: bool) -> Option<AuthState> {
    if required {
        Some(AuthState::new())
    } else {
        None
    }
}

fn generate_challenge() -> String {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    hex::encode(bytes)
}

fn find_tag_value(tags: &[Vec<String>], name: &str) -> Option<String> {
    tags.iter()
        .find(|tag| tag.first().map(|entry| entry == name).unwrap_or(false))
        .and_then(|tag| tag.get(1).cloned())
}

fn parse_filters(filters: &[Value]) -> Result<Vec<Filter>, crate::FilterError> {
    filters
        .iter()
        .map(Filter::from_json)
        .collect::<Result<Vec<_>, _>>()
}

fn filter_from_event_filter(filter: &gittree_core::EventFilter) -> Filter {
    Filter {
        ids: filter.ids.clone(),
        kinds: filter.kinds.clone(),
        authors: filter.authors.clone(),
        tags: filter.tags.clone(),
        since: None,
        until: None,
        limit: filter.limit,
    }
}

#[cfg(test)]
mod tests {
    use super::Session;
    use crate::{
        AdmissionDecider, ClientMessage, EventStore, MemoryStore, NostrEvent, Policy,
        ServerMessage,
    };
    use async_trait::async_trait;
    use gittree_core::{AdmissionDecision, EventFilter};
    use secp256k1::{Keypair, Secp256k1, SecretKey};
    use serde_json::json;
    use std::sync::Arc;

    #[tokio::test]
    async fn handle_raw_reports_invalid_messages() {
        let mut session = Session::new(MemoryStore::new());
        let responses = session.handle_raw("{\"bad\":true}").await;
        assert_eq!(responses.len(), 1);
        assert!(matches!(responses[0], ServerMessage::Notice { .. }));
    }

    #[tokio::test]
    async fn handle_message_updates_subscriptions() {
        let mut session = Session::new(MemoryStore::new());
        session
            .handle_message(ClientMessage::Req {
            subscription_id: "sub".to_string(),
            filters: vec![json!({})],
            })
            .await;
        assert!(session.registry().contains(&crate::SubscriptionId::new("sub")));

        session
            .handle_message(ClientMessage::Close {
            subscription_id: "sub".to_string(),
            })
            .await;
        assert!(!session.registry().contains(&crate::SubscriptionId::new("sub")));
    }

    #[tokio::test]
    async fn req_sends_events_and_eose() {
        let store = MemoryStore::new();
        let event = NostrEvent {
            id: "id".to_string(),
            pubkey: "aa".repeat(32),
            created_at: 1,
            kind: 1,
            tags: Vec::new(),
            content: String::new(),
            sig: "00".repeat(64),
        };
        store.insert(event).await.expect("insert");

        let mut session = Session::new(store);
        let responses = session
            .handle_message(ClientMessage::Req {
            subscription_id: "sub".to_string(),
            filters: vec![json!({})],
            })
            .await;

        assert_eq!(responses.len(), 2);
        assert!(matches!(responses[0], ServerMessage::Event { .. }));
        assert!(matches!(responses[1], ServerMessage::Eose { .. }));
        assert!(session.registry().eose_sent(&crate::SubscriptionId::new("sub")));
    }

    #[tokio::test]
    async fn count_returns_match_count() {
        let store = MemoryStore::new();
        let event = NostrEvent {
            id: "id".to_string(),
            pubkey: "aa".repeat(32),
            created_at: 1,
            kind: 1,
            tags: Vec::new(),
            content: String::new(),
            sig: "00".repeat(64),
        };
        store.insert(event).await.expect("insert");

        let mut session = Session::new(store);
        let responses = session
            .handle_message(ClientMessage::Count {
                subscription_id: "sub".to_string(),
                filters: vec![json!({})],
            })
            .await;

        assert_eq!(responses.len(), 1);
        assert!(matches!(
            responses[0],
            ServerMessage::Count { count: 1, .. }
        ));
    }

    #[tokio::test]
    async fn req_rejects_when_subscription_limit_reached() {
        let store = MemoryStore::new();
        let policy = Policy {
            max_subscriptions: Some(1),
            ..Policy::default()
        };
        let mut session = Session::with_policy(store, policy);
        let _ = session
            .handle_message(ClientMessage::Req {
                subscription_id: "sub-1".to_string(),
                filters: vec![json!({})],
            })
            .await;

        let responses = session
            .handle_message(ClientMessage::Req {
                subscription_id: "sub-2".to_string(),
                filters: vec![json!({})],
            })
            .await;

        assert_eq!(responses.len(), 1);
        assert!(matches!(responses[0], ServerMessage::Notice { .. }));
        assert!(session.registry().contains(&crate::SubscriptionId::new("sub-1")));
        assert!(!session.registry().contains(&crate::SubscriptionId::new("sub-2")));
    }

    #[tokio::test]
    async fn req_rejects_when_limit_exceeds_policy() {
        let store = MemoryStore::new();
        let policy = Policy {
            max_limit: Some(1),
            ..Policy::default()
        };
        let mut session = Session::with_policy(store, policy);

        let responses = session
            .handle_message(ClientMessage::Req {
                subscription_id: "sub".to_string(),
                filters: vec![json!({"limit": 2})],
            })
            .await;

        assert_eq!(responses.len(), 1);
        assert!(matches!(responses[0], ServerMessage::Notice { .. }));
        assert!(!session.registry().contains(&crate::SubscriptionId::new("sub")));
    }

    #[tokio::test]
    async fn count_rejects_when_limit_exceeds_policy() {
        let store = MemoryStore::new();
        let policy = Policy {
            max_limit: Some(1),
            ..Policy::default()
        };
        let mut session = Session::with_policy(store, policy);

        let responses = session
            .handle_message(ClientMessage::Count {
                subscription_id: "sub".to_string(),
                filters: vec![json!({"limit": 5})],
            })
            .await;

        assert_eq!(responses.len(), 1);
        assert!(matches!(responses[0], ServerMessage::Notice { .. }));
    }

    #[tokio::test]
    async fn req_rate_limiter_rejects_excess_requests() {
        let store = MemoryStore::new();
        let policy = Policy {
            max_requests_per_min: Some(1),
            ..Policy::default()
        };
        let mut session = Session::with_policy(store, policy);
        let _ = session
            .handle_message(ClientMessage::Req {
                subscription_id: "sub-1".to_string(),
                filters: vec![json!({})],
            })
            .await;

        let responses = session
            .handle_message(ClientMessage::Req {
                subscription_id: "sub-2".to_string(),
                filters: vec![json!({})],
            })
            .await;

        assert_eq!(responses.len(), 1);
        assert!(matches!(responses[0], ServerMessage::Notice { .. }));
    }

    #[tokio::test]
    async fn event_rate_limiter_rejects_excess_events() {
        let store = MemoryStore::new();
        let policy = Policy {
            max_events_per_min: Some(1),
            ..Policy::default()
        };
        let mut session = Session::with_policy(store, policy);

        let first = serde_json::to_value(signed_event("rate-1")).expect("event");
        let second = serde_json::to_value(signed_event("rate-2")).expect("event");

        let _ = session.handle_message(ClientMessage::Event(first)).await;
        let responses = session.handle_message(ClientMessage::Event(second)).await;

        assert_eq!(responses.len(), 1);
        assert!(matches!(responses[0], ServerMessage::Notice { .. }));
    }

    #[tokio::test]
    async fn retention_prunes_events_older_than_cutoff() {
        let store = MemoryStore::new();
        let old = NostrEvent {
            id: "old".to_string(),
            pubkey: "aa".repeat(32),
            created_at: 10,
            kind: 1,
            tags: Vec::new(),
            content: String::new(),
            sig: "00".repeat(64),
        };
        let new = NostrEvent {
            id: "new".to_string(),
            pubkey: "aa".repeat(32),
            created_at: 100,
            kind: 1,
            tags: Vec::new(),
            content: String::new(),
            sig: "00".repeat(64),
        };
        store.insert(old).await.expect("insert");
        store.insert(new).await.expect("insert");

        let policy = Policy {
            retention_max_age_seconds: Some(50),
            ..Policy::default()
        };
        let mut session = Session::with_policy(store, policy);
        session.apply_retention(100).await.expect("retention");

        let responses = session
            .handle_message(ClientMessage::Req {
                subscription_id: "sub".to_string(),
                filters: vec![json!({})],
            })
            .await;
        let ids: Vec<String> = responses
            .into_iter()
            .filter_map(|message| match message {
                ServerMessage::Event { event, .. } => event
                    .get("id")
                    .and_then(|value| value.as_str())
                    .map(|value| value.to_string()),
                _ => None,
            })
            .collect();
        assert_eq!(ids, vec!["new".to_string()]);
    }

    #[test]
    fn auth_challenge_emitted_when_required() {
        let session = Session::with_policy_and_auth(MemoryStore::new(), Policy::default(), true);
        let challenge = session.auth_challenge().expect("challenge");
        assert!(!challenge.is_empty());
        let messages = session.initial_messages();
        assert!(matches!(messages[0], ServerMessage::Auth { .. }));
    }

    struct StubAdmission {
        decision: AdmissionDecision,
    }

    #[async_trait]
    impl AdmissionDecider for StubAdmission {
        async fn decide(&self, _event: &crate::RelayEvent) -> AdmissionDecision {
            self.decision.clone()
        }
    }

    fn signed_event(seed: &str) -> NostrEvent {
        signed_event_with_tags(seed, 1, Vec::new())
    }

    fn signed_event_with_tags(seed: &str, kind: u32, tags: Vec<Vec<String>>) -> NostrEvent {
        let secp = Secp256k1::new();
        let secret_key = SecretKey::from_slice(&[0x11; 32]).expect("secret");
        let keypair = Keypair::from_secret_key(&secp, &secret_key);
        let (pubkey, _) = secp256k1::XOnlyPublicKey::from_keypair(&keypair);

        let mut event = NostrEvent {
            id: seed.to_string(),
            pubkey: hex::encode(pubkey.serialize()),
            created_at: 1,
            kind,
            tags,
            content: String::new(),
            sig: String::new(),
        };
        event.id = event.compute_id().expect("id");
        let id_bytes = hex::decode(&event.id).expect("id bytes");
        let msg = secp256k1::Message::from_digest_slice(&id_bytes).expect("msg");
        let sig = secp.sign_schnorr(&msg, &keypair);
        event.sig = hex::encode(sig.as_ref());
        event
    }

    #[tokio::test]
    async fn auth_accepts_valid_event() {
        let mut session = Session::with_policy_and_auth(MemoryStore::new(), Policy::default(), true);
        let challenge = session.auth_challenge().expect("challenge");
        let event = signed_event_with_tags(
            "auth",
            super::AUTH_KIND,
            vec![vec!["challenge".to_string(), challenge]],
        );

        let response = session
            .handle_message(ClientMessage::Auth(serde_json::to_value(event).unwrap()))
            .await;
        assert!(matches!(
            response[0],
            ServerMessage::Ok { accepted: true, .. }
        ));
    }

    #[tokio::test]
    async fn auth_required_rejects_event_without_auth() {
        let mut session = Session::with_policy_and_auth(MemoryStore::new(), Policy::default(), true);
        let event = signed_event("seed");
        let response = session
            .handle_message(ClientMessage::Event(serde_json::to_value(event).unwrap()))
            .await;
        assert!(matches!(
            response[0],
            ServerMessage::Ok { accepted: false, .. }
        ));
    }

    #[tokio::test]
    async fn admission_rejects_event() {
        let admission = StubAdmission {
            decision: AdmissionDecision::Reject {
                reason: "nope".to_string(),
            },
        };
        let mut session = Session::with_admission(MemoryStore::new(), Arc::new(admission));
        let event = signed_event("seed");

        let response = session
            .handle_message(ClientMessage::Event(serde_json::to_value(event).unwrap()))
            .await;
        assert_eq!(response.len(), 1);
        assert!(matches!(
            response[0],
            ServerMessage::Ok { accepted: false, .. }
        ));
    }

    #[tokio::test]
    async fn admission_requires_related_events() {
        let related = signed_event("rel");
        let store = MemoryStore::new();
        store.insert(related.clone()).await.expect("insert");

        let mut filter = EventFilter::new();
        filter.ids = vec![related.id.clone()];
        filter.limit = Some(1);

        let admission = StubAdmission {
            decision: AdmissionDecision::RequiresRelatedEvents { filters: vec![filter] },
        };

        let mut session = Session::with_admission(store, Arc::new(admission));
        let event = signed_event("seed");
        let response = session
            .handle_message(ClientMessage::Event(serde_json::to_value(event).unwrap()))
            .await;
        assert_eq!(response.len(), 1);
        assert!(matches!(
            response[0],
            ServerMessage::Ok { accepted: true, .. }
        ));
    }

    #[tokio::test]
    async fn dispatch_event_matches_subscriptions() {
        let store = MemoryStore::new();
        let mut session = Session::new(store);
        session
            .handle_message(ClientMessage::Req {
                subscription_id: "sub".to_string(),
                filters: vec![json!({})],
            })
            .await;

        let event = signed_event("seed");
        let responses = session.dispatch_event(&event);
        assert_eq!(responses.len(), 1);
        assert!(matches!(responses[0], ServerMessage::Event { .. }));
    }

    #[tokio::test]
    async fn broadcast_sends_inserted_events() {
        let store = MemoryStore::new();
        let (tx, mut rx) = tokio::sync::broadcast::channel(8);
        let mut session =
            Session::with_broadcast(store, Policy::default(), None, tx, false);
        let event = signed_event("seed");

        let response = session
            .handle_message(ClientMessage::Event(serde_json::to_value(event.clone()).unwrap()))
            .await;
        assert!(matches!(
            response[0],
            ServerMessage::Ok { accepted: true, .. }
        ));

        let received = rx.recv().await.expect("broadcast");
        assert_eq!(received.id, event.id);
    }
}
