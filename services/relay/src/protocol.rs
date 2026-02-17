use serde_json::{Value, json};

#[derive(Debug, Clone, PartialEq)]
pub enum ClientMessage {
    Event(Value),
    Req {
        subscription_id: String,
        filters: Vec<Value>,
    },
    Close {
        subscription_id: String,
    },
    Auth(Value),
    Count {
        subscription_id: String,
        filters: Vec<Value>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum ServerMessage {
    Event {
        subscription_id: String,
        event: Value,
    },
    Notice {
        message: String,
    },
    Eose {
        subscription_id: String,
    },
    Ok {
        event_id: String,
        accepted: bool,
        message: String,
    },
    Closed {
        subscription_id: String,
        message: String,
    },
    Auth {
        challenge: String,
    },
    Count {
        subscription_id: String,
        count: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProtocolError {
    InvalidJson,
    InvalidFrame,
    MissingField(&'static str),
    InvalidField(&'static str),
    UnknownMessageType(String),
}

impl std::fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProtocolError::InvalidJson => write!(f, "invalid json"),
            ProtocolError::InvalidFrame => write!(f, "invalid frame"),
            ProtocolError::MissingField(field) => write!(f, "missing field {field}"),
            ProtocolError::InvalidField(field) => write!(f, "invalid field {field}"),
            ProtocolError::UnknownMessageType(kind) => write!(f, "unknown message type {kind}"),
        }
    }
}

impl std::error::Error for ProtocolError {}

pub fn decode_client_message(input: &str) -> Result<ClientMessage, ProtocolError> {
    let value: Value = serde_json::from_str(input).map_err(|_| ProtocolError::InvalidJson)?;
    let items = value.as_array().ok_or(ProtocolError::InvalidFrame)?;
    let kind = items
        .first()
        .and_then(|value| value.as_str())
        .ok_or(ProtocolError::MissingField("type"))?;

    match kind {
        "EVENT" => {
            let event = items
                .get(1)
                .cloned()
                .ok_or(ProtocolError::MissingField("event"))?;
            Ok(ClientMessage::Event(event))
        }
        "REQ" => {
            let subscription_id = items
                .get(1)
                .and_then(|value| value.as_str())
                .ok_or(ProtocolError::MissingField("subscription_id"))?
                .to_string();
            if items.len() < 3 {
                return Err(ProtocolError::MissingField("filters"));
            }
            let filters = items.iter().skip(2).cloned().collect::<Vec<_>>();
            Ok(ClientMessage::Req {
                subscription_id,
                filters,
            })
        }
        "CLOSE" => {
            let subscription_id = items
                .get(1)
                .and_then(|value| value.as_str())
                .ok_or(ProtocolError::MissingField("subscription_id"))?
                .to_string();
            Ok(ClientMessage::Close { subscription_id })
        }
        "AUTH" => {
            let event = items
                .get(1)
                .cloned()
                .ok_or(ProtocolError::MissingField("event"))?;
            Ok(ClientMessage::Auth(event))
        }
        "COUNT" => {
            let subscription_id = items
                .get(1)
                .and_then(|value| value.as_str())
                .ok_or(ProtocolError::MissingField("subscription_id"))?
                .to_string();
            if items.len() < 3 {
                return Err(ProtocolError::MissingField("filters"));
            }
            let filters = items.iter().skip(2).cloned().collect::<Vec<_>>();
            Ok(ClientMessage::Count {
                subscription_id,
                filters,
            })
        }
        other => Err(ProtocolError::UnknownMessageType(other.to_string())),
    }
}

pub fn encode_server_message(message: &ServerMessage) -> Result<String, ProtocolError> {
    let value = match message {
        ServerMessage::Event {
            subscription_id,
            event,
        } => json!(["EVENT", subscription_id, event]),
        ServerMessage::Notice { message } => json!(["NOTICE", message]),
        ServerMessage::Eose { subscription_id } => json!(["EOSE", subscription_id]),
        ServerMessage::Ok {
            event_id,
            accepted,
            message,
        } => json!(["OK", event_id, accepted, message]),
        ServerMessage::Closed {
            subscription_id,
            message,
        } => json!(["CLOSED", subscription_id, message]),
        ServerMessage::Auth { challenge } => json!(["AUTH", challenge]),
        ServerMessage::Count {
            subscription_id,
            count,
        } => json!(["COUNT", subscription_id, {"count": count}]),
    };

    serde_json::to_string(&value).map_err(|_| ProtocolError::InvalidFrame)
}

#[cfg(test)]
mod tests {
    use super::{
        ClientMessage, ProtocolError, ServerMessage, decode_client_message, encode_server_message,
    };
    use serde_json::json;

    #[test]
    fn decode_event_message() {
        let input = r#"["EVENT",{"id":"abc"}]"#;
        let message = decode_client_message(input).expect("message");
        assert_eq!(message, ClientMessage::Event(json!({"id": "abc"})));
    }

    #[test]
    fn decode_req_message() {
        let input = r#"["REQ","sub",{"kinds":[1]},{"authors":["a"]}]"#;
        let message = decode_client_message(input).expect("message");
        assert_eq!(
            message,
            ClientMessage::Req {
                subscription_id: "sub".to_string(),
                filters: vec![json!({"kinds":[1]}), json!({"authors":["a"]})],
            }
        );
    }

    #[test]
    fn decode_close_message() {
        let input = r#"["CLOSE","sub"]"#;
        let message = decode_client_message(input).expect("message");
        assert_eq!(
            message,
            ClientMessage::Close {
                subscription_id: "sub".to_string(),
            }
        );
    }

    #[test]
    fn decode_auth_message() {
        let input = r#"["AUTH",{"id":"auth-evt"}]"#;
        let message = decode_client_message(input).expect("message");
        assert_eq!(message, ClientMessage::Auth(json!({"id": "auth-evt"})));
    }

    #[test]
    fn decode_count_message() {
        let input = r#"["COUNT","sub",{"kinds":[1]}]"#;
        let message = decode_client_message(input).expect("message");
        assert_eq!(
            message,
            ClientMessage::Count {
                subscription_id: "sub".to_string(),
                filters: vec![json!({"kinds":[1]})],
            }
        );
    }

    #[test]
    fn decode_unknown_message_type() {
        let input = r#"["PING"]"#;
        let err = decode_client_message(input).unwrap_err();
        assert_eq!(err, ProtocolError::UnknownMessageType("PING".to_string()));
    }

    #[test]
    fn decode_missing_filters() {
        let input = r#"["REQ","sub"]"#;
        let err = decode_client_message(input).unwrap_err();
        assert_eq!(err, ProtocolError::MissingField("filters"));
    }

    #[test]
    fn decode_count_missing_filters() {
        let input = r#"["COUNT","sub"]"#;
        let err = decode_client_message(input).unwrap_err();
        assert_eq!(err, ProtocolError::MissingField("filters"));
    }

    #[test]
    fn decode_rejects_missing_required_fields_by_message_type() {
        let err = decode_client_message(r#"["EVENT"]"#).unwrap_err();
        assert_eq!(err, ProtocolError::MissingField("event"));

        let err = decode_client_message(r#"["AUTH"]"#).unwrap_err();
        assert_eq!(err, ProtocolError::MissingField("event"));

        let err = decode_client_message(r#"["REQ"]"#).unwrap_err();
        assert_eq!(err, ProtocolError::MissingField("subscription_id"));

        let err = decode_client_message(r#"["CLOSE"]"#).unwrap_err();
        assert_eq!(err, ProtocolError::MissingField("subscription_id"));

        let err = decode_client_message(r#"["COUNT"]"#).unwrap_err();
        assert_eq!(err, ProtocolError::MissingField("subscription_id"));
    }

    #[test]
    fn decode_rejects_non_array() {
        let input = r#"{"EVENT":{"id":"abc"}}"#;
        let err = decode_client_message(input).unwrap_err();
        assert_eq!(err, ProtocolError::InvalidFrame);
    }

    #[test]
    fn encode_notice_message() {
        let message = ServerMessage::Notice {
            message: "hello".to_string(),
        };
        let encoded = encode_server_message(&message).expect("encode");
        let value: serde_json::Value = serde_json::from_str(&encoded).expect("json");
        assert_eq!(value, json!(["NOTICE", "hello"]));
    }

    #[test]
    fn encode_ok_message() {
        let message = ServerMessage::Ok {
            event_id: "id".to_string(),
            accepted: true,
            message: "ok".to_string(),
        };
        let encoded = encode_server_message(&message).expect("encode");
        let value: serde_json::Value = serde_json::from_str(&encoded).expect("json");
        assert_eq!(value, json!(["OK", "id", true, "ok"]));
    }

    #[test]
    fn encode_closed_message() {
        let message = ServerMessage::Closed {
            subscription_id: "sub".to_string(),
            message: "auth-required".to_string(),
        };
        let encoded = encode_server_message(&message).expect("encode");
        let value: serde_json::Value = serde_json::from_str(&encoded).expect("json");
        assert_eq!(value, json!(["CLOSED", "sub", "auth-required"]));
    }

    #[test]
    fn encode_auth_message() {
        let message = ServerMessage::Auth {
            challenge: "challenge".to_string(),
        };
        let encoded = encode_server_message(&message).expect("encode");
        let value: serde_json::Value = serde_json::from_str(&encoded).expect("json");
        assert_eq!(value, json!(["AUTH", "challenge"]));
    }

    #[test]
    fn encode_count_message() {
        let message = ServerMessage::Count {
            subscription_id: "sub".to_string(),
            count: 42,
        };
        let encoded = encode_server_message(&message).expect("encode");
        let value: serde_json::Value = serde_json::from_str(&encoded).expect("json");
        assert_eq!(value, json!(["COUNT", "sub", {"count": 42}]));
    }

    #[test]
    fn protocol_error_display_messages_are_stable() {
        assert_eq!(ProtocolError::InvalidJson.to_string(), "invalid json");
        assert_eq!(
            ProtocolError::MissingField("subscription_id").to_string(),
            "missing field subscription_id"
        );
        assert_eq!(
            ProtocolError::InvalidField("subscription_id").to_string(),
            "invalid field subscription_id"
        );
        assert_eq!(
            ProtocolError::UnknownMessageType("PING".to_string()).to_string(),
            "unknown message type PING"
        );
    }
}
