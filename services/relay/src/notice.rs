use crate::{EventError, FilterError, ProtocolError, ServerMessage};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Notice {
    Message(String),
}

impl Notice {
    pub fn message(msg: impl Into<String>) -> Self {
        Notice::Message(msg.into())
    }

    pub fn as_str(&self) -> &str {
        match self {
            Notice::Message(msg) => msg,
        }
    }
}

impl From<ProtocolError> for Notice {
    fn from(err: ProtocolError) -> Self {
        Notice::message(format!("invalid message: {err}"))
    }
}

impl From<EventError> for Notice {
    fn from(err: EventError) -> Self {
        Notice::message(format!("invalid event: {err}"))
    }
}

impl From<FilterError> for Notice {
    fn from(err: FilterError) -> Self {
        Notice::message(format!("invalid filter: {err}"))
    }
}

impl From<Notice> for ServerMessage {
    fn from(notice: Notice) -> Self {
        ServerMessage::Notice {
            message: notice.as_str().to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Notice;
    use crate::{EventError, FilterError, ProtocolError, ServerMessage};

    #[test]
    fn protocol_error_maps_to_notice_message() {
        let notice = Notice::from(ProtocolError::InvalidFrame);
        assert_eq!(notice.as_str(), "invalid message: invalid frame");
    }

    #[test]
    fn filter_error_maps_to_notice_message() {
        let notice = Notice::from(FilterError::InvalidField("ids".to_string()));
        assert_eq!(notice.as_str(), "invalid filter: invalid filter field ids");
    }

    #[test]
    fn event_error_maps_to_notice_message() {
        let notice = Notice::from(EventError::InvalidSignature);
        assert_eq!(notice.as_str(), "invalid event: invalid event signature");
    }

    #[test]
    fn notice_converts_to_server_message() {
        let notice = Notice::message("hello");
        let message: ServerMessage = notice.into();
        assert_eq!(
            message,
            ServerMessage::Notice {
                message: "hello".to_string()
            }
        );
    }
}
