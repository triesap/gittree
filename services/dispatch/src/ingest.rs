use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelayEventEnvelope {
    pub id: String,
    pub pubkey: String,
    pub kind: u32,
    pub created_at: i64,
    pub content: String,
    pub tags: Vec<Vec<String>>,
    pub relay_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DispatchFilterConfig {
    pub admin_pubkey: String,
    pub relay_allowlist: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IngestRejectReason {
    WrongKind,
    MissingPrefix,
    MissingAdminTag,
    RelayNotAllowed,
}

pub fn is_dispatch_command_event(
    config: &DispatchFilterConfig,
    event: &RelayEventEnvelope,
) -> Result<(), IngestRejectReason> {
    if event.kind != 1 {
        return Err(IngestRejectReason::WrongKind);
    }
    if !event.content.trim_start().starts_with("gittree ") {
        return Err(IngestRejectReason::MissingPrefix);
    }
    if !config
        .relay_allowlist
        .iter()
        .any(|relay| relay == &event.relay_url)
    {
        return Err(IngestRejectReason::RelayNotAllowed);
    }
    if !has_admin_tag(event, &config.admin_pubkey) {
        return Err(IngestRejectReason::MissingAdminTag);
    }
    Ok(())
}

fn has_admin_tag(event: &RelayEventEnvelope, admin_pubkey: &str) -> bool {
    event
        .tags
        .iter()
        .any(|tag| tag.len() >= 2 && tag[0] == "p" && tag[1] == admin_pubkey)
}

#[cfg(test)]
mod tests {
    use super::{
        DispatchFilterConfig, IngestRejectReason, RelayEventEnvelope, is_dispatch_command_event,
    };

    fn config() -> DispatchFilterConfig {
        DispatchFilterConfig {
            admin_pubkey: "npub1admin".to_string(),
            relay_allowlist: vec!["wss://gittr.ee".to_string()],
        }
    }

    fn envelope() -> RelayEventEnvelope {
        RelayEventEnvelope {
            id: "event-1".to_string(),
            pubkey: "npub1user".to_string(),
            kind: 1,
            created_at: 100,
            content: "gittree account create".to_string(),
            tags: vec![vec!["p".to_string(), "npub1admin".to_string()]],
            relay_url: "wss://gittr.ee".to_string(),
        }
    }

    #[test]
    fn accepts_valid_command_event() {
        let result = is_dispatch_command_event(&config(), &envelope());
        assert!(result.is_ok());
    }

    #[test]
    fn rejects_wrong_kind() {
        let mut event = envelope();
        event.kind = 7;
        let result = is_dispatch_command_event(&config(), &event);
        assert_eq!(result, Err(IngestRejectReason::WrongKind));
    }

    #[test]
    fn rejects_missing_prefix() {
        let mut event = envelope();
        event.content = "hello world".to_string();
        let result = is_dispatch_command_event(&config(), &event);
        assert_eq!(result, Err(IngestRejectReason::MissingPrefix));
    }

    #[test]
    fn rejects_unknown_relay() {
        let mut event = envelope();
        event.relay_url = "wss://other.example".to_string();
        let result = is_dispatch_command_event(&config(), &event);
        assert_eq!(result, Err(IngestRejectReason::RelayNotAllowed));
    }

    #[test]
    fn rejects_missing_admin_tag() {
        let mut event = envelope();
        event.tags = vec![vec!["p".to_string(), "npub1other".to_string()]];
        let result = is_dispatch_command_event(&config(), &event);
        assert_eq!(result, Err(IngestRejectReason::MissingAdminTag));
    }
}
