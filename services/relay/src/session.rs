use crate::{
    AdmissionDecider, ClientMessage, EventStore, Filter, Notice, Policy, RelayEvent, RelayMetrics,
    ServerMessage, StoreError, StoreOutcome, SubscriptionId, SubscriptionRegistry,
    decode_client_message,
};
use gittree_core::AdmissionDecision;
use gittree_storage::{RelayInviteRecord, RelayMembershipRecord, RelayMembershipRepository};
use secp256k1::rand::{RngCore, rngs::OsRng};
use secp256k1::{Keypair, Message, Secp256k1, SecretKey};
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::broadcast;

const AUTH_KIND: u32 = 22242;
const AUTH_REQUIRED_REASON: &str = "auth-required";
const AUTH_PUBKEY_MISMATCH_REASON: &str = "auth-pubkey-mismatch";
const AUTH_MAX_SKEW_SECS: i64 = 600;
const NIP43_MEMBERSHIP_KIND: u32 = 13534;
const NIP43_JOIN_KIND: u32 = 28934;
const NIP43_INVITE_KIND: u32 = 28935;
const NIP43_LEAVE_KIND: u32 = 28936;
const MEMBERSHIP_STATUS_ACTIVE: &str = "active";
const MEMBERSHIP_STATUS_LEFT: &str = "left";
const DEFAULT_INVITE_ROLE: &str = "member";
const INVITE_TTL_SECS: i64 = 86_400;
const RESTRICTED_PREFIX: &str = "restricted:";
const DUPLICATE_PREFIX: &str = "duplicate:";
const INFO_PREFIX: &str = "info:";

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

struct TenantSigner {
    pubkey: String,
    secret_key: SecretKey,
}

impl TenantSigner {
    fn new(relay_pubkey: Vec<u8>, relay_secret: Vec<u8>) -> Option<Self> {
        if relay_pubkey.len() != 32 {
            return None;
        }
        let secret_key = SecretKey::from_slice(&relay_secret).ok()?;
        Some(Self {
            pubkey: hex::encode(relay_pubkey),
            secret_key,
        })
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

    fn hit_limit(counter: &mut RateLimitCounter, limit: Option<u64>, window: Duration) -> bool {
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
    tenant_id: Option<String>,
    membership: Option<Arc<dyn RelayMembershipRepository>>,
    tenant_signer: Option<TenantSigner>,
    relay_url: Option<String>,
    read_auth_required: bool,
    write_auth_required: bool,
    read_membership_required: bool,
    write_membership_required: bool,
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
            tenant_id: None,
            membership: None,
            tenant_signer: None,
            relay_url: None,
            read_auth_required: false,
            write_auth_required: false,
            read_membership_required: false,
            write_membership_required: false,
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
            tenant_id: None,
            membership: None,
            tenant_signer: None,
            relay_url: None,
            read_auth_required: false,
            write_auth_required: false,
            read_membership_required: false,
            write_membership_required: false,
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
            tenant_id: None,
            membership: None,
            tenant_signer: None,
            relay_url: None,
            read_auth_required: false,
            write_auth_required: false,
            read_membership_required: false,
            write_membership_required: false,
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
            tenant_id: None,
            membership: None,
            tenant_signer: None,
            relay_url: None,
            read_auth_required: false,
            write_auth_required: false,
            read_membership_required: false,
            write_membership_required: false,
            rate_limiter: RateLimitConfig::from_policy(&policy).map(RateLimiter::new),
            metrics: None,
        }
    }

    pub fn with_policy_and_auth(store: S, policy: Policy, auth_required: bool) -> Self {
        let auth_enabled = auth_required;
        Self {
            registry: SubscriptionRegistry::default(),
            store,
            policy,
            admission: None,
            broadcast: None,
            auth: auth_state(auth_enabled),
            tenant_id: None,
            membership: None,
            tenant_signer: None,
            relay_url: None,
            read_auth_required: auth_required,
            write_auth_required: auth_required,
            read_membership_required: false,
            write_membership_required: false,
            rate_limiter: RateLimitConfig::from_policy(&policy).map(RateLimiter::new),
            metrics: None,
        }
    }

    pub fn with_broadcast(
        store: S,
        policy: Policy,
        admission: Option<Arc<dyn AdmissionDecider>>,
        broadcast: broadcast::Sender<crate::NostrEvent>,
        read_auth_required: bool,
        write_auth_required: bool,
    ) -> Self {
        let auth_enabled = read_auth_required || write_auth_required;
        Self {
            registry: SubscriptionRegistry::default(),
            store,
            policy,
            admission,
            broadcast: Some(broadcast),
            auth: auth_state(auth_enabled),
            tenant_id: None,
            membership: None,
            tenant_signer: None,
            relay_url: None,
            read_auth_required,
            write_auth_required,
            read_membership_required: false,
            write_membership_required: false,
            rate_limiter: RateLimitConfig::from_policy(&policy).map(RateLimiter::new),
            metrics: None,
        }
    }

    pub fn with_membership(
        mut self,
        tenant_id: Option<String>,
        membership: Option<Arc<dyn RelayMembershipRepository>>,
    ) -> Self {
        self.tenant_id = tenant_id;
        self.membership = membership;
        self
    }

    pub fn with_membership_requirements(
        mut self,
        read_required: bool,
        write_required: bool,
    ) -> Self {
        self.read_membership_required = read_required;
        self.write_membership_required = write_required;
        self
    }

    pub fn with_relay_signer(mut self, relay_pubkey: Vec<u8>, relay_secret: Vec<u8>) -> Self {
        self.tenant_signer = TenantSigner::new(relay_pubkey, relay_secret);
        self
    }

