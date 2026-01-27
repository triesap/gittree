use crate::NostrEvent;
use gittree_config::RelayPolicyConfig;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Policy {
    pub max_content_len: usize,
    pub max_tags: usize,
    pub max_tag_values: usize,
    pub max_tag_value_len: usize,
    pub max_future_seconds: i64,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            max_content_len: 8_192,
            max_tags: 128,
            max_tag_values: 16,
            max_tag_value_len: 512,
            max_future_seconds: 60,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyError {
    ContentTooLong,
    TooManyTags,
    TooManyTagValues,
    TagValueTooLong,
    EventInFuture,
}

impl std::fmt::Display for PolicyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PolicyError::ContentTooLong => write!(f, "content too long"),
            PolicyError::TooManyTags => write!(f, "too many tags"),
            PolicyError::TooManyTagValues => write!(f, "too many tag values"),
            PolicyError::TagValueTooLong => write!(f, "tag value too long"),
            PolicyError::EventInFuture => write!(f, "event timestamp too far in future"),
        }
    }
}

impl std::error::Error for PolicyError {}

impl Policy {
    pub fn from_config(config: &RelayPolicyConfig) -> Self {
        Self {
            max_content_len: config.max_content_len as usize,
            max_tags: config.max_tags as usize,
            max_tag_values: config.max_tag_values as usize,
            max_tag_value_len: config.max_tag_value_len as usize,
            max_future_seconds: config.max_future_seconds as i64,
        }
    }

    pub fn validate_event(&self, event: &NostrEvent, now: i64) -> Result<(), PolicyError> {
        if event.content.len() > self.max_content_len {
            return Err(PolicyError::ContentTooLong);
        }
        if event.tags.len() > self.max_tags {
            return Err(PolicyError::TooManyTags);
        }
        for tag in &event.tags {
            if tag.len().saturating_sub(1) > self.max_tag_values {
                return Err(PolicyError::TooManyTagValues);
            }
            for value in tag.iter().skip(1) {
                if value.len() > self.max_tag_value_len {
                    return Err(PolicyError::TagValueTooLong);
                }
            }
        }
        if event.created_at > now + self.max_future_seconds {
            return Err(PolicyError::EventInFuture);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{Policy, PolicyError};
    use crate::NostrEvent;
    use gittree_config::RelayPolicyConfig;

    fn sample_event() -> NostrEvent {
        NostrEvent {
            id: "id".to_string(),
            pubkey: "aa".repeat(32),
            created_at: 0,
            kind: 1,
            tags: Vec::new(),
            content: String::new(),
            sig: "00".repeat(64),
        }
    }

    #[test]
    fn rejects_content_too_long() {
        let mut event = sample_event();
        event.content = "x".repeat(5);
        let policy = Policy {
            max_content_len: 4,
            ..Policy::default()
        };
        let err = policy.validate_event(&event, 0).unwrap_err();
        assert_eq!(err, PolicyError::ContentTooLong);
    }

    #[test]
    fn policy_from_config_maps_limits() {
        let config = RelayPolicyConfig {
            max_content_len: 2048,
            max_tags: 5,
            max_tag_values: 3,
            max_tag_value_len: 22,
            max_future_seconds: 12,
            max_subscriptions: None,
            max_limit: None,
            max_message_bytes: None,
            auth_required: false,
        };
        let policy = Policy::from_config(&config);
        assert_eq!(policy.max_content_len, 2048);
        assert_eq!(policy.max_tags, 5);
        assert_eq!(policy.max_tag_values, 3);
        assert_eq!(policy.max_tag_value_len, 22);
        assert_eq!(policy.max_future_seconds, 12);
    }

    #[test]
    fn rejects_too_many_tags() {
        let mut event = sample_event();
        event.tags = vec![vec!["e".to_string(), "1".to_string()]; 3];
        let policy = Policy {
            max_tags: 2,
            ..Policy::default()
        };
        let err = policy.validate_event(&event, 0).unwrap_err();
        assert_eq!(err, PolicyError::TooManyTags);
    }

    #[test]
    fn rejects_tag_value_too_long() {
        let mut event = sample_event();
        event.tags = vec![vec!["e".to_string(), "long".to_string()]];
        let policy = Policy {
            max_tag_value_len: 2,
            ..Policy::default()
        };
        let err = policy.validate_event(&event, 0).unwrap_err();
        assert_eq!(err, PolicyError::TagValueTooLong);
    }

    #[test]
    fn rejects_future_timestamps() {
        let mut event = sample_event();
        event.created_at = 100;
        let policy = Policy {
            max_future_seconds: 10,
            ..Policy::default()
        };
        let err = policy.validate_event(&event, 0).unwrap_err();
        assert_eq!(err, PolicyError::EventInFuture);
    }
}
