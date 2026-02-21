use crate::nip34_common::{
    HEX_EVENT_ID_LEN, HEX_PUBKEY_LEN, is_hex_hash, is_hex_len, validate_repo_address,
};
use crate::tags::{extend_unique, push_unique};
use crate::{CoreError, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusAppliedRef {
    pub event_id: String,
    pub relay: String,
    pub pubkey: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusEvent {
    pub root_event: String,
    pub reply_events: Vec<String>,
    pub mentions: Vec<String>,
    pub repo_address: Option<String>,
    pub repo_refs: Vec<String>,
    pub applied_refs: Vec<StatusAppliedRef>,
    pub merge_commit: Option<String>,
    pub applied_as_commits: Vec<String>,
}

impl StatusEvent {
    pub fn from_tags(tags: &[Vec<String>]) -> Result<Self> {
        let mut root_event = None;
        let mut reply_events = Vec::new();
        let mut mentions = Vec::new();
        let mut repo_address = None;
        let mut repo_refs = Vec::new();
        let mut applied_refs = Vec::new();
        let mut merge_commit = None;
        let mut applied_as_commits = Vec::new();

        for tag in tags {
            if let Some((event_id, marker)) = parse_e_tag(tag) {
                match marker {
                    Some("root") => {
                        if let Some(existing) = &root_event {
                            if existing != event_id {
                                return Err(CoreError::InvalidTag {
                                    tag: "e",
                                    value: event_id.to_string(),
                                });
                            }
                        } else {
                            root_event = Some(event_id.to_string());
                        }
                    }
                    Some("reply") => push_unique(&mut reply_events, event_id),
                    _ => {}
                }
                continue;
            }

            match tag.as_slice() {
                [t, value, ..] if t == "a" => repo_address = Some(value.clone()),
                [t, value, ..] if t == "p" => push_unique(&mut mentions, value),
                [t, value, ..] if t == "r" => push_unique(&mut repo_refs, value),
                [t, values @ ..] if t == "applied-as-commits" => {
                    extend_unique(&mut applied_as_commits, values)
                }
                [t, value, ..] if t == "merge-commit" => merge_commit = Some(value.clone()),
                [t, ..] if t == "q" => applied_refs.push(parse_q_tag(tag)?),
                _ => {}
            }
        }

        Ok(Self {
            root_event: root_event.ok_or(CoreError::MissingField("e"))?,
            reply_events,
            mentions,
            repo_address,
            repo_refs,
            applied_refs,
            merge_commit,
            applied_as_commits,
        })
    }

    pub fn to_tags(&self) -> Vec<Vec<String>> {
        let mut tags = Vec::new();

        tags.push(vec![
            "e".to_string(),
            self.root_event.clone(),
            "".to_string(),
            "root".to_string(),
        ]);

        for reply in &self.reply_events {
            tags.push(vec![
                "e".to_string(),
                reply.clone(),
                "".to_string(),
                "reply".to_string(),
            ]);
        }

        for mention in &self.mentions {
            tags.push(vec!["p".to_string(), mention.clone()]);
        }

        if let Some(repo_address) = &self.repo_address {
            tags.push(vec!["a".to_string(), repo_address.clone()]);
        }

        for repo_ref in &self.repo_refs {
            tags.push(vec!["r".to_string(), repo_ref.clone()]);
        }

        for applied in &self.applied_refs {
            tags.push(vec![
                "q".to_string(),
                applied.event_id.clone(),
                applied.relay.clone(),
                applied.pubkey.clone(),
            ]);
        }

        if let Some(merge_commit) = &self.merge_commit {
            tags.push(vec!["merge-commit".to_string(), merge_commit.clone()]);
        }

        if !self.applied_as_commits.is_empty() {
            let mut tag = Vec::with_capacity(self.applied_as_commits.len() + 1);
            tag.push("applied-as-commits".to_string());
            tag.extend(self.applied_as_commits.iter().cloned());
            tags.push(tag);
        }

        tags
    }

    pub fn validate(&self) -> Result<()> {
        if !is_hex_len(&self.root_event, HEX_EVENT_ID_LEN) {
            return Err(CoreError::InvalidField {
                field: "e",
                value: self.root_event.clone(),
            });
        }

        for reply in &self.reply_events {
            if !is_hex_len(reply, HEX_EVENT_ID_LEN) {
                return Err(CoreError::InvalidField {
                    field: "e",
                    value: reply.clone(),
                });
            }
        }

        for mention in &self.mentions {
            if !is_hex_len(mention, HEX_PUBKEY_LEN) {
                return Err(CoreError::InvalidField {
                    field: "p",
                    value: mention.clone(),
                });
            }
        }

        if let Some(repo_address) = &self.repo_address {
            validate_repo_address(repo_address)?;
        }

        for repo_ref in &self.repo_refs {
            if !is_hex_hash(repo_ref) {
                return Err(CoreError::InvalidField {
                    field: "r",
                    value: repo_ref.clone(),
                });
            }
        }

        for applied in &self.applied_refs {
            if !is_hex_len(&applied.event_id, HEX_EVENT_ID_LEN) {
                return Err(CoreError::InvalidField {
                    field: "q",
                    value: applied.event_id.clone(),
                });
            }
            if !is_hex_len(&applied.pubkey, HEX_PUBKEY_LEN) {
                return Err(CoreError::InvalidField {
                    field: "q",
                    value: applied.pubkey.clone(),
                });
            }
            if applied.relay.trim().is_empty() {
                return Err(CoreError::InvalidField {
                    field: "q",
                    value: applied.relay.clone(),
                });
            }
        }

        if let Some(merge_commit) = &self.merge_commit {
            if !is_hex_hash(merge_commit) {
                return Err(CoreError::InvalidField {
                    field: "merge-commit",
                    value: merge_commit.clone(),
                });
            }
        }

        for commit in &self.applied_as_commits {
            if !is_hex_hash(commit) {
                return Err(CoreError::InvalidField {
                    field: "applied-as-commits",
                    value: commit.clone(),
                });
            }
        }

        Ok(())
    }
}

fn parse_e_tag(tag: &[String]) -> Option<(&str, Option<&str>)> {
    if tag.len() < 2 || tag.first().map(|t| t.as_str()) != Some("e") {
        return None;
    }

    let event_id = tag[1].as_str();
    let marker = tag.get(3).map(String::as_str);
    Some((event_id, marker))
}

fn parse_q_tag(tag: &[String]) -> Result<StatusAppliedRef> {
    if tag.len() < 4 {
        return Err(CoreError::InvalidField {
            field: "q",
            value: tag.join(" "),
        });
    }

    Ok(StatusAppliedRef {
        event_id: tag[1].clone(),
        relay: tag[2].clone(),
        pubkey: tag[3].clone(),
    })
}

#[cfg(test)]
mod tests {
    use crate::CoreError;

    use super::StatusAppliedRef;
    use super::StatusEvent;

    fn hex_of(byte: u8, len: usize) -> String {
        format!("{:02x}", byte).repeat(len / 2)
    }

    #[test]
    fn status_round_trips_tags() {
        let pubkey = hex_of(0x11, 64);
        let status = StatusEvent {
            root_event: hex_of(0x22, 64),
            reply_events: vec![hex_of(0x33, 64)],
            mentions: vec![pubkey.clone()],
            repo_address: Some(format!("30617:{pubkey}:repo")),
            repo_refs: vec![hex_of(0x44, 40)],
            applied_refs: vec![StatusAppliedRef {
                event_id: hex_of(0x55, 64),
                relay: "wss://relay.example".to_string(),
                pubkey: pubkey.clone(),
            }],
            merge_commit: Some(hex_of(0x66, 40)),
            applied_as_commits: vec![hex_of(0x77, 40)],
        };

        let tags = status.to_tags();
        let parsed = StatusEvent::from_tags(&tags).expect("parse");
        assert_eq!(parsed, status);
        parsed.validate().expect("valid");
    }

    #[test]
    fn status_requires_root_event() {
        let status = StatusEvent {
            root_event: "bad".to_string(),
            reply_events: Vec::new(),
            mentions: Vec::new(),
            repo_address: None,
            repo_refs: Vec::new(),
            applied_refs: Vec::new(),
            merge_commit: None,
            applied_as_commits: Vec::new(),
        };

        assert!(status.validate().is_err());
    }

    #[test]
    fn q_tag_requires_all_fields() {
        let tags = vec![vec!["q".to_string(), "abc".to_string()]];
        assert!(StatusEvent::from_tags(&tags).is_err());
    }

    #[test]
    fn status_from_tags_rejects_conflicting_root_event_tags() {
        let first_root = hex_of(0x11, 64);
        let second_root = hex_of(0x22, 64);
        let tags = vec![
            vec![
                "e".to_string(),
                first_root,
                "".to_string(),
                "root".to_string(),
            ],
            vec![
                "e".to_string(),
                second_root.clone(),
                "".to_string(),
                "root".to_string(),
            ],
        ];

        assert!(matches!(
            StatusEvent::from_tags(&tags),
            Err(CoreError::InvalidTag {
                tag: "e",
                value
            }) if value == second_root
        ));
    }

    #[test]
    fn status_from_tags_allows_duplicate_same_root_and_ignores_unknown_tags() {
        let root = hex_of(0x11, 64);
        let tags = vec![
            vec![
                "e".to_string(),
                root.clone(),
                "".to_string(),
                "root".to_string(),
            ],
            vec![
                "e".to_string(),
                root.clone(),
                "".to_string(),
                "root".to_string(),
            ],
            vec![
                "e".to_string(),
                hex_of(0x22, 64),
                "".to_string(),
                "unknown".to_string(),
            ],
            vec!["x-unknown".to_string(), "ignored".to_string()],
        ];

        let status = StatusEvent::from_tags(&tags).expect("status");
        assert_eq!(status.root_event, root);
        assert!(status.reply_events.is_empty());
    }

    #[test]
    fn status_validate_rejects_invalid_reply_repo_ref_and_merge_commit() {
        let mut status = StatusEvent {
            root_event: hex_of(0x11, 64),
            reply_events: vec!["abcd".to_string()],
            mentions: vec![],
            repo_address: None,
            repo_refs: vec![hex_of(0x22, 40)],
            applied_refs: vec![],
            merge_commit: None,
            applied_as_commits: Vec::new(),
        };
        assert!(matches!(
            status.validate(),
            Err(CoreError::InvalidField { field: "e", .. })
        ));

        status.reply_events = vec![];
        status.repo_refs = vec!["abcd".to_string()];
        assert!(matches!(
            status.validate(),
            Err(CoreError::InvalidField { field: "r", .. })
        ));

        status.repo_refs = vec![hex_of(0x22, 40)];
        status.merge_commit = Some("abcd".to_string());
        assert!(matches!(
            status.validate(),
            Err(CoreError::InvalidField {
                field: "merge-commit",
                ..
            })
        ));
    }

    #[test]
    fn status_validate_rejects_invalid_applied_refs_and_commits() {
        let valid_pubkey = hex_of(0x11, 64);
        let valid_event = hex_of(0x22, 64);
        let valid_commit = hex_of(0x33, 40);
        let status = StatusEvent {
            root_event: hex_of(0x44, 64),
            reply_events: vec![],
            mentions: vec![],
            repo_address: None,
            repo_refs: vec![],
            applied_refs: vec![StatusAppliedRef {
                event_id: valid_event.clone(),
                relay: "wss://relay.example".to_string(),
                pubkey: valid_pubkey.clone(),
            }],
            merge_commit: None,
            applied_as_commits: vec![valid_commit.clone()],
        };
        status.validate().expect("baseline should be valid");

        let mut invalid_q_event = status.clone();
        invalid_q_event.applied_refs[0].event_id = "abcd".to_string();
        assert!(matches!(
            invalid_q_event.validate(),
            Err(CoreError::InvalidField { field: "q", .. })
        ));

        let mut invalid_q_pubkey = status.clone();
        invalid_q_pubkey.applied_refs[0].pubkey = "abcd".to_string();
        assert!(matches!(
            invalid_q_pubkey.validate(),
            Err(CoreError::InvalidField { field: "q", .. })
        ));

        let mut invalid_q_relay = status.clone();
        invalid_q_relay.applied_refs[0].relay = "   ".to_string();
        assert!(matches!(
            invalid_q_relay.validate(),
            Err(CoreError::InvalidField { field: "q", .. })
        ));

        let mut invalid_applied_commit = status;
        invalid_applied_commit.applied_as_commits = vec!["abcd".to_string()];
        assert!(matches!(
            invalid_applied_commit.validate(),
            Err(CoreError::InvalidField {
                field: "applied-as-commits",
                ..
            })
        ));
    }

    #[test]
    fn status_validate_rejects_invalid_mentions() {
        let status = StatusEvent {
            root_event: hex_of(0x11, 64),
            reply_events: Vec::new(),
            mentions: vec!["abcd".to_string()],
            repo_address: None,
            repo_refs: Vec::new(),
            applied_refs: Vec::new(),
            merge_commit: None,
            applied_as_commits: Vec::new(),
        };

        assert!(matches!(
            status.validate(),
            Err(CoreError::InvalidField { field: "p", .. })
        ));
    }

    #[test]
    fn status_validate_rejects_invalid_repo_address() {
        let status = StatusEvent {
            root_event: hex_of(0x11, 64),
            reply_events: Vec::new(),
            mentions: Vec::new(),
            repo_address: Some("bad".to_string()),
            repo_refs: Vec::new(),
            applied_refs: Vec::new(),
            merge_commit: None,
            applied_as_commits: Vec::new(),
        };

        assert!(matches!(
            status.validate(),
            Err(CoreError::InvalidField { field: "a", .. })
        ));
    }
}