    pub fn with_relay_url(mut self, relay_url: Option<String>) -> Self {
        self.relay_url = relay_url;
        self
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

    fn is_authenticated(&self) -> bool {
        match self.auth.as_ref() {
            Some(state) => state.authenticated_pubkey.is_some(),
            None => false,
        }
    }

    fn authenticated_pubkey(&self) -> Option<&str> {
        match self.auth.as_ref() {
            Some(state) => state.authenticated_pubkey.as_deref(),
            None => None,
        }
    }

    async fn ensure_read_membership(&self) -> Result<(), String> {
        if !self.read_membership_required {
            return Ok(());
        }
        self.require_membership().await
    }

    async fn ensure_write_membership(&self) -> Result<(), String> {
        if !self.write_membership_required {
            return Ok(());
        }
        self.require_membership().await
    }

    async fn require_membership(&self) -> Result<(), String> {
        let Some(membership) = self.membership.as_ref() else {
            return Err(format!(
                "{RESTRICTED_PREFIX} relay does not support membership"
            ));
        };
        let Some(tenant_id) = self.tenant_id.as_deref() else {
            return Err(format!(
                "{RESTRICTED_PREFIX} relay does not support membership"
            ));
        };
        let Some(pubkey) = self.authenticated_pubkey() else {
            return Err(AUTH_REQUIRED_REASON.to_string());
        };

        let pubkey_bytes = decode_pubkey_hex(pubkey)?;
        let member =
            membership_by_pubkey_bytes(membership.as_ref(), tenant_id, &pubkey_bytes).await?;
        let Some(member) = member else {
            return Err(format!("{RESTRICTED_PREFIX} membership required"));
        };
        if member.status != MEMBERSHIP_STATUS_ACTIVE {
            return Err(format!("{RESTRICTED_PREFIX} membership required"));
        }
        Ok(())
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
        let event_value = serde_json::to_value(event).unwrap_or(Value::Null);

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

    async fn virtual_events(
        &self,
        filters: &[Filter],
        now: i64,
    ) -> Result<Vec<crate::NostrEvent>, String> {
        let mut events = Vec::new();

        if filters_request_kind(filters, NIP43_MEMBERSHIP_KIND) {
            if let Some(event) = self.build_membership_list_event(now).await? {
                if event_matches_filters(&event, filters) {
                    events.push(event);
                }
            }
        }

        if filters_request_kind(filters, NIP43_INVITE_KIND) {
            let event = self.build_invite_event(now).await?;
            if event_matches_filters(&event, filters) {
                events.push(event);
            }
        }

        Ok(events)
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
                if self.read_auth_required && !self.is_authenticated() {
                    return vec![ServerMessage::Closed {
                        subscription_id,
                        message: AUTH_REQUIRED_REASON.to_string(),
                    }];
                }
                if let Err(message) = self.ensure_read_membership().await {
                    return vec![ServerMessage::Closed {
                        subscription_id,
                        message,
                    }];
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

                let mut events = match self.store.query(&parsed).await {
                    Ok(events) => events,
                    Err(err) => return vec![Notice::from(err).into()],
                };
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs() as i64;
                let virtual_events = match self.virtual_events(&parsed, now).await {
                    Ok(events) => events,
                    Err(message) => {
                        return vec![ServerMessage::Closed {
                            subscription_id,
                            message,
                        }];
                    }
                };
                events.extend(virtual_events);

                self.registry.insert(sub_id.clone(), parsed.clone());

                let mut responses = Vec::new();
                for event in events {
                    let value = serde_json::to_value(event).unwrap_or(Value::Null);
                    responses.push(ServerMessage::Event {
                        subscription_id: subscription_id.clone(),
                        event: value,
                    });
                }
                self.registry.mark_eose(&sub_id);
                responses.push(ServerMessage::Eose { subscription_id });
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
                if self.read_auth_required && !self.is_authenticated() {
                    return vec![ServerMessage::Closed {
                        subscription_id,
                        message: AUTH_REQUIRED_REASON.to_string(),
                    }];
                }
                if let Err(message) = self.ensure_read_membership().await {
                    return vec![ServerMessage::Closed {
                        subscription_id,
                        message,
                    }];
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
        let exceeded = filters
            .iter()
            .any(|filter| filter.limit.is_some_and(|limit| limit > max_limit));
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

        if event.kind == AUTH_KIND {
            self.record_event("rejected");
            return vec![ServerMessage::Ok {
                event_id: event.id.clone(),
                accepted: false,
                message: "auth event must use AUTH".to_string(),
            }];
        }

        if let Some(responses) = self.handle_membership_event(&event, now).await {
            return responses;
        }

        if self.write_auth_required {
            let Some(auth) = &self.auth else {
                self.record_event("rejected");
                return vec![ServerMessage::Ok {
                    event_id: event.id.clone(),
                    accepted: false,
                    message: AUTH_REQUIRED_REASON.to_string(),
                }];
            };
            match auth.authenticated_pubkey.as_ref() {
                None => {
                    self.record_event("rejected");
                    return vec![ServerMessage::Ok {
                        event_id: event.id.clone(),
                        accepted: false,
                        message: AUTH_REQUIRED_REASON.to_string(),
                    }];
                }
                Some(pubkey) if pubkey != &event.pubkey => {
                    self.record_event("rejected");
                    return vec![ServerMessage::Ok {
                        event_id: event.id.clone(),
                        accepted: false,
                        message: AUTH_PUBKEY_MISMATCH_REASON.to_string(),
                    }];
                }
                Some(_) => {}
            }
        }

        if let Err(message) = self.ensure_write_membership().await {
            self.record_event("rejected");
            return vec![ServerMessage::Ok {
                event_id: event.id.clone(),
                accepted: false,
                message,
            }];
        }

        if let Some(admission) = &self.admission {
            let relay_event = RelayEvent {
                kind: event.kind as u64,
                pubkey: event.pubkey.clone(),
                event_id: event.id.clone(),
                tags: event.tags.clone(),
                relay_url: self.relay_url.clone(),
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

    async fn handle_membership_event(
        &mut self,
        event: &crate::NostrEvent,
        now: i64,
    ) -> Option<Vec<ServerMessage>> {
        if event.kind != NIP43_JOIN_KIND && event.kind != NIP43_LEAVE_KIND {
            return None;
        }

        let Some(membership) = self.membership.as_ref() else {
            self.record_event("rejected");
            return Some(membership_response(
                &event.id,
                false,
                format!("{RESTRICTED_PREFIX} relay does not support membership"),
            ));
        };
        let Some(tenant_id) = self.tenant_id.as_deref() else {
            self.record_event("rejected");
            return Some(membership_response(
                &event.id,
                false,
                format!("{RESTRICTED_PREFIX} relay does not support membership"),
            ));
        };
        if !has_tag(&event.tags, "-") {
            self.record_event("rejected");
            return Some(membership_response(
                &event.id,
                false,
                format!("{RESTRICTED_PREFIX} missing nip-70 tag"),
            ));
        }

        let pubkey_bytes = match hex::decode(&event.pubkey) {
            Ok(bytes) => bytes,
            Err(_) => {
                self.record_event("rejected");
                return Some(membership_response(
                    &event.id,
                    false,
                    format!("{RESTRICTED_PREFIX} invalid pubkey"),
                ));
            }
        };

        if event.kind == NIP43_JOIN_KIND {
            let Some(invite_code) = find_tag_value(&event.tags, "claim") else {
                self.record_event("rejected");
                return Some(membership_response(
                    &event.id,
                    false,
                    format!("{RESTRICTED_PREFIX} missing invite code"),
                ));
            };

            let invite = match membership.invite_by_code(tenant_id, &invite_code).await {
                Ok(invite) => invite,
                Err(err) => {
                    self.record_event("rejected");
                    return Some(vec![Notice::message(err.to_string()).into()]);
                }
            };
            let Some(invite) = invite else {
                self.record_event("rejected");
                return Some(membership_response(
                    &event.id,
                    false,
                    format!("{RESTRICTED_PREFIX} that is an invalid invite code."),
                ));
            };

            if let Some(expires_at) = invite.expires_at {
                if expires_at < now {
                    self.record_event("rejected");
                    return Some(membership_response(
                        &event.id,
                        false,
                        format!("{RESTRICTED_PREFIX} that invite code is expired."),
                    ));
                }
            }

            if let Some(invitee) = invite.invitee_pubkey.as_ref() {
                if invitee != &pubkey_bytes {
                    self.record_event("rejected");
                    return Some(membership_response(
                        &event.id,
                        false,
                        format!("{RESTRICTED_PREFIX} invite code does not match pubkey."),
                    ));
                }
            }

            let existing =
                match membership_by_pubkey_bytes(membership.as_ref(), tenant_id, &pubkey_bytes)
                    .await
                {
                    Ok(record) => record,
                    Err(err) => {
                        self.record_event("rejected");
                        return Some(vec![Notice::message(err).into()]);
                    }
                };

            let already_active = existing
                .as_ref()
                .is_some_and(|record| record.status == MEMBERSHIP_STATUS_ACTIVE);
            if already_active {
                self.record_event("duplicate");
                return Some(membership_response(
                    &event.id,
                    true,
                    format!("{DUPLICATE_PREFIX} you are already a member of this relay."),
                ));
            }

            let created_at = existing
                .as_ref()
                .map(|record| record.created_at)
                .unwrap_or(now);
            let record = RelayMembershipRecord {
                tenant_id: tenant_id.to_string(),
                pubkey: pubkey_bytes,
                role: invite.role.clone(),
                status: MEMBERSHIP_STATUS_ACTIVE.to_string(),
                created_at,
                updated_at: now,
            };
            if let Err(err) = membership.upsert_membership(record).await {
                self.record_event("rejected");
                return Some(vec![Notice::message(err.to_string()).into()]);
            }
            if let Err(err) = membership.delete_invite(tenant_id, &invite_code).await {
                self.record_event("rejected");
                return Some(vec![Notice::message(err.to_string()).into()]);
            }
            self.record_event("accepted");
            Some(membership_response(
                &event.id,
                true,
                format!("{INFO_PREFIX} welcome to the relay."),
            ))
        } else {
            let existing =
                match membership_by_pubkey_bytes(membership.as_ref(), tenant_id, &pubkey_bytes)
                    .await
                {
                    Ok(record) => record,
                    Err(err) => {
                        self.record_event("rejected");
                        return Some(vec![Notice::message(err).into()]);
                    }
                };

            let Some(record) = existing else {
                self.record_event("rejected");
                return Some(membership_response(
                    &event.id,
                    false,
                    format!("{RESTRICTED_PREFIX} not a member of this relay."),
                ));
            };

            if record.status != MEMBERSHIP_STATUS_ACTIVE {
                self.record_event("rejected");
                return Some(membership_response(
                    &event.id,
                    false,
                    format!("{RESTRICTED_PREFIX} not a member of this relay."),
                ));
            }

            let record = RelayMembershipRecord {
                tenant_id: record.tenant_id,
                pubkey: record.pubkey,
                role: record.role,
                status: MEMBERSHIP_STATUS_LEFT.to_string(),
                created_at: record.created_at,
                updated_at: now,
            };
            if let Err(err) = membership.upsert_membership(record).await {
                self.record_event("rejected");
                return Some(vec![Notice::message(err.to_string()).into()]);
            }
            self.record_event("accepted");
            Some(membership_response(
                &event.id,
                true,
                format!("{INFO_PREFIX} access revoked."),
            ))
        }
    }

    async fn build_membership_list_event(
        &self,
        now: i64,
    ) -> Result<Option<crate::NostrEvent>, String> {
        let Some(membership) = self.membership.as_ref() else {
            return Ok(None);
        };
        let Some(tenant_id) = self.tenant_id.as_deref() else {
            return Ok(None);
        };
        let Some(signer) = self.tenant_signer.as_ref() else {
            return Ok(None);
        };

        let records = match membership.list_memberships(tenant_id).await {
            Ok(records) => records,
            Err(err) => return Err(err.to_string()),
        };
        let mut tags = Vec::with_capacity(records.len() + 1);
        tags.push(vec!["-".to_string()]);
        for record in records {
            if record.status == MEMBERSHIP_STATUS_ACTIVE {
                tags.push(vec!["member".to_string(), hex::encode(record.pubkey)]);
            }
        }

        let mut event = crate::NostrEvent {
            id: String::new(),
            pubkey: signer.pubkey.clone(),
            created_at: now,
            kind: NIP43_MEMBERSHIP_KIND,
            tags,
            content: String::new(),
            sig: String::new(),
        };
        sign_event(&mut event, signer);
        Ok(Some(event))
    }

    async fn build_invite_event(&self, now: i64) -> Result<crate::NostrEvent, String> {
        let Some(membership) = self.membership.as_ref() else {
            return Err(format!(
                "{RESTRICTED_PREFIX} relay does not support invites"
            ));
        };
        let Some(tenant_id) = self.tenant_id.as_deref() else {
            return Err(format!(
                "{RESTRICTED_PREFIX} relay does not support invites"
            ));
        };
        let Some(signer) = self.tenant_signer.as_ref() else {
            return Err(format!(
                "{RESTRICTED_PREFIX} relay does not support invites"
            ));
        };
        let Some(pubkey) = self.authenticated_pubkey() else {
            return Err(AUTH_REQUIRED_REASON.to_string());
        };

        let pubkey_bytes = decode_pubkey_hex(pubkey)?;
        let member =
            membership_by_pubkey_bytes(membership.as_ref(), tenant_id, &pubkey_bytes).await?;
        let Some(member) = member else {
            return Err(format!("{RESTRICTED_PREFIX} membership required"));
        };
        if member.status != MEMBERSHIP_STATUS_ACTIVE {
            return Err(format!("{RESTRICTED_PREFIX} membership required"));
        }

        let invite_code = generate_invite_code();
        let invite = RelayInviteRecord {
            tenant_id: tenant_id.to_string(),
            invite_code: invite_code.clone(),
            role: DEFAULT_INVITE_ROLE.to_string(),
            inviter_pubkey: pubkey_bytes,
            invitee_pubkey: None,
            expires_at: Some(now.saturating_add(INVITE_TTL_SECS)),
            created_at: now,
        };
        match membership.insert_invite(invite).await {
            Ok(()) => {}
            Err(err) => return Err(err.to_string()),
        }

        let mut event = crate::NostrEvent {
            id: String::new(),
            pubkey: signer.pubkey.clone(),
            created_at: now,
            kind: NIP43_INVITE_KIND,
            tags: vec![
                vec!["-".to_string()],
                vec!["claim".to_string(), invite_code],
            ],
            content: String::new(),
            sig: String::new(),
        };
        sign_event(&mut event, signer);
        Ok(event)
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

        let Some(relay_tag) = find_tag_value(&event.tags, "relay") else {
            return vec![ServerMessage::Ok {
                event_id: event.id.clone(),
                accepted: false,
                message: "missing relay tag".to_string(),
            }];
        };

        if let Some(expected) = self.relay_url.as_deref().and_then(relay_host_from_url) {
            let Some(actual) = relay_host_from_url(&relay_tag) else {
                return vec![ServerMessage::Ok {
                    event_id: event.id.clone(),
                    accepted: false,
                    message: "invalid relay tag".to_string(),
                }];
            };
            if actual != expected {
                return vec![ServerMessage::Ok {
                    event_id: event.id.clone(),
                    accepted: false,
                    message: "relay tag mismatch".to_string(),
                }];
            }
        }

        let now = unix_now_secs();
        let skew = (now - event.created_at).abs();
        if skew > AUTH_MAX_SKEW_SECS {
            return vec![ServerMessage::Ok {
                event_id: event.id.clone(),
                accepted: false,
                message: "auth event timestamp out of range".to_string(),
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

fn decode_pubkey_hex(pubkey: &str) -> Result<Vec<u8>, String> {
    hex::decode(pubkey).map_err(|_| format!("{RESTRICTED_PREFIX} invalid pubkey"))
}

async fn membership_by_pubkey_bytes(
    membership: &(dyn RelayMembershipRepository + Send + Sync),
    tenant_id: &str,
    pubkey: &[u8],
) -> Result<Option<RelayMembershipRecord>, String> {
    membership
        .membership_by_pubkey(tenant_id, pubkey)
        .await
        .map_err(|err| err.to_string())
}

fn unix_now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn find_tag_value(tags: &[Vec<String>], name: &str) -> Option<String> {
    tags.iter()
        .find(|tag| tag.first().map(|entry| entry == name).unwrap_or(false))
        .and_then(|tag| tag.get(1).cloned())
}

fn has_tag(tags: &[Vec<String>], name: &str) -> bool {
    tags.iter()
        .any(|tag| tag.first().map(|entry| entry == name).unwrap_or(false))
}

fn relay_host_from_url(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    let without_scheme = value
        .strip_prefix("ws://")
        .or_else(|| value.strip_prefix("wss://"))
        .or_else(|| value.strip_prefix("http://"))
        .or_else(|| value.strip_prefix("https://"))
        .unwrap_or(value);
    let host = without_scheme.split('/').next().unwrap_or("");
    if host.is_empty() {
        return None;
    }
    let host = host.trim_end_matches('.');
    if let Some(inner) = host.strip_prefix('[') {
        if let Some(end) = inner.find(']') {
            return Some(inner[..end].to_ascii_lowercase());
        }
    }
    let host = host.split(':').next().unwrap_or(host);
    Some(host.to_ascii_lowercase())
}

fn filters_request_kind(filters: &[Filter], kind: u32) -> bool {
    filters.iter().any(|filter| filter.kinds.contains(&kind))
}

fn event_matches_filters(event: &crate::NostrEvent, filters: &[Filter]) -> bool {
    if filters.is_empty() {
        return true;
    }
    let tags = match crate::TagIndex::from_tags(&event.tags) {
        Ok(tags) => tags,
        Err(_) => return false,
    };
    filters.iter().any(|filter| filter.matches(event, &tags))
}

fn membership_response(event_id: &str, accepted: bool, message: String) -> Vec<ServerMessage> {
    vec![ServerMessage::Ok {
        event_id: event_id.to_string(),
        accepted,
        message,
    }]
}

fn sign_event(event: &mut crate::NostrEvent, signer: &TenantSigner) {
    event.id = event.compute_id();
    let id_bytes = hex::decode(&event.id).expect("event id is always canonical hex");
    let msg = Message::from_digest_slice(&id_bytes).expect("canonical event id is 32-byte digest");
    let secp = Secp256k1::new();
    let keypair = Keypair::from_secret_key(&secp, &signer.secret_key);
    let sig = secp.sign_schnorr(&msg, &keypair);
    event.sig = hex::encode(sig.as_ref());
}

fn generate_invite_code() -> String {
    let mut bytes = [0u8; 16];
    OsRng.fill_bytes(&mut bytes);
    hex::encode(bytes)
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
        ServerMessage, StoreError, StoreOutcome,
    };
    use async_trait::async_trait;
    use gittree_core::{AdmissionDecision, EventFilter};
    use gittree_storage::{
        InMemoryRepositories, RelayInviteRecord, RelayMembershipRecord, RelayMembershipRepository,
        StorageError,
    };
    use secp256k1::{Keypair, Secp256k1, SecretKey};
    use serde_json::json;
    use std::sync::Arc;
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    fn assert_notice(message: &ServerMessage) {
        assert!(matches!(message, ServerMessage::Notice { .. }));
    }

    fn assert_event(message: &ServerMessage) {
        assert!(matches!(message, ServerMessage::Event { .. }));
    }

    fn assert_eose(message: &ServerMessage) {
        assert!(matches!(message, ServerMessage::Eose { .. }));
    }

    fn assert_auth(message: &ServerMessage) {
        assert!(matches!(message, ServerMessage::Auth { .. }));
    }

    fn assert_count(message: &ServerMessage, expected: u64) {
        assert!(matches!(
            message,
            ServerMessage::Count { count, .. } if *count == expected
        ));
    }

    fn assert_ok_rejected(message: &ServerMessage) {
        assert!(matches!(
            message,
            ServerMessage::Ok {
                accepted: false,
                ..
            }
        ));
    }

    fn assert_ok_rejected_with_non_empty_message(message: &ServerMessage) {
        assert!(matches!(
            message,
            ServerMessage::Ok {
                accepted: false,
                message,
                ..
            } if !message.is_empty()
        ));
    }

    fn closed_reason(message: &ServerMessage) -> &str {
        match message {
            ServerMessage::Closed { message, .. } => message.as_str(),
            other => panic!("expected closed message, got {other:?}"),
        }
    }

    #[test]
    fn assertion_helpers_accept_expected_variants() {
        assert_notice(&ServerMessage::Notice {
            message: "notice".to_string(),
        });
        assert_event(&ServerMessage::Event {
            subscription_id: "sub".to_string(),
            event: json!({}),
        });
        assert_eose(&ServerMessage::Eose {
            subscription_id: "sub".to_string(),
        });
        assert_auth(&ServerMessage::Auth {
            challenge: "challenge".to_string(),
        });
        assert_count(
            &ServerMessage::Count {
                subscription_id: "sub".to_string(),
                count: 2,
            },
            2,
        );
        assert_eq!(
            closed_reason(&ServerMessage::Closed {
                subscription_id: "sub".to_string(),
                message: "closed".to_string(),
            }),
            "closed"
        );
    }

    #[test]
    #[should_panic]
    fn assertion_helper_panics_on_wrong_variant() {
        assert_notice(&ServerMessage::Eose {
            subscription_id: "sub".to_string(),
        });
    }

    #[test]
    #[should_panic]
    fn assertion_helper_panics_on_wrong_event_variant() {
        assert_event(&ServerMessage::Notice {
            message: "notice".to_string(),
        });
    }

    #[test]
    #[should_panic]
    fn assertion_helper_panics_on_wrong_eose_variant() {
        assert_eose(&ServerMessage::Notice {
            message: "notice".to_string(),
        });
    }

    #[test]
    #[should_panic]
    fn assertion_helper_panics_on_wrong_auth_variant() {
        assert_auth(&ServerMessage::Notice {
            message: "notice".to_string(),
        });
    }

    #[test]
    #[should_panic]
    fn assertion_helper_panics_on_wrong_count_variant() {
        assert_count(
            &ServerMessage::Notice {
                message: "notice".to_string(),
            },
            1,
        );
    }

    #[test]
    #[should_panic]
    fn assertion_helper_panics_on_count_mismatch() {
        assert_count(
            &ServerMessage::Count {
                subscription_id: "sub".to_string(),
                count: 1,
            },
            2,
        );
    }

    #[test]
    #[should_panic]
    fn assertion_helper_panics_on_non_rejected_ok() {
        assert_ok_rejected(&ServerMessage::Ok {
            event_id: "id".to_string(),
            accepted: true,
            message: "saved".to_string(),
        });
    }

    #[test]
    #[should_panic]
    fn assertion_helper_panics_on_empty_rejected_message() {
        assert_ok_rejected_with_non_empty_message(&ServerMessage::Ok {
            event_id: "id".to_string(),
            accepted: false,
            message: String::new(),
        });
    }

    #[test]
    #[should_panic]
    fn assertion_helper_panics_on_non_rejected_non_empty_ok() {
        assert_ok_rejected_with_non_empty_message(&ServerMessage::Ok {
            event_id: "id".to_string(),
            accepted: true,
            message: "saved".to_string(),
        });
    }

    #[test]
    #[should_panic]
    fn assertion_helper_panics_on_non_ok_variant_for_rejected_non_empty() {
        assert_ok_rejected_with_non_empty_message(&ServerMessage::Notice {
            message: "notice".to_string(),
        });
    }

    #[test]
    #[should_panic]
    fn closed_reason_panics_on_non_closed_variant() {
        let _ = closed_reason(&ServerMessage::Notice {
            message: "notice".to_string(),
        });
    }

    #[tokio::test]
    async fn handle_raw_reports_invalid_messages() {
        let mut session = Session::new(MemoryStore::new());
        let responses = session.handle_raw("{\"bad\":true}").await;
        assert_eq!(responses.len(), 1);
        assert_notice(&responses[0]);
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
        assert!(
            session
                .registry()
                .contains(&crate::SubscriptionId::new("sub"))
        );

        session
            .handle_message(ClientMessage::Close {
                subscription_id: "sub".to_string(),
            })
            .await;
        assert!(
            !session
                .registry()
                .contains(&crate::SubscriptionId::new("sub"))
        );
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
        assert_event(&responses[0]);
        assert_eose(&responses[1]);
        assert!(
            session
                .registry()
                .eose_sent(&crate::SubscriptionId::new("sub"))
        );
    }

    #[tokio::test]
    async fn req_rejects_invalid_filters_and_query_errors() {
        let mut parse_session = Session::new(MemoryStore::new());
        let parse_response = parse_session
            .handle_message(ClientMessage::Req {
                subscription_id: "sub".to_string(),
                filters: vec![json!({"ids": "not-an-array"})],
            })
            .await;
        assert_notice(&parse_response[0]);

        let mut query_session = Session::new(scripted_store_dyn(ScriptedStore::query_error()));
        let query_response = query_session
            .handle_message(ClientMessage::Req {
                subscription_id: "sub".to_string(),
                filters: vec![json!({})],
            })
            .await;
        assert_notice(&query_response[0]);
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
        assert_count(&responses[0], 1);
    }

    #[tokio::test]
    async fn count_rejects_rate_limit_auth_membership_and_query_errors() {
        let mut rate_limited = Session::with_policy(
            MemoryStore::new(),
            Policy {
                max_requests_per_min: Some(1),
                ..Policy::default()
            },
        );
        let _ = rate_limited
            .handle_message(ClientMessage::Count {
                subscription_id: "sub-1".to_string(),
                filters: vec![json!({})],
            })
            .await;
        let rate_response = rate_limited
            .handle_message(ClientMessage::Count {
                subscription_id: "sub-2".to_string(),
                filters: vec![json!({})],
            })
            .await;
        assert_notice(&rate_response[0]);

        let mut auth_required = Session::with_broadcast(
            MemoryStore::new(),
            Policy::default(),
            None,
            tokio::sync::broadcast::channel(4).0,
            true,
            false,
        );
        let auth_response = auth_required
            .handle_message(ClientMessage::Count {
                subscription_id: "sub".to_string(),
                filters: vec![json!({})],
            })
            .await;
        assert_eq!(
            closed_reason(&auth_response[0]),
            super::AUTH_REQUIRED_REASON
        );

        let membership = Arc::new(InMemoryRepositories::new());
        let mut membership_required =
            Session::with_policy_and_auth(MemoryStore::new(), Policy::default(), true)
                .with_membership(Some("tenant-1".to_string()), Some(membership))
                .with_membership_requirements(true, false);
        authenticate_session(&mut membership_required).await;
        let membership_response = membership_required
            .handle_message(ClientMessage::Count {
                subscription_id: "sub".to_string(),
                filters: vec![json!({})],
            })
            .await;
        assert!(closed_reason(&membership_response[0]).starts_with(super::RESTRICTED_PREFIX));

        let mut parse_session = Session::new(MemoryStore::new());
        let parse_response = parse_session
            .handle_message(ClientMessage::Count {
                subscription_id: "sub".to_string(),
                filters: vec![json!({"ids": "not-an-array"})],
            })
            .await;
        assert_notice(&parse_response[0]);

        let mut query_session = Session::new(scripted_store_dyn(ScriptedStore::query_error()));
        let query_response = query_session
            .handle_message(ClientMessage::Count {
                subscription_id: "sub".to_string(),
                filters: vec![json!({})],
            })
            .await;
        assert_notice(&query_response[0]);
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
        assert_notice(&responses[0]);
        assert!(
            session
                .registry()
                .contains(&crate::SubscriptionId::new("sub-1"))
        );
        assert!(
            !session
                .registry()
                .contains(&crate::SubscriptionId::new("sub-2"))
        );
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
        assert_notice(&responses[0]);
        assert!(
            !session
                .registry()
                .contains(&crate::SubscriptionId::new("sub"))
        );
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
        assert_notice(&responses[0]);
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
        assert_notice(&responses[0]);
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
        assert_notice(&responses[0]);
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

    #[tokio::test]
    async fn retention_skips_when_max_age_is_non_positive() {
        let store = MemoryStore::new();
        let policy = Policy {
            retention_max_age_seconds: Some(0),
            ..Policy::default()
        };
        let session = Session::with_policy(store, policy);
        session.apply_retention(100).await.expect("retention");
    }

    #[tokio::test]
    async fn retention_surfaces_store_query_errors() {
        let policy = Policy {
            retention_max_age_seconds: Some(5),
            ..Policy::default()
        };
        let session =
            Session::with_policy(scripted_store_dyn(ScriptedStore::query_error()), policy);
        let err = session
            .apply_retention(100)
            .await
            .expect_err("query errors should fail retention");
        assert!(matches!(err, StoreError::Backend(_)));
    }

    #[tokio::test]
    async fn retention_surfaces_store_delete_errors() {
        let store = Arc::new(ScriptedStore::delete_error());
        let old = NostrEvent {
            id: "old-delete-error".to_string(),
            pubkey: "aa".repeat(32),
            created_at: 10,
            kind: 1,
            tags: Vec::new(),
            content: String::new(),
            sig: "00".repeat(64),
        };
        store.insert(old).await.expect("insert");

        let policy = Policy {
            retention_max_age_seconds: Some(5),
            ..Policy::default()
        };
        let session_store: Arc<dyn EventStore> = store;
        let session = Session::with_policy(session_store, policy);
        let err = session
            .apply_retention(100)
            .await
            .expect_err("delete errors should fail retention");
        assert!(matches!(err, StoreError::Backend(_)));
    }

    #[test]
    fn auth_challenge_emitted_when_required() {
        let session = Session::with_policy_and_auth(MemoryStore::new(), Policy::default(), true);
        let challenge = session.auth_challenge().expect("challenge");
        assert!(!challenge.is_empty());
        let messages = session.initial_messages();
        assert_auth(&messages[0]);
    }

    #[test]
    fn rate_limiter_hit_limit_handles_none_zero_and_window_rollover() {
        let mut counter = super::RateLimitCounter {
            count: 3,
            window_start: Instant::now(),
        };
        assert!(!super::RateLimiter::hit_limit(
            &mut counter,
            None,
            Duration::from_secs(1),
        ));
        assert!(!super::RateLimiter::hit_limit(
            &mut counter,
            Some(0),
            Duration::from_secs(1),
        ));

        let mut stale_counter = super::RateLimitCounter {
            count: 9,
            window_start: Instant::now() - Duration::from_millis(10),
        };
        assert!(!super::RateLimiter::hit_limit(
            &mut stale_counter,
            Some(1),
            Duration::from_millis(1),
        ));
        assert_eq!(stale_counter.count, 1);
    }

    #[test]
    fn with_policy_and_admission_starts_without_auth_messages() {
        let admission = Arc::new(StubAdmission {
            decision: AdmissionDecision::Accept,
        });
        let session =
            Session::with_policy_and_admission(MemoryStore::new(), Policy::default(), admission);
        assert!(session.auth_challenge().is_none());
        assert!(session.initial_messages().is_empty());
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

    struct ScriptedMembership {
        mode: &'static str,
        inner: InMemoryRepositories,
    }

    impl ScriptedMembership {
        fn new(mode: &'static str) -> Self {
            Self {
                mode,
                inner: InMemoryRepositories::new(),
            }
        }

        fn error(mode: &'static str) -> StorageError {
            StorageError::Internal {
                message: format!("{mode} failure"),
            }
        }
    }

    #[async_trait]
    impl RelayMembershipRepository for ScriptedMembership {
        async fn upsert_membership(
            &self,
            record: RelayMembershipRecord,
        ) -> Result<(), StorageError> {
            if self.mode == "upsert_membership" {
                return Err(Self::error(self.mode));
            }
            self.inner.upsert_membership(record).await
        }

        async fn membership_by_pubkey(
            &self,
            tenant_id: &str,
            pubkey: &[u8],
        ) -> Result<Option<RelayMembershipRecord>, StorageError> {
            if self.mode == "membership_by_pubkey" {
                return Err(Self::error(self.mode));
            }
            self.inner.membership_by_pubkey(tenant_id, pubkey).await
        }

        async fn list_memberships(
            &self,
            tenant_id: &str,
        ) -> Result<Vec<RelayMembershipRecord>, StorageError> {
            if self.mode == "list_memberships" {
                return Err(Self::error(self.mode));
            }
            self.inner.list_memberships(tenant_id).await
        }

        async fn remove_membership(
            &self,
            tenant_id: &str,
            pubkey: &[u8],
        ) -> Result<bool, StorageError> {
            self.inner.remove_membership(tenant_id, pubkey).await
        }

        async fn insert_invite(&self, record: RelayInviteRecord) -> Result<(), StorageError> {
            if self.mode == "insert_invite" {
                return Err(Self::error(self.mode));
            }
            self.inner.insert_invite(record).await
        }

        async fn invite_by_code(
            &self,
            tenant_id: &str,
            invite_code: &str,
        ) -> Result<Option<RelayInviteRecord>, StorageError> {
            if self.mode == "invite_by_code" {
                return Err(Self::error(self.mode));
            }
            self.inner.invite_by_code(tenant_id, invite_code).await
        }

        async fn delete_invite(
            &self,
            tenant_id: &str,
            invite_code: &str,
        ) -> Result<(), StorageError> {
            if self.mode == "delete_invite" {
                return Err(Self::error(self.mode));
            }
            self.inner.delete_invite(tenant_id, invite_code).await
        }
    }

    #[derive(Clone, Copy)]
    enum ScriptedStoreMode {
        QueryError,
        InsertError,
        DeleteError,
    }

    struct ScriptedStore {
        mode: ScriptedStoreMode,
        inner: MemoryStore,
    }

    impl ScriptedStore {
        fn query_error() -> Self {
            Self {
                mode: ScriptedStoreMode::QueryError,
                inner: MemoryStore::new(),
            }
        }

        fn insert_error() -> Self {
            Self {
                mode: ScriptedStoreMode::InsertError,
                inner: MemoryStore::new(),
            }
        }

        fn delete_error() -> Self {
            Self {
                mode: ScriptedStoreMode::DeleteError,
                inner: MemoryStore::new(),
            }
        }
    }

    #[async_trait]
    impl EventStore for ScriptedStore {
        async fn insert(&self, event: NostrEvent) -> Result<StoreOutcome, StoreError> {
            if matches!(self.mode, ScriptedStoreMode::InsertError) {
                return Err(StoreError::Backend("insert failure".to_string()));
            }
            self.inner.insert(event).await
        }

        async fn get(&self, id: &str) -> Result<Option<NostrEvent>, StoreError> {
            self.inner.get(id).await
        }

        async fn delete(&self, id: &str) -> Result<bool, StoreError> {
            if matches!(self.mode, ScriptedStoreMode::DeleteError) {
                return Err(StoreError::Backend("delete failure".to_string()));
            }
            self.inner.delete(id).await
        }

        async fn query(&self, filters: &[crate::Filter]) -> Result<Vec<NostrEvent>, StoreError> {
            if matches!(self.mode, ScriptedStoreMode::QueryError) {
                return Err(StoreError::Backend("query failure".to_string()));
            }
            self.inner.query(filters).await
        }
    }

    fn scripted_store_dyn(store: ScriptedStore) -> Arc<dyn EventStore> {
        Arc::new(store)
    }

    #[tokio::test]
    async fn scripted_store_get_and_delete_delegate_to_inner() {
        let store = ScriptedStore::query_error();
        let event = signed_event("scripted-store-paths");
        let event_id = event.id.clone();
        store.insert(event).await.expect("insert");
        let stored = store.get(&event_id).await.expect("get").expect("stored");
        assert_eq!(stored.id, event_id);
        let deleted = store.delete(&event_id).await.expect("delete");
        assert!(deleted);
    }

    fn signed_event(seed: &str) -> NostrEvent {
        signed_event_with_tags(seed, 1, Vec::new())
    }

    fn signed_event_with_tags(seed: &str, kind: u32, tags: Vec<Vec<String>>) -> NostrEvent {
        signed_event_with_tags_at(seed, kind, tags, 1)
    }

    fn signed_event_with_tags_at(
        seed: &str,
        kind: u32,
        tags: Vec<Vec<String>>,
        created_at: i64,
    ) -> NostrEvent {
        let secp = Secp256k1::new();
        let secret_key = SecretKey::from_slice(&[0x11; 32]).expect("secret");
        let keypair = Keypair::from_secret_key(&secp, &secret_key);
        let (pubkey, _) = secp256k1::XOnlyPublicKey::from_keypair(&keypair);

        let mut event = NostrEvent {
            id: seed.to_string(),
            pubkey: hex::encode(pubkey.serialize()),
            created_at,
            kind,
            tags,
            content: String::new(),
            sig: String::new(),
        };
        event.id = event.compute_id();
        let id_bytes = hex::decode(&event.id).expect("id bytes");
        let msg = secp256k1::Message::from_digest_slice(&id_bytes).expect("msg");
        let sig = secp.sign_schnorr(&msg, &keypair);
        event.sig = hex::encode(sig.as_ref());
        event
    }

    async fn authenticate_session<Store: EventStore>(session: &mut Session<Store>) {
        let challenge = session.auth_challenge().expect("challenge");
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        let auth_event = signed_event_with_tags_at(
            "auth-seed",
            super::AUTH_KIND,
            vec![
                vec!["challenge".to_string(), challenge],
                vec!["relay".to_string(), "wss://relay.example".to_string()],
            ],
            now,
        );
        let _ = session
            .handle_message(ClientMessage::Auth(
                serde_json::to_value(auth_event).expect("auth event"),
            ))
            .await;
    }

    #[tokio::test]
    async fn auth_accepts_valid_event() {
        let mut session =
            Session::with_policy_and_auth(MemoryStore::new(), Policy::default(), true);
        let challenge = session.auth_challenge().expect("challenge");
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        let event = signed_event_with_tags_at(
            "auth",
            super::AUTH_KIND,
            vec![
                vec!["challenge".to_string(), challenge],
                vec!["relay".to_string(), "wss://relay.example".to_string()],
            ],
            now,
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
    async fn auth_rejects_invalid_kind() {
        let mut session =
            Session::with_policy_and_auth(MemoryStore::new(), Policy::default(), true);
        let event = signed_event("auth-invalid-kind");
        let response = session
            .handle_message(ClientMessage::Auth(
                serde_json::to_value(event).expect("auth event"),
            ))
            .await;
        assert!(matches!(
            response[0],
            ServerMessage::Ok {
                accepted: false,
                ref message,
                ..
            } if message == "invalid auth kind"
        ));
    }

    #[tokio::test]
    async fn auth_rejects_invalid_signature() {
        let mut session =
            Session::with_policy_and_auth(MemoryStore::new(), Policy::default(), true);
        let challenge = session.auth_challenge().expect("challenge");
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        let mut event = signed_event_with_tags_at(
            "auth-bad-signature",
            super::AUTH_KIND,
            vec![
                vec!["challenge".to_string(), challenge],
                vec!["relay".to_string(), "wss://relay.example".to_string()],
            ],
            now,
        );
        event.sig = "00".repeat(64);
        let response = session
            .handle_message(ClientMessage::Auth(
                serde_json::to_value(event).expect("auth event"),
            ))
            .await;
        assert_ok_rejected_with_non_empty_message(&response[0]);
    }

    #[tokio::test]
    async fn auth_rejects_missing_relay_and_bad_challenge() {
        let mut session =
            Session::with_policy_and_auth(MemoryStore::new(), Policy::default(), true);
        let challenge = session.auth_challenge().expect("challenge");
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        let missing_relay = signed_event_with_tags_at(
            "auth-missing-relay",
            super::AUTH_KIND,
            vec![vec!["challenge".to_string(), challenge.clone()]],
            now,
        );
        let response = session
            .handle_message(ClientMessage::Auth(
                serde_json::to_value(missing_relay).expect("auth event"),
            ))
            .await;
        assert!(matches!(
            response[0],
            ServerMessage::Ok {
                accepted: false,
                ref message,
                ..
            } if message == "missing relay tag"
        ));

        let bad_challenge = signed_event_with_tags_at(
            "auth-bad-challenge",
            super::AUTH_KIND,
            vec![
                vec!["challenge".to_string(), "wrong".to_string()],
                vec!["relay".to_string(), "wss://relay.example".to_string()],
            ],
            now,
        );
        let response = session
            .handle_message(ClientMessage::Auth(
                serde_json::to_value(bad_challenge).expect("auth event"),
            ))
            .await;
        assert!(matches!(
            response[0],
            ServerMessage::Ok {
                accepted: false,
                ref message,
                ..
            } if message == "invalid challenge"
        ));
    }

    #[tokio::test]
    async fn auth_rejects_relay_tag_mismatch_and_stale_timestamp() {
        let mut session =
            Session::with_policy_and_auth(MemoryStore::new(), Policy::default(), true)
                .with_relay_url(Some("wss://relay.example".to_string()));
        let challenge = session.auth_challenge().expect("challenge");
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;

        let relay_mismatch = signed_event_with_tags_at(
            "auth-relay-mismatch",
            super::AUTH_KIND,
            vec![
                vec!["challenge".to_string(), challenge.clone()],
                vec!["relay".to_string(), "wss://other.example".to_string()],
            ],
            now,
        );
        let response = session
            .handle_message(ClientMessage::Auth(
                serde_json::to_value(relay_mismatch).expect("auth event"),
            ))
            .await;
        assert!(matches!(
            response[0],
            ServerMessage::Ok {
                accepted: false,
                ref message,
                ..
            } if message == "relay tag mismatch"
        ));

        let stale = signed_event_with_tags_at(
            "auth-stale",
            super::AUTH_KIND,
            vec![
                vec!["challenge".to_string(), challenge],
                vec!["relay".to_string(), "wss://relay.example".to_string()],
            ],
            now - super::AUTH_MAX_SKEW_SECS - 1,
        );
        let response = session
            .handle_message(ClientMessage::Auth(
                serde_json::to_value(stale).expect("auth event"),
            ))
            .await;
        assert!(matches!(
            response[0],
            ServerMessage::Ok {
                accepted: false,
                ref message,
                ..
            } if message == "auth event timestamp out of range"
        ));
    }

    #[tokio::test]
    async fn auth_rejects_invalid_payload_missing_challenge_and_invalid_relay_tag() {
        let mut session =
            Session::with_policy_and_auth(MemoryStore::new(), Policy::default(), true)
                .with_relay_url(Some("wss://relay.example".to_string()));

        let response = session
            .handle_message(ClientMessage::Auth(json!({"not": "an auth event"})))
            .await;
        assert_notice(&response[0]);

        let challenge = session.auth_challenge().expect("challenge");
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        let missing_challenge = signed_event_with_tags_at(
            "auth-missing-challenge",
            super::AUTH_KIND,
            vec![vec!["relay".to_string(), "wss://relay.example".to_string()]],
            now,
        );
        let response = session
            .handle_message(ClientMessage::Auth(
                serde_json::to_value(missing_challenge).expect("auth event"),
            ))
            .await;
        assert!(matches!(
            response[0],
            ServerMessage::Ok {
                accepted: false,
                ref message,
                ..
            } if message == "missing challenge tag"
        ));

        let invalid_relay = signed_event_with_tags_at(
            "auth-invalid-relay",
            super::AUTH_KIND,
            vec![
                vec!["challenge".to_string(), challenge],
                vec!["relay".to_string(), "wss:///".to_string()],
            ],
            now,
        );
        let response = session
            .handle_message(ClientMessage::Auth(
                serde_json::to_value(invalid_relay).expect("auth event"),
            ))
            .await;
        assert!(matches!(
            response[0],
            ServerMessage::Ok {
                accepted: false,
                ref message,
                ..
            } if message == "invalid relay tag"
        ));
    }

    #[tokio::test]
    async fn auth_rejects_when_not_enabled() {
        let mut session = Session::new(MemoryStore::new());
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        let event = signed_event_with_tags_at("auth-disabled", super::AUTH_KIND, Vec::new(), now);
        let response = session
            .handle_message(ClientMessage::Auth(
                serde_json::to_value(event).expect("auth event"),
            ))
            .await;
        assert!(matches!(
            response[0],
            ServerMessage::Ok {
                accepted: false,
                ref message,
                ..
            } if message == "auth not enabled"
        ));
    }

    #[tokio::test]
    async fn auth_required_rejects_event_without_auth() {
        let mut session =
            Session::with_policy_and_auth(MemoryStore::new(), Policy::default(), true);
        let event = signed_event("seed");
        let response = session
            .handle_message(ClientMessage::Event(serde_json::to_value(event).unwrap()))
            .await;
        assert_ok_rejected(&response[0]);
    }

    #[tokio::test]
    async fn event_rejects_when_write_auth_required_and_auth_state_missing() {
        let store = MemoryStore::new();
        let (tx, _) = tokio::sync::broadcast::channel(4);
        let mut session = Session::with_broadcast(store, Policy::default(), None, tx, false, true);
        session.auth = None;

        let response = session
            .handle_message(ClientMessage::Event(
                serde_json::to_value(signed_event("seed")).unwrap(),
            ))
            .await;
        assert!(matches!(
            response[0],
            ServerMessage::Ok {
                accepted: false,
                ref message,
                ..
            } if message == super::AUTH_REQUIRED_REASON
        ));
    }

    #[tokio::test]
    async fn event_rejects_invalid_payload_and_policy_violation() {
        let mut invalid_payload_session = Session::new(MemoryStore::new());
        let invalid_payload = invalid_payload_session
            .handle_message(ClientMessage::Event(json!({"not": "an event"})))
            .await;
        assert_notice(&invalid_payload[0]);

        let mut policy_session = Session::with_policy(
            MemoryStore::new(),
            Policy {
                max_future_seconds: 0,
                ..Policy::default()
            },
        );
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        let future_event = signed_event_with_tags_at("future", 1, Vec::new(), now + 120);
        let response = policy_session
            .handle_message(ClientMessage::Event(
                serde_json::to_value(future_event).expect("event"),
            ))
            .await;
        assert!(matches!(
            response[0],
            ServerMessage::Ok {
                accepted: false,
                ref message,
                ..
            } if message == "event timestamp too far in future"
        ));
    }

    #[tokio::test]
    async fn event_rejects_auth_pubkey_mismatch_and_insert_failure() {
        let mut mismatch_session =
            Session::with_policy_and_auth(MemoryStore::new(), Policy::default(), true);
        authenticate_session(&mut mismatch_session).await;
        let mismatched_event = signed_event_with_tags("mismatch", 1, Vec::new());
        mismatch_session
            .auth
            .as_mut()
            .expect("auth")
            .authenticated_pubkey = Some("aa".repeat(32));
        let mismatch_response = mismatch_session
            .handle_message(ClientMessage::Event(
                serde_json::to_value(mismatched_event).expect("event"),
            ))
            .await;
        assert!(matches!(
            mismatch_response[0],
            ServerMessage::Ok {
                accepted: false,
                ref message,
                ..
            } if message == super::AUTH_PUBKEY_MISMATCH_REASON
        ));

        let mut insert_failure = Session::new(scripted_store_dyn(ScriptedStore::insert_error()));
        let response = insert_failure
            .handle_message(ClientMessage::Event(
                serde_json::to_value(signed_event("insert-fail")).expect("event"),
            ))
            .await;
        assert_notice(&response[0]);
    }

    #[tokio::test]
    async fn event_rejects_auth_kind_as_regular_event() {
        let mut session = Session::new(MemoryStore::new());
        let event = signed_event_with_tags("auth-event", super::AUTH_KIND, Vec::new());
        let response = session
            .handle_message(ClientMessage::Event(
                serde_json::to_value(event).expect("event"),
            ))
            .await;
        assert!(matches!(
            response[0],
            ServerMessage::Ok {
                accepted: false,
                ref message,
                ..
            } if message == "auth event must use AUTH"
        ));
    }

    #[tokio::test]
    async fn join_request_accepts_valid_invite() {
        let membership = Arc::new(InMemoryRepositories::new());
        let tenant_id = "tenant-1";
        let invite = RelayInviteRecord::new(
            tenant_id,
            "invite-code",
            "member",
            &"11".repeat(32),
            None,
            None,
            1,
        )
        .expect("invite");
        membership
            .insert_invite(invite)
            .await
            .expect("invite insert");

        let event = signed_event_with_tags(
            "join",
            super::NIP43_JOIN_KIND,
            vec![
                vec!["-".to_string()],
                vec!["claim".to_string(), "invite-code".to_string()],
            ],
        );
        let mut session = Session::new(MemoryStore::new())
            .with_membership(Some(tenant_id.to_string()), Some(membership.clone()));
        let response = session
            .handle_message(ClientMessage::Event(
                serde_json::to_value(event.clone()).unwrap(),
            ))
            .await;
        assert!(matches!(
            response[0],
            ServerMessage::Ok { accepted: true, .. }
        ));

        let pubkey_bytes = hex::decode(&event.pubkey).expect("pubkey");
        let record = membership
            .membership_by_pubkey(tenant_id, &pubkey_bytes)
            .await
            .expect("membership lookup");
        assert!(record.is_some());
        let invite = membership
            .invite_by_code(tenant_id, "invite-code")
            .await
            .expect("invite lookup");
        assert!(invite.is_none());
    }

    #[tokio::test]
    async fn join_request_rejects_missing_claim() {
        let membership = Arc::new(InMemoryRepositories::new());
        let mut session = Session::new(MemoryStore::new())
            .with_membership(Some("tenant-1".to_string()), Some(membership));
        let event = signed_event_with_tags(
            "join-missing-claim",
            super::NIP43_JOIN_KIND,
            vec![vec!["-".to_string()]],
        );
        let response = session
            .handle_message(ClientMessage::Event(
                serde_json::to_value(event).expect("event"),
            ))
            .await;
        assert!(matches!(
            response[0],
            ServerMessage::Ok {
                accepted: false,
                ref message,
                ..
            } if message.contains("missing invite code")
        ));
    }

    #[tokio::test]
    async fn join_request_rejects_without_membership_backend_tenant_or_nip70_tag() {
        let join_event = signed_event_with_tags(
            "join-guard",
            super::NIP43_JOIN_KIND,
            vec![
                vec!["-".to_string()],
                vec!["claim".to_string(), "invite-code".to_string()],
            ],
        );
        let mut no_backend = Session::new(MemoryStore::new());
        let response = no_backend
            .handle_message(ClientMessage::Event(
                serde_json::to_value(join_event.clone()).expect("event"),
            ))
            .await;
        assert!(matches!(
            response[0],
            ServerMessage::Ok {
                accepted: false,
                ref message,
                ..
            } if message.contains("relay does not support membership")
        ));

        let membership = Arc::new(InMemoryRepositories::new());
        let mut no_tenant =
            Session::new(MemoryStore::new()).with_membership(None, Some(membership.clone()));
        let response = no_tenant
            .handle_message(ClientMessage::Event(
                serde_json::to_value(join_event.clone()).expect("event"),
            ))
            .await;
        assert!(matches!(
            response[0],
            ServerMessage::Ok {
                accepted: false,
                ref message,
                ..
            } if message.contains("relay does not support membership")
        ));

        let no_nip70 = signed_event_with_tags(
            "join-no-nip70",
            super::NIP43_JOIN_KIND,
            vec![vec!["claim".to_string(), "invite-code".to_string()]],
        );
        let mut no_nip70_session = Session::new(MemoryStore::new())
            .with_membership(Some("tenant-1".to_string()), Some(membership));
        let response = no_nip70_session
            .handle_message(ClientMessage::Event(
                serde_json::to_value(no_nip70).expect("event"),
            ))
            .await;
        assert!(matches!(
            response[0],
            ServerMessage::Ok {
                accepted: false,
                ref message,
                ..
            } if message.contains("missing nip-70 tag")
        ));
    }

    #[tokio::test]
    async fn handle_membership_event_rejects_invalid_pubkey_hex() {
        let tenant_id = "tenant-1";
        let membership = Arc::new(InMemoryRepositories::new());
        let mut session = Session::new(MemoryStore::new())
            .with_membership(Some(tenant_id.to_string()), Some(membership));
        let mut event = signed_event_with_tags(
            "join-invalid-pubkey",
            super::NIP43_JOIN_KIND,
            vec![
                vec!["-".to_string()],
                vec!["claim".to_string(), "invite-code".to_string()],
            ],
        );
        event.pubkey = "not-hex".to_string();

        let response = session
            .handle_membership_event(&event, event.created_at)
            .await
            .expect("membership response");
        assert!(matches!(
            response[0],
            ServerMessage::Ok {
                accepted: false,
                ref message,
                ..
            } if message.contains("invalid pubkey")
        ));
    }

    #[tokio::test]
    async fn join_and_leave_reject_invalid_or_duplicate_membership_states() {
        let membership = Arc::new(InMemoryRepositories::new());
        let tenant_id = "tenant-1";
        let join_unknown_invite = signed_event_with_tags(
            "join-unknown-invite",
            super::NIP43_JOIN_KIND,
            vec![
                vec!["-".to_string()],
                vec!["claim".to_string(), "unknown".to_string()],
            ],
        );
        let mut session = Session::new(MemoryStore::new())
            .with_membership(Some(tenant_id.to_string()), Some(membership.clone()));
        let response = session
            .handle_message(ClientMessage::Event(
                serde_json::to_value(join_unknown_invite.clone()).expect("event"),
            ))
            .await;
        assert!(matches!(
            response[0],
            ServerMessage::Ok {
                accepted: false,
                ref message,
                ..
            } if message.contains("invalid invite code")
        ));

        let pubkey_bytes = hex::decode(&join_unknown_invite.pubkey).expect("pubkey");
        let invite = RelayInviteRecord::new(
            tenant_id,
            "active-member",
            "member",
            &"11".repeat(32),
            None,
            None,
            1,
        )
        .expect("invite");
        membership
            .insert_invite(invite)
            .await
            .expect("insert invite");
        membership
            .upsert_membership(RelayMembershipRecord {
                tenant_id: tenant_id.to_string(),
                pubkey: pubkey_bytes.clone(),
                role: "member".to_string(),
                status: super::MEMBERSHIP_STATUS_ACTIVE.to_string(),
                created_at: 1,
                updated_at: 1,
            })
            .await
            .expect("insert membership");
        let duplicate_join = signed_event_with_tags(
            "join-duplicate",
            super::NIP43_JOIN_KIND,
            vec![
                vec!["-".to_string()],
                vec!["claim".to_string(), "active-member".to_string()],
            ],
        );
        let response = session
            .handle_message(ClientMessage::Event(
                serde_json::to_value(duplicate_join).expect("event"),
            ))
            .await;
        assert!(matches!(
            response[0],
            ServerMessage::Ok {
                accepted: true,
                ref message,
                ..
            } if message.contains("already a member")
        ));

        let mut leave_session = Session::new(MemoryStore::new())
            .with_membership(Some(tenant_id.to_string()), Some(membership.clone()));
        let leave_event = signed_event_with_tags(
            "leave-not-member",
            super::NIP43_LEAVE_KIND,
            vec![vec!["-".to_string()]],
        );
        let response = leave_session
            .handle_message(ClientMessage::Event(
                serde_json::to_value(leave_event.clone()).expect("event"),
            ))
            .await;
        assert!(matches!(
            response[0],
            ServerMessage::Ok {
                accepted: true,
                ref message,
                ..
            } if message.contains("access revoked")
        ));

        let leave_pubkey = hex::decode(&leave_event.pubkey).expect("pubkey");
        membership
            .upsert_membership(RelayMembershipRecord {
                tenant_id: tenant_id.to_string(),
                pubkey: leave_pubkey,
                role: "member".to_string(),
                status: super::MEMBERSHIP_STATUS_LEFT.to_string(),
                created_at: 1,
                updated_at: 1,
            })
            .await
            .expect("insert left membership");
        let response = leave_session
            .handle_message(ClientMessage::Event(
                serde_json::to_value(leave_event).expect("event"),
            ))
            .await;
        assert!(matches!(
            response[0],
            ServerMessage::Ok {
                accepted: false,
                ref message,
                ..
            } if message.contains("not a member")
        ));
    }

    #[tokio::test]
    async fn join_request_rejects_expired_or_mismatched_invite() {
        let membership = Arc::new(InMemoryRepositories::new());
        let tenant_id = "tenant-1";
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        let expired = RelayInviteRecord::new(
            tenant_id,
            "expired",
            "member",
            &"11".repeat(32),
            None,
            Some(now - 1),
            now - 10,
        )
        .expect("invite");
        membership
            .insert_invite(expired)
            .await
            .expect("insert invite");

        let invitee_mismatch = RelayInviteRecord::new(
            tenant_id,
            "invitee-mismatch",
            "member",
            &"11".repeat(32),
            Some(&"22".repeat(32)),
            Some(now + 60),
            now,
        )
        .expect("invite");
        membership
            .insert_invite(invitee_mismatch)
            .await
            .expect("insert invite");

        let mut session = Session::new(MemoryStore::new())
            .with_membership(Some(tenant_id.to_string()), Some(membership));

        let expired_event = signed_event_with_tags(
            "join-expired",
            super::NIP43_JOIN_KIND,
            vec![
                vec!["-".to_string()],
                vec!["claim".to_string(), "expired".to_string()],
            ],
        );
        let response = session
            .handle_message(ClientMessage::Event(
                serde_json::to_value(expired_event).expect("event"),
            ))
            .await;
        assert!(matches!(
            response[0],
            ServerMessage::Ok {
                accepted: false,
                ref message,
                ..
            } if message.contains("expired")
        ));

        let mismatch_event = signed_event_with_tags(
            "join-mismatch",
            super::NIP43_JOIN_KIND,
            vec![
                vec!["-".to_string()],
                vec!["claim".to_string(), "invitee-mismatch".to_string()],
            ],
        );
        let response = session
            .handle_message(ClientMessage::Event(
                serde_json::to_value(mismatch_event).expect("event"),
            ))
            .await;
        assert!(matches!(
            response[0],
            ServerMessage::Ok {
                accepted: false,
                ref message,
                ..
            } if message.contains("does not match pubkey")
        ));
    }

    #[tokio::test]
    async fn join_and_invite_paths_surface_membership_repository_errors() {
        let tenant_id = "tenant-1";
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;

        let invite_error = Arc::new(ScriptedMembership::new("invite_by_code"));
        let mut session = Session::new(MemoryStore::new())
            .with_membership(Some(tenant_id.to_string()), Some(invite_error));
        let join_event = signed_event_with_tags(
            "join-invite-err",
            super::NIP43_JOIN_KIND,
            vec![
                vec!["-".to_string()],
                vec!["claim".to_string(), "code".to_string()],
            ],
        );
        let response = session
            .handle_message(ClientMessage::Event(
                serde_json::to_value(join_event).expect("event"),
            ))
            .await;
        assert_notice(&response[0]);

        let delete_error = Arc::new(ScriptedMembership::new("delete_invite"));
        let valid_invite = RelayInviteRecord::new(
            tenant_id,
            "delete-fails",
            "member",
            &"11".repeat(32),
            None,
            Some(now + 60),
            now,
        )
        .expect("invite");
        delete_error
            .insert_invite(valid_invite)
            .await
            .expect("insert invite");
        let mut session = Session::new(MemoryStore::new())
            .with_membership(Some(tenant_id.to_string()), Some(delete_error));
        let join_event = signed_event_with_tags(
            "join-delete-err",
            super::NIP43_JOIN_KIND,
            vec![
                vec!["-".to_string()],
                vec!["claim".to_string(), "delete-fails".to_string()],
            ],
        );
        let response = session
            .handle_message(ClientMessage::Event(
                serde_json::to_value(join_event).expect("event"),
            ))
            .await;
        assert_notice(&response[0]);
    }

    #[tokio::test]
    async fn join_request_surfaces_membership_lookup_and_upsert_errors() {
        let tenant_id = "tenant-1";
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        let join_event = signed_event_with_tags(
            "join-lookup-upsert-errors",
            super::NIP43_JOIN_KIND,
            vec![
                vec!["-".to_string()],
                vec!["claim".to_string(), "join-error".to_string()],
            ],
        );

        let membership_lookup_error = Arc::new(ScriptedMembership::new("membership_by_pubkey"));
        membership_lookup_error
            .insert_invite(
                RelayInviteRecord::new(
                    tenant_id,
                    "join-error",
                    "member",
                    &"11".repeat(32),
                    None,
                    Some(now + 60),
                    now,
                )
                .expect("invite"),
            )
            .await
            .expect("insert invite");
        let mut session = Session::new(MemoryStore::new())
            .with_membership(Some(tenant_id.to_string()), Some(membership_lookup_error));
        let response = session
            .handle_message(ClientMessage::Event(
                serde_json::to_value(join_event.clone()).expect("event"),
            ))
            .await;
        assert_notice(&response[0]);

        let upsert_error = Arc::new(ScriptedMembership::new("upsert_membership"));
        upsert_error
            .insert_invite(
                RelayInviteRecord::new(
                    tenant_id,
                    "join-error",
                    "member",
                    &"11".repeat(32),
                    None,
                    Some(now + 60),
                    now,
                )
                .expect("invite"),
            )
            .await
            .expect("insert invite");
        let mut session = Session::new(MemoryStore::new())
            .with_membership(Some(tenant_id.to_string()), Some(upsert_error));
        let response = session
            .handle_message(ClientMessage::Event(
                serde_json::to_value(join_event).expect("event"),
            ))
            .await;
        assert_notice(&response[0]);
    }

    #[tokio::test]
    async fn join_request_accepts_matching_invitee_pubkey() {
        let tenant_id = "tenant-1";
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        let membership = Arc::new(InMemoryRepositories::new());
        let join_event = signed_event_with_tags(
            "join-invitee-match",
            super::NIP43_JOIN_KIND,
            vec![
                vec!["-".to_string()],
                vec!["claim".to_string(), "invitee-match".to_string()],
            ],
        );
        membership
            .insert_invite(
                RelayInviteRecord::new(
                    tenant_id,
                    "invitee-match",
                    "member",
                    &"11".repeat(32),
                    Some(&join_event.pubkey),
                    Some(now + 60),
                    now,
                )
                .expect("invite"),
            )
            .await
            .expect("insert invite");

        let mut session = Session::new(MemoryStore::new())
            .with_membership(Some(tenant_id.to_string()), Some(membership));
        let response = session
            .handle_message(ClientMessage::Event(
                serde_json::to_value(join_event).expect("event"),
            ))
            .await;
        assert!(matches!(
            response[0],
            ServerMessage::Ok { accepted: true, .. }
        ));
    }

    #[tokio::test]
    async fn leave_request_surfaces_membership_lookup_and_upsert_errors() {
        let tenant_id = "tenant-1";
        let leave_event = signed_event_with_tags(
            "leave-lookup-upsert-errors",
            super::NIP43_LEAVE_KIND,
            vec![vec!["-".to_string()]],
        );
        let pubkey_bytes = hex::decode(&leave_event.pubkey).expect("pubkey");

        let lookup_error = Arc::new(ScriptedMembership::new("membership_by_pubkey"));
        let mut session = Session::new(MemoryStore::new())
            .with_membership(Some(tenant_id.to_string()), Some(lookup_error));
        let response = session
            .handle_message(ClientMessage::Event(
                serde_json::to_value(leave_event.clone()).expect("event"),
            ))
            .await;
        assert_notice(&response[0]);

        let upsert_error = Arc::new(ScriptedMembership::new("upsert_membership"));
        upsert_error
            .inner
            .upsert_membership(RelayMembershipRecord {
                tenant_id: tenant_id.to_string(),
                pubkey: pubkey_bytes,
                role: "member".to_string(),
                status: super::MEMBERSHIP_STATUS_ACTIVE.to_string(),
                created_at: 1,
                updated_at: 1,
            })
            .await
            .expect("seed membership");
        let mut session = Session::new(MemoryStore::new())
            .with_membership(Some(tenant_id.to_string()), Some(upsert_error));
        let response = session
            .handle_message(ClientMessage::Event(
                serde_json::to_value(leave_event).expect("event"),
            ))
            .await;
        assert_notice(&response[0]);
    }

    #[tokio::test]
    async fn leave_request_rejects_when_member_record_missing() {
        let tenant_id = "tenant-1";
        let membership = Arc::new(InMemoryRepositories::new());
        let event = signed_event_with_tags(
            "leave-missing-member",
            super::NIP43_LEAVE_KIND,
            vec![vec!["-".to_string()]],
        );
        let mut session = Session::new(MemoryStore::new())
            .with_membership(Some(tenant_id.to_string()), Some(membership));
        let response = session
            .handle_message(ClientMessage::Event(
                serde_json::to_value(event).expect("event"),
            ))
            .await;
        assert!(matches!(
            response[0],
            ServerMessage::Ok {
                accepted: false,
                ref message,
                ..
            } if message.contains("not a member")
        ));
    }

    #[tokio::test]
    async fn leave_request_marks_member_left() {
        let membership = Arc::new(InMemoryRepositories::new());
        let tenant_id = "tenant-1";
        let event = signed_event_with_tags(
            "leave",
            super::NIP43_LEAVE_KIND,
            vec![vec!["-".to_string()]],
        );
        let pubkey_bytes = hex::decode(&event.pubkey).expect("pubkey");
        let record = RelayMembershipRecord {
            tenant_id: tenant_id.to_string(),
            pubkey: pubkey_bytes.clone(),
            role: "member".to_string(),
            status: super::MEMBERSHIP_STATUS_ACTIVE.to_string(),
            created_at: 1,
            updated_at: 1,
        };
        membership
            .upsert_membership(record)
            .await
            .expect("membership insert");

        let mut session = Session::new(MemoryStore::new())
            .with_membership(Some(tenant_id.to_string()), Some(membership.clone()));
        let response = session
            .handle_message(ClientMessage::Event(
                serde_json::to_value(event.clone()).unwrap(),
            ))
            .await;
        assert!(matches!(
            response[0],
            ServerMessage::Ok { accepted: true, .. }
        ));

        let record = membership
            .membership_by_pubkey(tenant_id, &pubkey_bytes)
            .await
            .expect("membership lookup")
            .expect("member");
        assert_eq!(record.status, super::MEMBERSHIP_STATUS_LEFT);
    }

    #[tokio::test]
    async fn req_includes_membership_list_event() {
        let membership = Arc::new(InMemoryRepositories::new());
        let tenant_id = "tenant-1";
        let active = RelayMembershipRecord {
            tenant_id: tenant_id.to_string(),
            pubkey: hex::decode(&"22".repeat(32)).expect("pubkey"),
            role: "member".to_string(),
            status: super::MEMBERSHIP_STATUS_ACTIVE.to_string(),
            created_at: 1,
            updated_at: 1,
        };
        let inactive = RelayMembershipRecord {
            tenant_id: tenant_id.to_string(),
            pubkey: hex::decode(&"33".repeat(32)).expect("pubkey"),
            role: "member".to_string(),
            status: super::MEMBERSHIP_STATUS_LEFT.to_string(),
            created_at: 1,
            updated_at: 1,
        };
        membership
            .upsert_membership(active)
            .await
            .expect("membership insert");
        membership
            .upsert_membership(inactive)
            .await
            .expect("membership insert");

        let secp = Secp256k1::new();
        let secret_key = SecretKey::from_slice(&[0x22; 32]).expect("secret");
        let keypair = Keypair::from_secret_key(&secp, &secret_key);
        let (pubkey, _) = secp256k1::XOnlyPublicKey::from_keypair(&keypair);

        let mut session = Session::new(MemoryStore::new())
            .with_membership(Some(tenant_id.to_string()), Some(membership))
            .with_relay_signer(
                pubkey.serialize().to_vec(),
                secret_key.secret_bytes().to_vec(),
            );

        let responses = session
            .handle_message(ClientMessage::Req {
                subscription_id: "sub".to_string(),
                filters: vec![json!({"kinds": [super::NIP43_MEMBERSHIP_KIND]})],
            })
            .await;

        let events = responses
            .iter()
            .filter_map(|response| match response {
                ServerMessage::Event { event, .. } => Some(event),
                _ => None,
            })
            .collect::<Vec<_>>();
        let event = events.first().copied().expect("membership event");
        assert_eq!(
            event.get("kind").and_then(|kind| kind.as_u64()),
            Some(super::NIP43_MEMBERSHIP_KIND as u64)
        );
        let tags = event
            .get("tags")
            .and_then(|tags| tags.as_array())
            .expect("tags");
        let member_tags = tags
            .iter()
            .filter_map(|tag| tag.as_array())
            .filter(|tag| tag.first().and_then(|value| value.as_str()) == Some("member"))
            .count();
        assert_eq!(member_tags, 1);
    }

    #[test]
    fn sign_event_populates_id_and_signature() {
        let secret_key = SecretKey::from_slice(&[0x66; 32]).expect("secret");
        let secp = Secp256k1::new();
        let keypair = Keypair::from_secret_key(&secp, &secret_key);
        let (pubkey, _) = secp256k1::XOnlyPublicKey::from_keypair(&keypair);
        let signer = super::TenantSigner {
            pubkey: hex::encode(pubkey.serialize()),
            secret_key,
        };
        let mut event = NostrEvent {
            id: "placeholder".to_string(),
            pubkey: signer.pubkey.clone(),
            created_at: 1,
            kind: super::NIP43_MEMBERSHIP_KIND,
            tags: vec![vec!["-".to_string()]],
            content: String::new(),
            sig: String::new(),
        };

        super::sign_event(&mut event, &signer);
        assert_eq!(event.id.len(), 64);
        assert_eq!(event.sig.len(), 128);
    }

    #[tokio::test]
    async fn build_membership_list_event_returns_signed_event_when_configured() {
        let membership = Arc::new(InMemoryRepositories::new());
        let tenant_id = "tenant-1";
        membership
            .upsert_membership(RelayMembershipRecord {
                tenant_id: tenant_id.to_string(),
                pubkey: hex::decode(&"77".repeat(32)).expect("pubkey"),
                role: "member".to_string(),
                status: super::MEMBERSHIP_STATUS_ACTIVE.to_string(),
                created_at: 1,
                updated_at: 1,
            })
            .await
            .expect("membership insert");

        let secret_key = SecretKey::from_slice(&[0x67; 32]).expect("secret");
        let secp = Secp256k1::new();
        let keypair = Keypair::from_secret_key(&secp, &secret_key);
        let (pubkey, _) = secp256k1::XOnlyPublicKey::from_keypair(&keypair);
        let session = Session::new(MemoryStore::new())
            .with_membership(Some(tenant_id.to_string()), Some(membership))
            .with_relay_signer(
                pubkey.serialize().to_vec(),
                secret_key.secret_bytes().to_vec(),
            );

        let event = session
            .build_membership_list_event(10)
            .await
            .expect("membership list")
            .expect("membership list event");
        assert_eq!(event.kind, super::NIP43_MEMBERSHIP_KIND);
        assert_eq!(event.id.len(), 64);
        assert_eq!(event.sig.len(), 128);
    }

    #[tokio::test]
    async fn build_invite_event_returns_signed_event_for_active_member() {
        let membership = Arc::new(InMemoryRepositories::new());
        let tenant_id = "tenant-1";
        let secret_key = SecretKey::from_slice(&[0x68; 32]).expect("secret");
        let secp = Secp256k1::new();
        let keypair = Keypair::from_secret_key(&secp, &secret_key);
        let (pubkey, _) = secp256k1::XOnlyPublicKey::from_keypair(&keypair);
        let mut session =
            Session::with_policy_and_auth(MemoryStore::new(), Policy::default(), true)
                .with_membership(Some(tenant_id.to_string()), Some(membership.clone()))
                .with_relay_signer(
                    pubkey.serialize().to_vec(),
                    secret_key.secret_bytes().to_vec(),
                );
        authenticate_session(&mut session).await;
        let authenticated_pubkey = session
            .authenticated_pubkey()
            .expect("authenticated pubkey")
            .to_string();
        membership
            .upsert_membership(RelayMembershipRecord {
                tenant_id: tenant_id.to_string(),
                pubkey: hex::decode(&authenticated_pubkey).expect("pubkey"),
                role: "member".to_string(),
                status: super::MEMBERSHIP_STATUS_ACTIVE.to_string(),
                created_at: 1,
                updated_at: 1,
            })
            .await
            .expect("membership insert");

        let event = session.build_invite_event(10).await.expect("invite");
        assert_eq!(event.kind, super::NIP43_INVITE_KIND);
        assert_eq!(event.id.len(), 64);
        assert_eq!(event.sig.len(), 128);
        assert!(event.tags.iter().any(|tag| {
            tag.first().map(String::as_str) == Some("claim")
                && tag.get(1).is_some_and(|value| !value.is_empty())
        }));
    }

    #[tokio::test]
    async fn req_membership_required_surfaces_repository_failure() {
        let membership = Arc::new(ScriptedMembership::new("membership_by_pubkey"));
        let mut session =
            Session::with_policy_and_auth(MemoryStore::new(), Policy::default(), true)
                .with_membership(Some("tenant-1".to_string()), Some(membership))
                .with_membership_requirements(true, false);
        authenticate_session(&mut session).await;
        let response = session
            .handle_message(ClientMessage::Req {
                subscription_id: "sub".to_string(),
                filters: vec![json!({})],
            })
            .await;
        assert!(matches!(
            response[0],
            ServerMessage::Closed {
                ref message, ..
            } if message.contains("membership_by_pubkey failure")
        ));
    }

    #[tokio::test]
    async fn membership_requirements_reject_without_backend_or_tenant() {
        let mut no_backend =
            Session::with_policy_and_auth(MemoryStore::new(), Policy::default(), true)
                .with_membership_requirements(true, false);
        authenticate_session(&mut no_backend).await;
        let response = no_backend
            .handle_message(ClientMessage::Req {
                subscription_id: "sub".to_string(),
                filters: vec![json!({})],
            })
            .await;
        assert!(matches!(
            response[0],
            ServerMessage::Closed {
                ref message, ..
            } if message.contains("relay does not support membership")
        ));

        let membership = Arc::new(InMemoryRepositories::new());
        let mut no_tenant =
            Session::with_policy_and_auth(MemoryStore::new(), Policy::default(), true)
                .with_membership(None, Some(membership))
                .with_membership_requirements(true, false);
        authenticate_session(&mut no_tenant).await;
        let response = no_tenant
            .handle_message(ClientMessage::Req {
                subscription_id: "sub".to_string(),
                filters: vec![json!({})],
            })
            .await;
        assert!(matches!(
            response[0],
            ServerMessage::Closed {
                ref message, ..
            } if message.contains("relay does not support membership")
        ));
    }

    #[tokio::test]
    async fn membership_requirements_reject_without_auth_or_active_membership() {
        let tenant_id = "tenant-1";
        let membership = Arc::new(InMemoryRepositories::new());
        let mut unauthenticated = Session::new(MemoryStore::new())
            .with_membership(Some(tenant_id.to_string()), Some(membership.clone()))
            .with_membership_requirements(true, false);
        let response = unauthenticated
            .handle_message(ClientMessage::Req {
                subscription_id: "sub".to_string(),
                filters: vec![json!({})],
            })
            .await;
        assert!(matches!(
            response[0],
            ServerMessage::Closed {
                ref message, ..
            } if message == super::AUTH_REQUIRED_REASON
        ));

        let member_event = signed_event("member-left");
        let record = RelayMembershipRecord {
            tenant_id: tenant_id.to_string(),
            pubkey: hex::decode(&member_event.pubkey).expect("pubkey"),
            role: "member".to_string(),
            status: super::MEMBERSHIP_STATUS_LEFT.to_string(),
            created_at: 1,
            updated_at: 1,
        };
        membership
            .upsert_membership(record)
            .await
            .expect("membership insert");

        let mut inactive =
            Session::with_policy_and_auth(MemoryStore::new(), Policy::default(), true)
                .with_membership(Some(tenant_id.to_string()), Some(membership))
                .with_membership_requirements(true, false);
        authenticate_session(&mut inactive).await;
        let response = inactive
            .handle_message(ClientMessage::Req {
                subscription_id: "sub".to_string(),
                filters: vec![json!({})],
            })
            .await;
        assert!(matches!(
            response[0],
            ServerMessage::Closed {
                ref message, ..
            } if message.starts_with(super::RESTRICTED_PREFIX)
        ));
    }

    #[tokio::test]
    async fn membership_requirements_allow_active_members_for_req_and_event() {
        let tenant_id = "tenant-1";
        let membership = Arc::new(InMemoryRepositories::new());

        let mut req_session =
            Session::with_policy_and_auth(MemoryStore::new(), Policy::default(), true)
                .with_membership(Some(tenant_id.to_string()), Some(membership.clone()))
                .with_membership_requirements(true, false);
        authenticate_session(&mut req_session).await;
        let req_pubkey = req_session
            .auth
            .as_ref()
            .and_then(|auth| auth.authenticated_pubkey.as_ref())
            .cloned()
            .expect("auth pubkey");
        membership
            .upsert_membership(RelayMembershipRecord {
                tenant_id: tenant_id.to_string(),
                pubkey: hex::decode(req_pubkey).expect("pubkey bytes"),
                role: "member".to_string(),
                status: super::MEMBERSHIP_STATUS_ACTIVE.to_string(),
                created_at: 1,
                updated_at: 1,
            })
            .await
            .expect("membership insert");
        let req_response = req_session
            .handle_message(ClientMessage::Req {
                subscription_id: "sub".to_string(),
                filters: vec![json!({})],
            })
            .await;
        assert!(
            req_response
                .iter()
                .any(|message| matches!(message, ServerMessage::Eose { .. }))
        );

        let (tx, _) = tokio::sync::broadcast::channel(4);
        let mut event_session =
            Session::with_broadcast(MemoryStore::new(), Policy::default(), None, tx, true, true)
                .with_membership(Some(tenant_id.to_string()), Some(membership.clone()))
                .with_membership_requirements(false, true);
        authenticate_session(&mut event_session).await;
        let event_pubkey = event_session
            .auth
            .as_ref()
            .and_then(|auth| auth.authenticated_pubkey.as_ref())
            .cloned()
            .expect("auth pubkey");
        membership
            .upsert_membership(RelayMembershipRecord {
                tenant_id: tenant_id.to_string(),
                pubkey: hex::decode(event_pubkey).expect("pubkey bytes"),
                role: "member".to_string(),
                status: super::MEMBERSHIP_STATUS_ACTIVE.to_string(),
                created_at: 2,
                updated_at: 2,
            })
            .await
            .expect("membership insert");
        let event_response = event_session
            .handle_message(ClientMessage::Event(
                serde_json::to_value(signed_event("seed-active-member")).expect("event"),
            ))
            .await;
        assert!(matches!(
            event_response[0],
            ServerMessage::Ok {
                accepted: true,
                ref message,
                ..
            } if message == "saved"
        ));
    }

    #[tokio::test]
    async fn req_membership_requirements_reject_invalid_authenticated_pubkey_hex() {
        let membership = Arc::new(InMemoryRepositories::new());
        let mut session =
            Session::with_policy_and_auth(MemoryStore::new(), Policy::default(), true)
                .with_membership(Some("tenant-1".to_string()), Some(membership))
                .with_membership_requirements(true, false);
        session.auth.as_mut().expect("auth").authenticated_pubkey = Some("not-hex".to_string());
        let response = session
            .handle_message(ClientMessage::Req {
                subscription_id: "sub".to_string(),
                filters: vec![json!({})],
            })
            .await;
        assert!(matches!(
            response[0],
            ServerMessage::Closed {
                ref message, ..
            } if message.contains("invalid pubkey")
        ));
    }

    #[tokio::test]
    async fn event_membership_requirements_reject_invalid_authenticated_pubkey_hex() {
        let membership = Arc::new(InMemoryRepositories::new());
        let (tx, _) = tokio::sync::broadcast::channel(4);
        let mut session =
            Session::with_broadcast(MemoryStore::new(), Policy::default(), None, tx, true, false)
                .with_membership(Some("tenant-1".to_string()), Some(membership))
                .with_membership_requirements(false, true);
        session.auth.as_mut().expect("auth").authenticated_pubkey = Some("not-hex".to_string());
        let response = session
            .handle_message(ClientMessage::Event(
                serde_json::to_value(signed_event("seed-invalid-auth")).expect("event"),
            ))
            .await;
        assert!(matches!(
            response[0],
            ServerMessage::Ok {
                accepted: false,
                ref message,
                ..
            } if message.contains("invalid pubkey")
        ));
    }

    #[tokio::test]
    async fn req_membership_and_invite_generation_handle_missing_signer_and_bad_auth_pubkey() {
        let membership = Arc::new(InMemoryRepositories::new());
        let tenant_id = "tenant-1";
        let active = RelayMembershipRecord {
            tenant_id: tenant_id.to_string(),
            pubkey: hex::decode(&"11".repeat(32)).expect("pubkey"),
            role: "member".to_string(),
            status: super::MEMBERSHIP_STATUS_ACTIVE.to_string(),
            created_at: 1,
            updated_at: 1,
        };
        membership
            .upsert_membership(active)
            .await
            .expect("membership");

        let mut missing_signer = Session::new(MemoryStore::new())
            .with_membership(Some(tenant_id.to_string()), Some(membership.clone()))
            .with_relay_signer(vec![1, 2, 3], vec![4; 32]);
        let response = missing_signer
            .handle_message(ClientMessage::Req {
                subscription_id: "sub".to_string(),
                filters: vec![json!({"kinds":[super::NIP43_MEMBERSHIP_KIND]})],
            })
            .await;
        assert!(
            !response
                .iter()
                .any(|message| matches!(message, ServerMessage::Event { .. }))
        );

        let relay_secret = SecretKey::from_slice(&[0x55; 32]).expect("secret");
        let secp = Secp256k1::new();
        let relay_keypair = Keypair::from_secret_key(&secp, &relay_secret);
        let (relay_pubkey, _) = secp256k1::XOnlyPublicKey::from_keypair(&relay_keypair);
        let mut bad_auth =
            Session::with_policy_and_auth(MemoryStore::new(), Policy::default(), true)
                .with_membership(Some(tenant_id.to_string()), Some(membership))
                .with_relay_signer(
                    relay_pubkey.serialize().to_vec(),
                    relay_secret.secret_bytes().to_vec(),
                );
        bad_auth.auth.as_mut().expect("auth").authenticated_pubkey = Some("invalid".to_string());
        let response = bad_auth
            .handle_message(ClientMessage::Req {
                subscription_id: "sub".to_string(),
                filters: vec![json!({"kinds":[super::NIP43_INVITE_KIND]})],
            })
            .await;
        assert!(matches!(
            response[0],
            ServerMessage::Closed {
                ref message, ..
            } if message.contains("invalid pubkey")
        ));
    }

    #[tokio::test]
    async fn req_invite_generation_surfaces_membership_lookup_and_insert_failures() {
        let tenant_id = "tenant-1";
        let req = ClientMessage::Req {
            subscription_id: "sub".to_string(),
            filters: vec![json!({"kinds":[super::NIP43_INVITE_KIND]})],
        };
        let relay_secret = SecretKey::from_slice(&[0x5a; 32]).expect("secret");
        let secp = Secp256k1::new();
        let relay_keypair = Keypair::from_secret_key(&secp, &relay_secret);
        let (relay_pubkey, _) = secp256k1::XOnlyPublicKey::from_keypair(&relay_keypair);

        let membership_lookup_error = Arc::new(ScriptedMembership::new("membership_by_pubkey"));
        let mut lookup_session =
            Session::with_policy_and_auth(MemoryStore::new(), Policy::default(), true)
                .with_membership(Some(tenant_id.to_string()), Some(membership_lookup_error))
                .with_relay_signer(
                    relay_pubkey.serialize().to_vec(),
                    relay_secret.secret_bytes().to_vec(),
                );
        authenticate_session(&mut lookup_session).await;
        let response = lookup_session.handle_message(req.clone()).await;
        assert!(matches!(
            response[0],
            ServerMessage::Closed {
                ref message, ..
            } if message.contains("membership_by_pubkey failure")
        ));

        let insert_error = Arc::new(ScriptedMembership::new("insert_invite"));
        let mut insert_session =
            Session::with_policy_and_auth(MemoryStore::new(), Policy::default(), true)
                .with_membership(Some(tenant_id.to_string()), Some(insert_error.clone()))
                .with_relay_signer(
                    relay_pubkey.serialize().to_vec(),
                    relay_secret.secret_bytes().to_vec(),
                );
        authenticate_session(&mut insert_session).await;
        let auth_pubkey = insert_session
            .auth
            .as_ref()
            .and_then(|auth| auth.authenticated_pubkey.as_ref())
            .cloned()
            .expect("auth pubkey");
        insert_error
            .inner
            .upsert_membership(RelayMembershipRecord {
                tenant_id: tenant_id.to_string(),
                pubkey: hex::decode(auth_pubkey).expect("pubkey"),
                role: "member".to_string(),
                status: super::MEMBERSHIP_STATUS_ACTIVE.to_string(),
                created_at: 1,
                updated_at: 1,
            })
            .await
            .expect("seed membership");
        let response = insert_session.handle_message(req).await;
        assert!(matches!(
            response[0],
            ServerMessage::Closed {
                ref message, ..
            } if message.contains("insert_invite failure")
        ));
    }

    #[tokio::test]
    async fn req_membership_list_event_surfaces_list_memberships_failure() {
        let membership = Arc::new(ScriptedMembership::new("list_memberships"));
        let relay_secret = SecretKey::from_slice(&[0x42; 32]).expect("secret");
        let secp = Secp256k1::new();
        let relay_keypair = Keypair::from_secret_key(&secp, &relay_secret);
        let (relay_pubkey, _) = secp256k1::XOnlyPublicKey::from_keypair(&relay_keypair);
        let mut session = Session::new(MemoryStore::new())
            .with_membership(Some("tenant-1".to_string()), Some(membership))
            .with_relay_signer(
                relay_pubkey.serialize().to_vec(),
                relay_secret.secret_bytes().to_vec(),
            );
        let response = session
            .handle_message(ClientMessage::Req {
                subscription_id: "sub".to_string(),
                filters: vec![json!({"kinds":[super::NIP43_MEMBERSHIP_KIND]})],
            })
            .await;
        assert!(matches!(
            response[0],
            ServerMessage::Closed {
                ref message, ..
            } if message.contains("list_memberships failure")
        ));
    }

    #[tokio::test]
    async fn scripted_membership_remove_membership_passthrough_returns_false() {
        let membership = ScriptedMembership::new("none");
        let removed = membership
            .remove_membership("tenant-1", &[1, 2, 3])
            .await
            .expect("remove");
        assert!(!removed);
    }

    #[tokio::test]
    async fn req_invite_virtual_event_guards_require_full_context_and_active_member() {
        let req = ClientMessage::Req {
            subscription_id: "sub".to_string(),
            filters: vec![json!({"kinds":[super::NIP43_INVITE_KIND]})],
        };

        let mut no_membership = Session::new(MemoryStore::new());
        let response = no_membership.handle_message(req.clone()).await;
        assert!(matches!(
            response[0],
            ServerMessage::Closed {
                ref message, ..
            } if message.contains("relay does not support invites")
        ));

        let membership = Arc::new(InMemoryRepositories::new());
        let mut no_tenant =
            Session::new(MemoryStore::new()).with_membership(None, Some(membership.clone()));
        let response = no_tenant.handle_message(req.clone()).await;
        assert!(matches!(
            response[0],
            ServerMessage::Closed {
                ref message, ..
            } if message.contains("relay does not support invites")
        ));

        let mut no_signer = Session::new(MemoryStore::new())
            .with_membership(Some("tenant-1".to_string()), Some(membership.clone()));
        let response = no_signer.handle_message(req.clone()).await;
        assert!(matches!(
            response[0],
            ServerMessage::Closed {
                ref message, ..
            } if message.contains("relay does not support invites")
        ));

        let relay_secret = SecretKey::from_slice(&[0x33; 32]).expect("secret");
        let secp = Secp256k1::new();
        let relay_keypair = Keypair::from_secret_key(&secp, &relay_secret);
        let (relay_pubkey, _) = secp256k1::XOnlyPublicKey::from_keypair(&relay_keypair);
        let mut no_auth = Session::new(MemoryStore::new())
            .with_membership(Some("tenant-1".to_string()), Some(membership.clone()))
            .with_relay_signer(
                relay_pubkey.serialize().to_vec(),
                relay_secret.secret_bytes().to_vec(),
            );
        let response = no_auth.handle_message(req.clone()).await;
        assert!(matches!(
            response[0],
            ServerMessage::Closed {
                ref message, ..
            } if message == super::AUTH_REQUIRED_REASON
        ));

        let mut no_member =
            Session::with_policy_and_auth(MemoryStore::new(), Policy::default(), true)
                .with_membership(Some("tenant-1".to_string()), Some(membership.clone()))
                .with_relay_signer(
                    relay_pubkey.serialize().to_vec(),
                    relay_secret.secret_bytes().to_vec(),
                );
        authenticate_session(&mut no_member).await;
        let response = no_member.handle_message(req.clone()).await;
        assert!(matches!(
            response[0],
            ServerMessage::Closed {
                ref message, ..
            } if message.contains("membership required")
        ));

        let auth_pubkey = no_member
            .auth
            .as_ref()
            .and_then(|auth| auth.authenticated_pubkey.as_ref())
            .cloned()
            .expect("auth pubkey");
        membership
            .upsert_membership(RelayMembershipRecord {
                tenant_id: "tenant-1".to_string(),
                pubkey: hex::decode(auth_pubkey).expect("pubkey bytes"),
                role: "member".to_string(),
                status: super::MEMBERSHIP_STATUS_LEFT.to_string(),
                created_at: 1,
                updated_at: 1,
            })
            .await
            .expect("insert membership");
        let response = no_member.handle_message(req).await;
        assert!(matches!(
            response[0],
            ServerMessage::Closed {
                ref message, ..
            } if message.contains("membership required")
        ));
    }

    #[tokio::test]
    async fn req_membership_virtual_event_guards_require_full_context() {
        let req = ClientMessage::Req {
            subscription_id: "sub".to_string(),
            filters: vec![json!({"kinds":[super::NIP43_MEMBERSHIP_KIND]})],
        };

        let mut no_membership = Session::new(MemoryStore::new());
        let response = no_membership.handle_message(req.clone()).await;
        assert_eq!(response.len(), 1);
        assert_eose(&response[0]);

        let membership = Arc::new(InMemoryRepositories::new());
        let mut no_tenant =
            Session::new(MemoryStore::new()).with_membership(None, Some(membership.clone()));
        let response = no_tenant.handle_message(req.clone()).await;
        assert_eq!(response.len(), 1);
        assert_eose(&response[0]);

        let mut no_signer = Session::new(MemoryStore::new())
            .with_membership(Some("tenant-1".to_string()), Some(membership));
        let response = no_signer.handle_message(req).await;
        assert_eq!(response.len(), 1);
        assert_eose(&response[0]);
    }

    #[tokio::test]
    async fn admission_requires_related_events_rejects_when_missing() {
        let mut filter = EventFilter::new();
        filter.ids = vec!["missing".to_string()];
        filter.limit = Some(1);

        let admission = StubAdmission {
            decision: AdmissionDecision::RequiresRelatedEvents {
                filters: vec![filter],
            },
        };
        let mut session = Session::with_admission(MemoryStore::new(), Arc::new(admission));
        let event = signed_event("needs-related");
        let response = session
            .handle_message(ClientMessage::Event(
                serde_json::to_value(event).expect("event"),
            ))
            .await;
        assert!(matches!(
            response[0],
            ServerMessage::Ok {
                accepted: false,
                ref message,
                ..
            } if message == "missing related events"
        ));
    }

    #[tokio::test]
    async fn admission_requires_related_events_surfaces_store_query_error() {
        let mut filter = EventFilter::new();
        filter.ids = vec!["missing".to_string()];
        filter.limit = Some(1);
        let admission = StubAdmission {
            decision: AdmissionDecision::RequiresRelatedEvents {
                filters: vec![filter],
            },
        };
        let mut session = Session::with_admission(
            scripted_store_dyn(ScriptedStore::query_error()),
            Arc::new(admission),
        );
        let response = session
            .handle_message(ClientMessage::Event(
                serde_json::to_value(signed_event("needs-related-error")).expect("event"),
            ))
            .await;
        assert_notice(&response[0]);
    }

    #[test]
    fn relay_host_and_filter_matching_helpers_cover_edges() {
        assert_eq!(
            super::relay_host_from_url("https://Relay.Example:443"),
            Some("relay.example".to_string())
        );
        assert_eq!(super::relay_host_from_url(""), None);
        assert_eq!(super::relay_host_from_url("wss:///"), None);
        assert_eq!(
            super::relay_host_from_url("https://[::1]:443"),
            Some("::1".to_string())
        );
        assert_eq!(
            super::relay_host_from_url("https://[::1"),
            Some("[".to_string())
        );

        let event = signed_event_with_tags("bad-tags", 1, vec![Vec::new()]);
        let filters = vec![crate::Filter {
            ids: Vec::new(),
            authors: Vec::new(),
            kinds: vec![1],
            since: None,
            until: None,
            limit: None,
            tags: std::collections::BTreeMap::new(),
        }];
        assert!(!super::event_matches_filters(&event, &filters));
        assert!(super::event_matches_filters(
            &signed_event("empty-filters"),
            &[]
        ));
    }

    #[test]
    fn tag_and_filter_limit_helpers_cover_edges() {
        let tags = vec![
            vec!["claim".to_string(), "invite-code".to_string()],
            vec!["relay".to_string()],
            Vec::new(),
        ];
        assert_eq!(
            super::find_tag_value(&tags, "claim"),
            Some("invite-code".to_string())
        );
        assert_eq!(super::find_tag_value(&tags, "relay"), None);
        assert_eq!(super::find_tag_value(&tags, "missing"), None);
        assert!(super::has_tag(&tags, "claim"));
        assert!(super::has_tag(&tags, "relay"));
        assert!(!super::has_tag(&tags, "missing"));

        let mut policy = Policy::default();
        policy.max_limit = Some(5);
        let session = Session::with_policy(MemoryStore::new(), policy);
        let filter = crate::Filter {
            ids: Vec::new(),
            authors: Vec::new(),
            kinds: Vec::new(),
            since: None,
            until: None,
            limit: Some(5),
            tags: std::collections::BTreeMap::new(),
        };
        assert!(session.validate_filter_limits(&[filter]).is_none());
        assert!(session.validate_filter_limits(&[]).is_none());
    }

    #[tokio::test]
    async fn session_arc_store_helper_paths_cover_edge_cases() {
        let store: Arc<dyn EventStore> = Arc::new(MemoryStore::new());
        let mut session = Session::new(store);
        assert!(!session.is_authenticated());
        assert_eq!(session.authenticated_pubkey(), None);
        let membership_err = session
            .require_membership()
            .await
            .expect_err("membership required");
        assert!(membership_err.contains("relay does not support membership"));

        let event = signed_event("arc-dispatch");
        assert!(session.dispatch_event(&event).is_empty());
        let virtual_events = session
            .virtual_events(&[], 1)
            .await
            .expect("virtual events");
        assert!(virtual_events.is_empty());
        assert!(
            session
                .build_membership_list_event(1)
                .await
                .expect("membership event")
                .is_none()
        );
        let invite_err = session
            .build_invite_event(1)
            .await
            .expect_err("invite unsupported");
        assert!(invite_err.contains("relay does not support invites"));

        let pubkey = "aa".repeat(32);
        session.auth = Some(super::AuthState {
            challenge: "challenge".to_string(),
            authenticated_pubkey: Some(pubkey.clone()),
        });
        assert!(session.is_authenticated());
        assert_eq!(session.authenticated_pubkey(), Some(pubkey.as_str()));
        let auth_responses = session.handle_auth(json!({"bad": true})).await;
        let first = auth_responses.first().expect("auth notice");
        assert_notice(first);

        session.apply_retention(10).await.expect("retention");
        let mut policy = Policy::default();
        policy.max_limit = Some(1);
        session.policy = policy;
        let filter = crate::Filter {
            ids: Vec::new(),
            authors: Vec::new(),
            kinds: Vec::new(),
            since: None,
            until: None,
            limit: Some(2),
            tags: std::collections::BTreeMap::new(),
        };
        assert!(session.validate_filter_limits(&[filter]).is_some());
    }

    #[tokio::test]
    async fn session_arc_store_executes_dispatch_and_membership_join_paths() {
        let store: Arc<dyn EventStore> = Arc::new(MemoryStore::new());
        let membership = Arc::new(InMemoryRepositories::new());
        let tenant_id = "tenant-arc";
        let mut session = Session::new(store)
            .with_membership(Some(tenant_id.to_string()), Some(membership.clone()));

        let _ = session
            .handle_message(ClientMessage::Req {
                subscription_id: "sub".to_string(),
                filters: vec![json!({"kinds": [1]})],
            })
            .await;
        let dispatch_responses = session.dispatch_event(&signed_event("arc-dispatch-hit"));
        assert!(
            dispatch_responses
                .iter()
                .any(|response| matches!(response, ServerMessage::Event { .. }))
        );

        let member_pubkey_hex = signed_event("arc-member").pubkey;
        let member_pubkey = hex::decode(&member_pubkey_hex).expect("pubkey");
        membership
            .upsert_membership(RelayMembershipRecord {
                tenant_id: tenant_id.to_string(),
                pubkey: member_pubkey.clone(),
                role: "member".to_string(),
                status: "pending".to_string(),
                created_at: 1,
                updated_at: 1,
            })
            .await
            .expect("membership insert");
        membership
            .insert_invite(
                RelayInviteRecord::new(
                    tenant_id,
                    "invite-arc",
                    "member",
                    member_pubkey_hex.as_str(),
                    Some(member_pubkey_hex.as_str()),
                    None,
                    1,
                )
                .expect("invite"),
            )
            .await
            .expect("invite insert");

        let join_event = signed_event_with_tags(
            "arc-join",
            super::NIP43_JOIN_KIND,
            vec![
                vec!["-".to_string()],
                vec!["claim".to_string(), "invite-arc".to_string()],
            ],
        );
        let join_responses = session
            .handle_membership_event(&join_event, 10)
            .await
            .expect("membership response");
        assert!(matches!(
            join_responses.first(),
            Some(ServerMessage::Ok { accepted: true, .. })
        ));
    }

    #[tokio::test]
    async fn session_arc_store_executes_auth_invite_and_leave_paths() {
        let store: Arc<dyn EventStore> = Arc::new(MemoryStore::new());
        let membership = Arc::new(InMemoryRepositories::new());
        let tenant_id = "tenant-arc-auth";

        let secp = Secp256k1::new();
        let relay_secret = SecretKey::from_slice(&[0x66; 32]).expect("relay secret");
        let relay_keypair = Keypair::from_secret_key(&secp, &relay_secret);
        let (relay_pubkey, _) = secp256k1::XOnlyPublicKey::from_keypair(&relay_keypair);

        let member_pubkey_hex = signed_event("arc-member-auth").pubkey;
        let member_pubkey = hex::decode(&member_pubkey_hex).expect("member pubkey");
        membership
            .upsert_membership(RelayMembershipRecord {
                tenant_id: tenant_id.to_string(),
                pubkey: member_pubkey,
                role: "member".to_string(),
                status: super::MEMBERSHIP_STATUS_ACTIVE.to_string(),
                created_at: 1,
                updated_at: 1,
            })
            .await
            .expect("membership insert");

        let mut session = Session::with_policy_and_auth(store, Policy::default(), true)
            .with_membership(Some(tenant_id.to_string()), Some(membership.clone()))
            .with_membership_requirements(true, true)
            .with_relay_signer(
                relay_pubkey.serialize().to_vec(),
                relay_secret.secret_bytes().to_vec(),
            )
            .with_relay_url(Some("wss://relay.example".to_string()));

        authenticate_session(&mut session).await;
        session
            .require_membership()
            .await
            .expect("membership required");

        let invite_responses = session
            .handle_message(ClientMessage::Req {
                subscription_id: "arc-invite".to_string(),
                filters: vec![json!({"kinds": [super::NIP43_INVITE_KIND]})],
            })
            .await;
        assert!(
            invite_responses
                .iter()
                .any(|response| matches!(response, ServerMessage::Event { .. }))
        );

        let leave_event = signed_event_with_tags(
            "arc-leave",
            super::NIP43_LEAVE_KIND,
            vec![vec!["-".to_string()]],
        );
        let leave_responses = session
            .handle_membership_event(&leave_event, 30)
            .await
            .expect("leave response");
        assert!(matches!(
            leave_responses.first(),
            Some(ServerMessage::Ok { accepted: true, .. })
        ));
    }

    #[tokio::test]
    async fn session_arc_store_exercises_core_message_paths() {
        let store: Arc<dyn EventStore> = Arc::new(MemoryStore::new());
        let membership = Arc::new(InMemoryRepositories::new());
        let tenant_id = "tenant-arc-core";

        let secp = Secp256k1::new();
        let relay_secret = SecretKey::from_slice(&[0x77; 32]).expect("relay secret");
        let relay_keypair = Keypair::from_secret_key(&secp, &relay_secret);
        let (relay_pubkey, _) = secp256k1::XOnlyPublicKey::from_keypair(&relay_keypair);

        let mut session = Session::with_policy_and_auth(store, Policy::default(), true)
            .with_membership(Some(tenant_id.to_string()), Some(membership.clone()))
            .with_membership_requirements(true, true)
            .with_relay_signer(
                relay_pubkey.serialize().to_vec(),
                relay_secret.secret_bytes().to_vec(),
            )
            .with_relay_url(Some("wss://relay.example".to_string()));

        let initial = session.initial_messages();
        assert_eq!(initial.len(), 1);
        assert_auth(&initial[0]);

        let raw_invalid = session.handle_raw("not-json").await;
        assert_eq!(raw_invalid.len(), 1);
        assert_notice(&raw_invalid[0]);

        let req_before_auth = session
            .handle_message(ClientMessage::Req {
                subscription_id: "arc-pre-auth".to_string(),
                filters: vec![json!({})],
            })
            .await;
        assert_eq!(req_before_auth.len(), 1);
        assert_eq!(
            closed_reason(&req_before_auth[0]),
            super::AUTH_REQUIRED_REASON
        );

        let bad_auth = session
            .handle_message(ClientMessage::Auth(json!({"kind": 1})))
            .await;
        assert_eq!(bad_auth.len(), 1);
        assert_notice(&bad_auth[0]);

        authenticate_session(&mut session).await;
        let auth_pubkey_hex = session
            .authenticated_pubkey()
            .expect("authenticated pubkey")
            .to_string();
        let auth_pubkey = hex::decode(&auth_pubkey_hex).expect("auth pubkey");
        membership
            .upsert_membership(RelayMembershipRecord {
                tenant_id: tenant_id.to_string(),
                pubkey: auth_pubkey,
                role: "member".to_string(),
                status: super::MEMBERSHIP_STATUS_ACTIVE.to_string(),
                created_at: 1,
                updated_at: 1,
            })
            .await
            .expect("membership insert");

        let req = session
            .handle_message(ClientMessage::Req {
                subscription_id: "arc-sub".to_string(),
                filters: vec![json!({})],
            })
            .await;
        assert!(req.iter().any(|message| matches!(
            message,
            ServerMessage::Eose { subscription_id } if subscription_id == "arc-sub"
        )));

        let count = session
            .handle_message(ClientMessage::Count {
                subscription_id: "arc-count".to_string(),
                filters: vec![json!({})],
            })
            .await;
        assert!(matches!(
            count.first(),
            Some(ServerMessage::Count {
                subscription_id,
                ..
            }) if subscription_id == "arc-count"
        ));

        let membership_virtual = session
            .handle_message(ClientMessage::Req {
                subscription_id: "arc-virtual".to_string(),
                filters: vec![json!({
                    "kinds": [super::NIP43_MEMBERSHIP_KIND, super::NIP43_INVITE_KIND]
                })],
            })
            .await;
        assert!(
            membership_virtual
                .iter()
                .any(|message| matches!(message, ServerMessage::Event { .. }))
        );

        let valid_event = session
            .handle_message(ClientMessage::Event(
                serde_json::to_value(signed_event("arc-core-valid")).expect("event json"),
            ))
            .await;
        assert!(matches!(
            valid_event.first(),
            Some(ServerMessage::Ok { accepted: true, .. })
        ));

        let auth_kind_event = signed_event_with_tags(
            "arc-core-auth-kind",
            super::AUTH_KIND,
            vec![
                vec!["challenge".to_string(), "wrong".to_string()],
                vec!["relay".to_string(), "wss://relay.example".to_string()],
            ],
        );
        let auth_kind = session
            .handle_message(ClientMessage::Event(
                serde_json::to_value(auth_kind_event).expect("event json"),
            ))
            .await;
        assert_eq!(auth_kind.len(), 1);
        assert_ok_rejected(&auth_kind[0]);

        let mut invalid_tags_event = signed_event("arc-core-invalid-tags");
        invalid_tags_event.tags = vec![Vec::new()];
        let dispatch_invalid = session.dispatch_event(&invalid_tags_event);
        assert_eq!(dispatch_invalid.len(), 1);
        assert_notice(&dispatch_invalid[0]);

        let close = session
            .handle_message(ClientMessage::Close {
                subscription_id: "arc-sub".to_string(),
            })
            .await;
        assert!(close.is_empty());
    }

    #[tokio::test]
    async fn req_generates_invite_claim_when_member() {
        let membership = Arc::new(InMemoryRepositories::new());
        let tenant_id = "tenant-1";

        let secp = Secp256k1::new();
        let relay_secret = SecretKey::from_slice(&[0x44; 32]).expect("relay secret");
        let relay_keypair = Keypair::from_secret_key(&secp, &relay_secret);
        let (relay_pubkey, _) = secp256k1::XOnlyPublicKey::from_keypair(&relay_keypair);

        let auth_event = signed_event_with_tags(
            "auth",
            super::AUTH_KIND,
            vec![vec!["challenge".to_string(), "placeholder".to_string()]],
        );
        let member_pubkey = hex::decode(&auth_event.pubkey).expect("pubkey");
        let record = RelayMembershipRecord {
            tenant_id: tenant_id.to_string(),
            pubkey: member_pubkey,
            role: "member".to_string(),
            status: super::MEMBERSHIP_STATUS_ACTIVE.to_string(),
            created_at: 1,
            updated_at: 1,
        };
        membership
            .upsert_membership(record)
            .await
            .expect("membership insert");

        let mut session =
            Session::with_policy_and_auth(MemoryStore::new(), Policy::default(), true)
                .with_membership(Some(tenant_id.to_string()), Some(membership.clone()))
                .with_relay_signer(
                    relay_pubkey.serialize().to_vec(),
                    relay_secret.secret_bytes().to_vec(),
                );
        let challenge = session.auth_challenge().expect("challenge");
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        let auth_event = signed_event_with_tags_at(
            "auth",
            super::AUTH_KIND,
            vec![
                vec!["challenge".to_string(), challenge],
                vec!["relay".to_string(), "wss://relay.example".to_string()],
            ],
            now,
        );
        let _ = session
            .handle_message(ClientMessage::Auth(
                serde_json::to_value(auth_event).unwrap(),
            ))
            .await;

        let responses = session
            .handle_message(ClientMessage::Req {
                subscription_id: "sub".to_string(),
                filters: vec![json!({"kinds": [super::NIP43_INVITE_KIND]})],
            })
            .await;

        let events = responses
            .iter()
            .filter_map(|response| match response {
                ServerMessage::Event { event, .. } => Some(event),
                _ => None,
            })
            .collect::<Vec<_>>();
        let event = events.first().copied().expect("invite event");
        let tags = event
            .get("tags")
            .and_then(|tags| tags.as_array())
            .expect("tags");
        let claim = tags.iter().find_map(|tag| {
            if tag.get(0).and_then(|value| value.as_str()) == Some("claim") {
                tag.get(1).and_then(|value| value.as_str())
            } else {
                None
            }
        });
        let claim = claim.expect("claim");

        let invite = membership
            .invite_by_code(tenant_id, claim)
            .await
            .expect("invite lookup");
        assert!(invite.is_some());
    }

    #[tokio::test]
    async fn virtual_events_include_membership_and_invite_when_filters_match() {
        let membership = Arc::new(InMemoryRepositories::new());
        let tenant_id = "tenant-1";

        let secp = Secp256k1::new();
        let relay_secret = SecretKey::from_slice(&[0x45; 32]).expect("relay secret");
        let relay_keypair = Keypair::from_secret_key(&secp, &relay_secret);
        let (relay_pubkey, _) = secp256k1::XOnlyPublicKey::from_keypair(&relay_keypair);

        let auth_event = signed_event_with_tags(
            "auth-member",
            super::AUTH_KIND,
            vec![vec!["challenge".to_string(), "placeholder".to_string()]],
        );
        let member_pubkey = hex::decode(&auth_event.pubkey).expect("member pubkey");
        membership
            .upsert_membership(RelayMembershipRecord {
                tenant_id: tenant_id.to_string(),
                pubkey: member_pubkey,
                role: "member".to_string(),
                status: super::MEMBERSHIP_STATUS_ACTIVE.to_string(),
                created_at: 1,
                updated_at: 1,
            })
            .await
            .expect("membership insert");

        let mut session =
            Session::with_policy_and_auth(MemoryStore::new(), Policy::default(), true)
                .with_membership(Some(tenant_id.to_string()), Some(membership))
                .with_relay_signer(
                    relay_pubkey.serialize().to_vec(),
                    relay_secret.secret_bytes().to_vec(),
                );
        authenticate_session(&mut session).await;

        let filters = vec![crate::Filter {
            ids: Vec::new(),
            authors: Vec::new(),
            kinds: vec![super::NIP43_MEMBERSHIP_KIND, super::NIP43_INVITE_KIND],
            since: None,
            until: None,
            limit: None,
            tags: std::collections::BTreeMap::new(),
        }];
        let events = session
            .virtual_events(&filters, 10)
            .await
            .expect("virtual events");

        assert_eq!(events.len(), 2);
        assert!(
            events
                .iter()
                .any(|event| event.kind == super::NIP43_MEMBERSHIP_KIND)
        );
        assert!(
            events
                .iter()
                .any(|event| event.kind == super::NIP43_INVITE_KIND)
        );
    }

    #[tokio::test]
    async fn virtual_events_skip_membership_and_invite_when_filters_do_not_match() {
        let membership = Arc::new(InMemoryRepositories::new());
        let tenant_id = "tenant-1";

        let secp = Secp256k1::new();
        let relay_secret = SecretKey::from_slice(&[0x46; 32]).expect("relay secret");
        let relay_keypair = Keypair::from_secret_key(&secp, &relay_secret);
        let (relay_pubkey, _) = secp256k1::XOnlyPublicKey::from_keypair(&relay_keypair);

        let auth_event = signed_event_with_tags(
            "auth-member-mismatch",
            super::AUTH_KIND,
            vec![vec!["challenge".to_string(), "placeholder".to_string()]],
        );
        let member_pubkey = hex::decode(&auth_event.pubkey).expect("member pubkey");
        membership
            .upsert_membership(RelayMembershipRecord {
                tenant_id: tenant_id.to_string(),
                pubkey: member_pubkey,
                role: "member".to_string(),
                status: super::MEMBERSHIP_STATUS_ACTIVE.to_string(),
                created_at: 1,
                updated_at: 1,
            })
            .await
            .expect("membership insert");

        let mut session =
            Session::with_policy_and_auth(MemoryStore::new(), Policy::default(), true)
                .with_membership(Some(tenant_id.to_string()), Some(membership))
                .with_relay_signer(
                    relay_pubkey.serialize().to_vec(),
                    relay_secret.secret_bytes().to_vec(),
                );
        authenticate_session(&mut session).await;

        let filters = vec![crate::Filter {
            ids: Vec::new(),
            authors: vec!["22".repeat(32)],
            kinds: vec![super::NIP43_MEMBERSHIP_KIND, super::NIP43_INVITE_KIND],
            since: None,
            until: None,
            limit: None,
            tags: std::collections::BTreeMap::new(),
        }];
        let events = session
            .virtual_events(&filters, 10)
            .await
            .expect("virtual events");
        assert!(events.is_empty());
    }

    #[tokio::test]
    async fn req_rejects_non_member_when_membership_required() {
        let membership = Arc::new(InMemoryRepositories::new());
        let tenant_id = "tenant-1";

        let mut session =
            Session::with_policy_and_auth(MemoryStore::new(), Policy::default(), true)
                .with_membership(Some(tenant_id.to_string()), Some(membership))
                .with_membership_requirements(true, false);
        let challenge = session.auth_challenge().expect("challenge");
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        let auth_event = signed_event_with_tags_at(
            "auth",
            super::AUTH_KIND,
            vec![
                vec!["challenge".to_string(), challenge],
                vec!["relay".to_string(), "wss://relay.example".to_string()],
            ],
            now,
        );
        let _ = session
            .handle_message(ClientMessage::Auth(
                serde_json::to_value(auth_event).unwrap(),
            ))
            .await;

        let response = session
            .handle_message(ClientMessage::Req {
                subscription_id: "sub".to_string(),
                filters: vec![json!({})],
            })
            .await;

        assert!(matches!(
            response[0],
            ServerMessage::Closed {
                ref message, ..
            } if message.starts_with(super::RESTRICTED_PREFIX)
        ));
    }

    #[tokio::test]
    async fn event_rejects_non_member_when_membership_required() {
        let membership = Arc::new(InMemoryRepositories::new());
        let tenant_id = "tenant-1";

        let mut session =
            Session::with_policy_and_auth(MemoryStore::new(), Policy::default(), true)
                .with_membership(Some(tenant_id.to_string()), Some(membership))
                .with_membership_requirements(false, true);
        let challenge = session.auth_challenge().expect("challenge");
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        let auth_event = signed_event_with_tags_at(
            "auth",
            super::AUTH_KIND,
            vec![
                vec!["challenge".to_string(), challenge],
                vec!["relay".to_string(), "wss://relay.example".to_string()],
            ],
            now,
        );
        let _ = session
            .handle_message(ClientMessage::Auth(
                serde_json::to_value(auth_event).unwrap(),
            ))
            .await;

        let event = signed_event("seed");
        let response = session
            .handle_message(ClientMessage::Event(serde_json::to_value(event).unwrap()))
            .await;

        assert!(matches!(
            response[0],
            ServerMessage::Ok {
                accepted: false,
                ref message,
                ..
            } if message.starts_with(super::RESTRICTED_PREFIX)
        ));
    }

    #[tokio::test]
    async fn auth_required_rejects_req_without_auth() {
        let store = MemoryStore::new();
        let (tx, _) = tokio::sync::broadcast::channel(4);
        let mut session = Session::with_broadcast(store, Policy::default(), None, tx, true, false);
        let response = session
            .handle_message(ClientMessage::Req {
                subscription_id: "sub".to_string(),
                filters: vec![serde_json::json!({})],
            })
            .await;
        assert!(matches!(
            response[0],
            ServerMessage::Closed {
                ref message,
                ..
            } if message == super::AUTH_REQUIRED_REASON
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
        assert_ok_rejected(&response[0]);
    }

    #[tokio::test]
    async fn admission_accepts_event() {
        let admission = StubAdmission {
            decision: AdmissionDecision::Accept,
        };
        let mut session = Session::with_admission(MemoryStore::new(), Arc::new(admission));
        let response = session
            .handle_message(ClientMessage::Event(
                serde_json::to_value(signed_event("admission-accept")).expect("event"),
            ))
            .await;
        assert!(matches!(
            response[0],
            ServerMessage::Ok {
                accepted: true,
                ref message,
                ..
            } if message == "saved"
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
            decision: AdmissionDecision::RequiresRelatedEvents {
                filters: vec![filter],
            },
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
        assert_event(&responses[0]);
    }

    #[tokio::test]
    async fn dispatch_event_does_not_emit_for_non_matching_filters() {
        let store = MemoryStore::new();
        let mut session = Session::new(store);
        session
            .handle_message(ClientMessage::Req {
                subscription_id: "sub".to_string(),
                filters: vec![json!({"kinds": [999_999]})],
            })
            .await;

        let responses = session.dispatch_event(&signed_event("seed-no-match"));
        assert!(responses.is_empty());
    }

    #[tokio::test]
    async fn dispatch_event_skips_subscriptions_with_empty_filter_sets() {
        let store = MemoryStore::new();
        let mut session = Session::new(store);
        session
            .handle_message(ClientMessage::Req {
                subscription_id: "sub".to_string(),
                filters: Vec::new(),
            })
            .await;
        let responses = session.dispatch_event(&signed_event("seed-empty"));
        assert!(responses.is_empty());
    }

    #[tokio::test]
    async fn dispatch_event_reports_invalid_tags() {
        let store = MemoryStore::new();
        let mut session = Session::new(store);
        session
            .handle_message(ClientMessage::Req {
                subscription_id: "sub".to_string(),
                filters: vec![json!({})],
            })
            .await;

        let mut event = signed_event("seed-invalid-tags");
        event.tags = vec![Vec::new()];
        let responses = session.dispatch_event(&event);
        assert_eq!(responses.len(), 1);
        assert_notice(&responses[0]);
    }

    #[tokio::test]
    async fn broadcast_sends_inserted_events() {
        let store = MemoryStore::new();
        let (tx, mut rx) = tokio::sync::broadcast::channel(8);
        let mut session = Session::with_broadcast(store, Policy::default(), None, tx, false, false);
        let event = signed_event("seed");

        let response = session
            .handle_message(ClientMessage::Event(
                serde_json::to_value(event.clone()).unwrap(),
            ))
            .await;
        assert!(matches!(
            response[0],
            ServerMessage::Ok { accepted: true, .. }
        ));

        let received = rx.recv().await.expect("broadcast");
        assert_eq!(received.id, event.id);
    }
}
