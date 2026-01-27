use crate::{
    ClientMessage, Notice, ServerMessage, SubscriptionId, SubscriptionRegistry,
    decode_client_message,
};

#[derive(Debug, Default)]
pub struct Session {
    registry: SubscriptionRegistry,
}

impl Session {
    pub fn new() -> Self {
        Self::default()
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
            ClientMessage::Req { subscription_id, .. } => {
                self.registry.insert(SubscriptionId::new(subscription_id));
                Vec::new()
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

#[cfg(test)]
mod tests {
    use super::Session;
    use crate::{ClientMessage, ServerMessage};

    #[test]
    fn handle_raw_reports_invalid_messages() {
        let mut session = Session::new();
        let responses = session.handle_raw("{\"bad\":true}");
        assert_eq!(responses.len(), 1);
        assert!(matches!(responses[0], ServerMessage::Notice { .. }));
    }

    #[test]
    fn handle_message_updates_subscriptions() {
        let mut session = Session::new();
        session.handle_message(ClientMessage::Req {
            subscription_id: "sub".to_string(),
            filters: Vec::new(),
        });
        assert!(session.registry().contains(&crate::SubscriptionId::new("sub")));

        session.handle_message(ClientMessage::Close {
            subscription_id: "sub".to_string(),
        });
        assert!(!session.registry().contains(&crate::SubscriptionId::new("sub")));
    }
}
