use crate::{
    ClientMessage, EventStore, Filter, Notice, ServerMessage, SubscriptionId,
    SubscriptionRegistry, decode_client_message,
};
use serde_json::Value;

#[derive(Debug)]
pub struct Session<S: EventStore> {
    registry: SubscriptionRegistry,
    store: S,
}

impl<S: EventStore> Session<S> {
    pub fn new(store: S) -> Self {
        Self {
            registry: SubscriptionRegistry::default(),
            store,
        }
    }

    pub fn registry(&self) -> &SubscriptionRegistry {
        &self.registry
    }

    pub fn handle_raw(&mut self, input: &str) -> Vec<ServerMessage> {
        match decode_client_message(input) {
            Ok(message) => self.handle_message(message),
            Err(err) => vec![Notice::from(err).into()],
        }
    }

    pub fn handle_message(&mut self, message: ClientMessage) -> Vec<ServerMessage> {
        match message {
            ClientMessage::Req {
                subscription_id,
                filters,
            } => {
                let parsed = match parse_filters(&filters) {
                    Ok(filters) => filters,
                    Err(err) => return vec![Notice::from(err).into()],
                };
                let events = match self.store.query(&parsed) {
                    Ok(events) => events,
                    Err(err) => return vec![Notice::from(err).into()],
                };

                let sub_id = SubscriptionId::new(subscription_id.clone());
                self.registry.insert(sub_id.clone());

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
            ClientMessage::Event(_) | ClientMessage::Auth(_) | ClientMessage::Count { .. } => {
                Vec::new()
            }
        }
    }
}

fn parse_filters(filters: &[Value]) -> Result<Vec<Filter>, crate::FilterError> {
    filters
        .iter()
        .map(Filter::from_json)
        .collect::<Result<Vec<_>, _>>()
}

#[cfg(test)]
mod tests {
    use super::Session;
    use crate::{ClientMessage, EventStore, MemoryStore, NostrEvent, ServerMessage};
    use serde_json::json;

    #[test]
    fn handle_raw_reports_invalid_messages() {
        let mut session = Session::new(MemoryStore::new());
        let responses = session.handle_raw("{\"bad\":true}");
        assert_eq!(responses.len(), 1);
        assert!(matches!(responses[0], ServerMessage::Notice { .. }));
    }

    #[test]
    fn handle_message_updates_subscriptions() {
        let mut session = Session::new(MemoryStore::new());
        session.handle_message(ClientMessage::Req {
            subscription_id: "sub".to_string(),
            filters: vec![json!({})],
        });
        assert!(session.registry().contains(&crate::SubscriptionId::new("sub")));

        session.handle_message(ClientMessage::Close {
            subscription_id: "sub".to_string(),
        });
        assert!(!session.registry().contains(&crate::SubscriptionId::new("sub")));
    }

    #[test]
    fn req_sends_events_and_eose() {
        let mut store = MemoryStore::new();
        let event = NostrEvent {
            id: "id".to_string(),
            pubkey: "aa".repeat(32),
            created_at: 1,
            kind: 1,
            tags: Vec::new(),
            content: String::new(),
            sig: "00".repeat(64),
        };
        store.insert(event).expect("insert");

        let mut session = Session::new(store);
        let responses = session.handle_message(ClientMessage::Req {
            subscription_id: "sub".to_string(),
            filters: vec![json!({})],
        });

        assert_eq!(responses.len(), 2);
        assert!(matches!(responses[0], ServerMessage::Event { .. }));
        assert!(matches!(responses[1], ServerMessage::Eose { .. }));
        assert!(session.registry().eose_sent(&crate::SubscriptionId::new("sub")));
    }
}
