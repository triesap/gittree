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
        if self
            .session
            .max_message_bytes()
            .is_some_and(|limit| input.len() > limit)
        {
            let notice = Notice::message("message too large");
            return vec![encode_response(notice.into())];
        }
        let responses = self.session.handle_raw(input).await;
        responses.into_iter().map(encode_response).collect()
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
        for message in self.session.initial_messages() {
            if outbound.send(encode_response(message)).await.is_err() {
                return;
            }
        }
        loop {
            tokio::select! {
                input = inbound.recv() => {
                    let Some(input) = input else {
                        return;
                    };
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
            }
        }
    }
}

fn encode_response(message: ServerMessage) -> String {
    encode_response_with(message, |payload| encode_server_message(payload))
}

fn encode_response_with<F, E>(message: ServerMessage, encode: F) -> String
where
    F: FnOnce(&ServerMessage) -> Result<String, E>,
{
    match encode(&message) {
        Ok(serialized) => serialized,
        Err(_) => "[\"NOTICE\",\"failed to encode response\"]".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::SessionDriver;
    use crate::{EventStore, MemoryStore, NostrEvent, Policy, ServerMessage, Session};
    use serde_json::Value;
    use tokio::sync::{broadcast, mpsc};
    use tokio::time::{Duration, timeout};

    fn decode_frames(frames: &[String]) -> Vec<Value> {
        frames
            .iter()
            .map(|frame| serde_json::from_str(frame).expect("valid json"))
            .collect()
    }

    fn sample_event(id: &str) -> NostrEvent {
        NostrEvent {
            id: id.to_string(),
            pubkey: "aa".repeat(32),
            created_at: 1,
            kind: 1,
            tags: Vec::new(),
            content: String::new(),
            sig: "00".repeat(64),
        }
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
        let frames = driver.handle_text(r#"["REQ","sub",{}]"#).await;

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

    #[tokio::test]
    async fn driver_rejects_oversized_messages() {
        let store = MemoryStore::new();
        let policy = Policy {
            max_message_bytes: Some(10),
            ..Policy::default()
        };
        let session = Session::with_policy(store, policy);
        let mut driver = SessionDriver::new(session);
        let frames = driver.handle_text(r#"["REQ","sub",{}]"#).await;

        let decoded = decode_frames(&frames);
        assert_eq!(decoded.len(), 1);
        assert_eq!(decoded[0][0], Value::String("NOTICE".to_string()));
    }

    #[tokio::test]
    async fn run_emits_initial_auth_challenge() {
        let session = Session::with_policy_and_auth(MemoryStore::new(), Policy::default(), true);
        let driver = SessionDriver::new(session);
        let (in_tx, in_rx) = mpsc::channel(1);
        let (out_tx, mut out_rx) = mpsc::channel(2);
        let task = tokio::spawn(driver.run(in_rx, out_tx));
        drop(in_tx);

        let first = timeout(Duration::from_secs(1), out_rx.recv())
            .await
            .expect("timeout")
            .expect("frame");
        let decoded: Value = serde_json::from_str(&first).expect("json");
        assert_eq!(decoded[0], Value::String("AUTH".to_string()));
        task.await.expect("task");
    }

    #[tokio::test]
    async fn run_returns_when_outbound_is_closed_during_initial_send() {
        let session = Session::with_policy_and_auth(MemoryStore::new(), Policy::default(), true);
        let driver = SessionDriver::new(session);
        let (_in_tx, in_rx) = mpsc::channel(1);
        let (out_tx, out_rx) = mpsc::channel(1);
        drop(out_rx);
        timeout(Duration::from_secs(1), driver.run(in_rx, out_tx))
            .await
            .expect("timeout");
    }

    #[tokio::test]
    async fn run_stops_after_outbound_closes_for_inbound_responses() {
        let session = Session::new(MemoryStore::new());
        let driver = SessionDriver::new(session);
        let (in_tx, in_rx) = mpsc::channel(1);
        let (out_tx, out_rx) = mpsc::channel(1);
        drop(out_rx);
        let task = tokio::spawn(driver.run(in_rx, out_tx));
        in_tx
            .send(r#"["REQ","sub",{}]"#.to_string())
            .await
            .expect("send inbound");
        drop(in_tx);
        timeout(Duration::from_secs(1), task)
            .await
            .expect("timeout")
            .expect("task");
    }

    #[tokio::test]
    async fn run_breaks_when_outbound_closes_mid_response_batch() {
        let store = MemoryStore::new();
        let event = sample_event("evt-batch");
        store.insert(event).await.expect("insert");

        let session = Session::new(store);
        let driver = SessionDriver::new(session);
        let (in_tx, in_rx) = mpsc::channel(1);
        let (out_tx, mut out_rx) = mpsc::channel(1);
        let task = tokio::spawn(driver.run(in_rx, out_tx));

        in_tx
            .send(r#"["REQ","sub",{}]"#.to_string())
            .await
            .expect("send inbound");
        let _ = timeout(Duration::from_secs(1), out_rx.recv())
            .await
            .expect("timeout")
            .expect("first frame");
        drop(out_rx);
        drop(in_tx);
        timeout(Duration::from_secs(1), task)
            .await
            .expect("timeout")
            .expect("task");
    }

    #[tokio::test]
    async fn run_with_broadcast_returns_when_initial_send_fails() {
        let session = Session::with_policy_and_auth(MemoryStore::new(), Policy::default(), true);
        let driver = SessionDriver::new(session);
        let (_in_tx, in_rx) = mpsc::channel(1);
        let (out_tx, out_rx) = mpsc::channel(1);
        drop(out_rx);
        let (_broadcast_tx, broadcast_rx) = broadcast::channel(1);
        timeout(
            Duration::from_secs(1),
            driver.run_with_broadcast(in_rx, out_tx, broadcast_rx),
        )
        .await
        .expect("timeout");
    }

    #[tokio::test]
    async fn run_with_broadcast_handles_lagged_then_closed_channel() {
        let session = Session::new(MemoryStore::new());
        let driver = SessionDriver::new(session);
        let (_in_tx, in_rx) = mpsc::channel(1);
        let (out_tx, _out_rx) = mpsc::channel(4);
        let (broadcast_tx, broadcast_rx) = broadcast::channel(1);
        broadcast_tx.send(sample_event("e1")).expect("send 1");
        broadcast_tx.send(sample_event("e2")).expect("send 2");
        drop(broadcast_tx);
        timeout(
            Duration::from_secs(1),
            driver.run_with_broadcast(in_rx, out_tx, broadcast_rx),
        )
        .await
        .expect("timeout");
    }

    #[tokio::test]
    async fn run_with_broadcast_returns_when_outbound_closes_after_inbound() {
        let session = Session::new(MemoryStore::new());
        let driver = SessionDriver::new(session);
        let (in_tx, in_rx) = mpsc::channel(1);
        let (out_tx, out_rx) = mpsc::channel(1);
        drop(out_rx);
        let (_broadcast_tx, broadcast_rx) = broadcast::channel(1);
        let task = tokio::spawn(driver.run_with_broadcast(in_rx, out_tx, broadcast_rx));
        in_tx
            .send(r#"["REQ","sub",{}]"#.to_string())
            .await
            .expect("send inbound");
        drop(in_tx);
        timeout(Duration::from_secs(1), task)
            .await
            .expect("timeout")
            .expect("task");
    }

    #[tokio::test]
    async fn run_with_broadcast_stops_when_outbound_closes_for_broadcast_frames() {
        let session = Session::new(MemoryStore::new());
        let driver = SessionDriver::new(session);
        let (in_tx, in_rx) = mpsc::channel(2);
        let (out_tx, mut out_rx) = mpsc::channel(8);
        let (broadcast_tx, broadcast_rx) = broadcast::channel(2);
        let task = tokio::spawn(driver.run_with_broadcast(in_rx, out_tx, broadcast_rx));

        in_tx
            .send(r#"["REQ","sub",{}]"#.to_string())
            .await
            .expect("send req");
        let _ = timeout(Duration::from_secs(1), out_rx.recv())
            .await
            .expect("timeout")
            .expect("response");

        drop(out_rx);
        broadcast_tx
            .send(sample_event("broadcast"))
            .expect("send broadcast");
        drop(broadcast_tx);

        timeout(Duration::from_secs(1), task)
            .await
            .expect("timeout")
            .expect("task");
        drop(in_tx);
    }

    #[tokio::test]
    async fn run_with_broadcast_emits_event_for_matching_subscription() {
        let session = Session::new(MemoryStore::new());
        let driver = SessionDriver::new(session);
        let (in_tx, in_rx) = mpsc::channel(2);
        let (out_tx, mut out_rx) = mpsc::channel(8);
        let (broadcast_tx, broadcast_rx) = broadcast::channel(2);
        let task = tokio::spawn(driver.run_with_broadcast(in_rx, out_tx, broadcast_rx));

        in_tx
            .send(r#"["REQ","sub",{}]"#.to_string())
            .await
            .expect("send req");
        let first = timeout(Duration::from_secs(1), out_rx.recv())
            .await
            .expect("timeout")
            .expect("response");
        let first: Value = serde_json::from_str(&first).expect("json");
        assert_eq!(first[0], Value::String("EOSE".to_string()));

        broadcast_tx
            .send(sample_event("broadcast-hit"))
            .expect("send broadcast");
        let second = timeout(Duration::from_secs(1), out_rx.recv())
            .await
            .expect("timeout")
            .expect("response");
        let second: Value = serde_json::from_str(&second).expect("json");
        assert_eq!(second[0], Value::String("EVENT".to_string()));

        drop(in_tx);
        drop(broadcast_tx);
        timeout(Duration::from_secs(1), task)
            .await
            .expect("timeout")
            .expect("task");
    }

    #[tokio::test]
    async fn run_with_broadcast_returns_when_broadcast_send_fails_without_inbound_input() {
        let session = Session::new(MemoryStore::new());
        let driver = SessionDriver::new(session);
        let (_in_tx, in_rx) = mpsc::channel(1);
        let (out_tx, out_rx) = mpsc::channel(1);
        drop(out_rx);
        let (broadcast_tx, broadcast_rx) = broadcast::channel(1);
        let task = tokio::spawn(driver.run_with_broadcast(in_rx, out_tx, broadcast_rx));
        broadcast_tx
            .send(sample_event("broadcast-only"))
            .expect("send broadcast");
        drop(broadcast_tx);
        timeout(Duration::from_secs(1), task)
            .await
            .expect("timeout")
            .expect("task");
    }

    #[tokio::test]
    async fn run_with_broadcast_returns_on_closed_inputs() {
        let session = Session::new(MemoryStore::new());
        let driver = SessionDriver::new(session);
        let (in_tx, in_rx) = mpsc::channel(1);
        let (out_tx, _out_rx) = mpsc::channel(1);
        let (_broadcast_tx, broadcast_rx) = broadcast::channel(1);
        drop(in_tx);
        timeout(
            Duration::from_secs(1),
            driver.run_with_broadcast(in_rx, out_tx, broadcast_rx),
        )
        .await
        .expect("timeout");
    }

    #[test]
    fn encode_response_falls_back_when_encoder_errors() {
        let encoded = super::encode_response_with(
            ServerMessage::Notice {
                message: "any".to_string(),
            },
            |_| -> Result<String, ()> { Err(()) },
        );
        assert_eq!(encoded, "[\"NOTICE\",\"failed to encode response\"]");
    }
}
