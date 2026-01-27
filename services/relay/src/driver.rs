use crate::{EventStore, Notice, ServerMessage, Session, encode_server_message};
use tokio::sync::{broadcast, mpsc};

pub struct SessionDriver<S: EventStore> {
    session: Session<S>,
}

impl<S: EventStore> SessionDriver<S> {
    pub fn new(session: Session<S>) -> Self {
        Self { session }
    }

    pub async fn handle_text(&mut self, input: &str) -> Vec<String> {
        let responses = self.session.handle_raw(input).await;
        responses
            .into_iter()
            .map(encode_response)
            .collect()
    }

    pub async fn run(
        mut self,
        mut inbound: mpsc::Receiver<String>,
        outbound: mpsc::Sender<String>,
    ) {
        for message in self.session.initial_messages() {
            if outbound.send(encode_response(message)).await.is_err() {
                return;
            }
        }
        while let Some(input) = inbound.recv().await {
            let responses = self.handle_text(&input).await;
            for response in responses {
                if outbound.send(response).await.is_err() {
                    break;
                }
            }
        }
    }

    pub async fn run_with_broadcast(
        mut self,
        mut inbound: mpsc::Receiver<String>,
        outbound: mpsc::Sender<String>,
        mut broadcast_rx: broadcast::Receiver<crate::NostrEvent>,
    ) {
        let outbound = outbound;
        for message in self.session.initial_messages() {
            if outbound.send(encode_response(message)).await.is_err() {
                return;
            }
        }
        loop {
            tokio::select! {
                Some(input) = inbound.recv() => {
                    let responses = self.handle_text(&input).await;
                    for response in responses {
                        if outbound.send(response).await.is_err() {
                            return;
                        }
                    }
                }
                recv = broadcast_rx.recv() => {
                    let event = match recv {
                        Ok(event) => event,
                        Err(broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(broadcast::error::RecvError::Closed) => return,
                    };
                    let responses = self.session.dispatch_event(&event);
                    for response in responses {
                        let encoded = encode_response(response);
                        if outbound.send(encoded).await.is_err() {
                            return;
                        }
                    }
                }
                else => {
                    return;
                }
            }
        }
    }
}

fn encode_response(message: ServerMessage) -> String {
    match encode_server_message(&message) {
        Ok(serialized) => serialized,
        Err(err) => {
            let notice = Notice::message(err.to_string());
            encode_server_message(&notice.into())
                .unwrap_or_else(|_| "[\"NOTICE\",\"failed to encode response\"]".to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::SessionDriver;
    use crate::{EventStore, MemoryStore, NostrEvent, Session};
    use serde_json::Value;

    fn decode_frames(frames: &[String]) -> Vec<Value> {
        frames
            .iter()
            .map(|frame| serde_json::from_str(frame).expect("valid json"))
            .collect()
    }

    #[tokio::test]
    async fn driver_emits_event_and_eose() {
        let store = MemoryStore::new();
        let event = NostrEvent {
            id: "evt".to_string(),
            pubkey: "aa".repeat(32),
            created_at: 1,
            kind: 1,
            tags: Vec::new(),
            content: String::new(),
            sig: "00".repeat(64),
        };
        store.insert(event).await.expect("insert");

        let session = Session::new(store);
        let mut driver = SessionDriver::new(session);
        let frames = driver
            .handle_text(r#"["REQ","sub",{}]"#)
            .await;

        let decoded = decode_frames(&frames);
        assert_eq!(decoded.len(), 2);
        assert_eq!(decoded[0][0], Value::String("EVENT".to_string()));
        assert_eq!(decoded[1][0], Value::String("EOSE".to_string()));
    }

    #[tokio::test]
    async fn driver_emits_ok_for_events() {
        let store = MemoryStore::new();
        let session = Session::new(store);
        let mut driver = SessionDriver::new(session);
        let frames = driver
            .handle_text(r#"["EVENT",{"id":"evt","pubkey":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","created_at":1,"kind":1,"tags":[],"content":"","sig":"00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000"}]"#)
            .await;

        let decoded = decode_frames(&frames);
        assert_eq!(decoded.len(), 1);
        assert_eq!(decoded[0][0], Value::String("OK".to_string()));
    }
}
