use crate::{
    AdmissionDecider, ClientMessage, EventStore, Filter, Notice, Policy, RelayEvent,
    ServerMessage, StoreOutcome, SubscriptionId, SubscriptionRegistry, decode_client_message,
};
use gittree_core::AdmissionDecision;
use serde_json::Value;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

pub struct Session<S: EventStore> {
    registry: SubscriptionRegistry,
    store: S,
    policy: Policy,
    admission: Option<Arc<dyn AdmissionDecider>>,
}

impl<S: EventStore> Session<S> {
    pub fn new(store: S) -> Self {
        Self {
            registry: SubscriptionRegistry::default(),
            store,
            policy: Policy::default(),
            admission: None,
        }
    }

    pub fn with_policy(store: S, policy: Policy) -> Self {
        Self {
            registry: SubscriptionRegistry::default(),
            store,
            policy,
            admission: None,
        }
    }

    pub fn with_admission(store: S, admission: Arc<dyn AdmissionDecider>) -> Self {
        Self {
            registry: SubscriptionRegistry::default(),
            store,
            policy: Policy::default(),
            admission: Some(admission),
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
        }
    }

    pub fn registry(&self) -> &SubscriptionRegistry {
        &self.registry
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
                let parsed = match parse_filters(&filters) {
                    Ok(filters) => filters,
                    Err(err) => return vec![Notice::from(err).into()],
                };
                let events = match self.store.query(&parsed).await {
                    Ok(events) => events,
                    Err(err) => return vec![Notice::from(err).into()],
                };

                let sub_id = SubscriptionId::new(subscription_id.clone());
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
                self.registry.remove(&SubscriptionId::new(subscription_id));
                Vec::new()
            }
            ClientMessage::Event(value) => self.handle_event(value).await,
            ClientMessage::Auth(_) | ClientMessage::Count { .. } => Vec::new(),
        }
    }

    async fn handle_event(&mut self, value: Value) -> Vec<ServerMessage> {
        let event: crate::NostrEvent = match serde_json::from_value(value) {
            Ok(event) => event,
            Err(_) => return vec![Notice::message("invalid event").into()],
        };

        if let Err(err) = event.verify() {
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
            return vec![ServerMessage::Ok {
                event_id: event.id.clone(),
                accepted: false,
                message: err.to_string(),
            }];
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
                            Err(err) => return vec![Notice::from(err).into()],
                        };
                        if related.is_empty() {
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
            Err(err) => return vec![Notice::from(err).into()],
        };

        let mut responses = Vec::new();
        let message = match outcome {
            StoreOutcome::Inserted => "saved".to_string(),
            StoreOutcome::Duplicate => "duplicate".to_string(),
        };
        responses.push(ServerMessage::Ok {
            event_id: event.id.clone(),
            accepted: true,
            message,
        });

        let event_value = match serde_json::to_value(event.clone()) {
            Ok(value) => value,
            Err(_) => return vec![Notice::message("failed to serialize event").into()],
        };

        let tags = match crate::TagIndex::from_tags(&event.tags) {
            Ok(tags) => tags,
            Err(_) => return vec![Notice::message("invalid event tags").into()],
        };

        for (sub_id, filters) in self.registry.iter() {
            if filters.is_empty() {
                continue;
            }
            let matches = filters.iter().any(|filter| filter.matches(&event, &tags));
            if matches {
                responses.push(ServerMessage::Event {
                    subscription_id: sub_id.as_str().to_string(),
                    event: event_value.clone(),
                });
            }
        }

        responses
    }
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
        AdmissionDecider, ClientMessage, EventStore, MemoryStore, NostrEvent, ServerMessage,
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
        let secp = Secp256k1::new();
        let secret_key = SecretKey::from_slice(&[0x11; 32]).expect("secret");
        let keypair = Keypair::from_secret_key(&secp, &secret_key);
        let (pubkey, _) = secp256k1::XOnlyPublicKey::from_keypair(&keypair);

        let mut event = NostrEvent {
            id: seed.to_string(),
            pubkey: hex::encode(pubkey.serialize()),
            created_at: 1,
            kind: 1,
            tags: Vec::new(),
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
}
