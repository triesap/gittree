use crate::event_filters::{EventFilter, build_related_event_filters};
use crate::kinds::{KIND_GIT_REPO_ANNOUNCEMENT, KIND_GIT_REPO_STATE};
use crate::{CoreError, RepoAnnouncement};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdmissionDecision {
    Accept,
    Reject { reason: String },
    RequiresRelatedEvents { filters: Vec<EventFilter> },
}

pub fn evaluate_admission(
    kind: u32,
    pubkey: &str,
    event_id: &str,
    tags: &[Vec<String>],
    relay_host: Option<&str>,
) -> Result<AdmissionDecision, CoreError> {
    if kind == KIND_GIT_REPO_STATE.0 {
        return Ok(AdmissionDecision::Accept);
    }

    if kind == KIND_GIT_REPO_ANNOUNCEMENT.0 {
        let Some(host) = relay_host else {
            return Ok(AdmissionDecision::Reject {
                reason: "missing relay host for announcement check".to_string(),
            });
        };
        let announcement = RepoAnnouncement::from_tags(tags)?;
        if announcement.clone.is_empty() {
            return Ok(AdmissionDecision::Reject {
                reason: "repository announcement missing clone tags".to_string(),
            });
        }
        if announcement.relays.is_empty() {
            return Ok(AdmissionDecision::Reject {
                reason: "repository announcement missing relays tags".to_string(),
            });
        }
        if announcement.lists_grasp_host(host)? {
            Ok(AdmissionDecision::Accept)
        } else {
            Ok(AdmissionDecision::Reject {
                reason: "repository announcement does not list relay host".to_string(),
            })
        }
    } else {
        Ok(AdmissionDecision::RequiresRelatedEvents {
            filters: build_related_event_filters(kind, pubkey, event_id, tags),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::AdmissionDecision;
    use super::evaluate_admission;
    use crate::RepoAnnouncement;
    use crate::kinds::{KIND_GIT_PATCH, KIND_GIT_REPO_ANNOUNCEMENT, KIND_GIT_REPO_STATE};

    #[test]
    fn admission_accepts_state_events() {
        let decision =
            evaluate_admission(KIND_GIT_REPO_STATE.0, "pubkey", "event", &[], Some("relay"))
                .expect("decision");
        assert_eq!(decision, AdmissionDecision::Accept);
    }

    #[test]
    fn admission_accepts_announcement_when_host_listed() {
        let announcement = RepoAnnouncement {
            identifier: "repo".to_string(),
            name: None,
            description: None,
            root_commit: None,
            clone: vec!["https://git.example/repo.git".to_string()],
            web: Vec::new(),
            relays: vec!["wss://gittr.ee".to_string()],
            blossoms: Vec::new(),
            hashtags: Vec::new(),
            maintainers: Vec::new(),
        };
        let tags = announcement.to_tags();
        let decision = evaluate_admission(
            KIND_GIT_REPO_ANNOUNCEMENT.0,
            "pubkey",
            "event",
            &tags,
            Some("gittr.ee"),
        )
        .expect("decision");
        assert_eq!(decision, AdmissionDecision::Accept);
    }

    #[test]
    fn admission_rejects_announcement_without_clone_tags() {
        let announcement = RepoAnnouncement {
            identifier: "repo".to_string(),
            name: None,
            description: None,
            root_commit: None,
            clone: Vec::new(),
            web: Vec::new(),
            relays: vec!["wss://gittr.ee".to_string()],
            blossoms: Vec::new(),
            hashtags: Vec::new(),
            maintainers: Vec::new(),
        };
        let tags = announcement.to_tags();
        let decision = evaluate_admission(
            KIND_GIT_REPO_ANNOUNCEMENT.0,
            "pubkey",
            "event",
            &tags,
            Some("gittr.ee"),
        )
        .expect("decision");
        assert_eq!(
            decision,
            AdmissionDecision::Reject {
                reason: "repository announcement missing clone tags".to_string(),
            }
        );
    }

    #[test]
    fn admission_rejects_announcement_without_relays_tags() {
        let announcement = RepoAnnouncement {
            identifier: "repo".to_string(),
            name: None,
            description: None,
            root_commit: None,
            clone: vec!["https://git.example/repo.git".to_string()],
            web: Vec::new(),
            relays: Vec::new(),
            blossoms: Vec::new(),
            hashtags: Vec::new(),
            maintainers: Vec::new(),
        };
        let tags = announcement.to_tags();
        let decision = evaluate_admission(
            KIND_GIT_REPO_ANNOUNCEMENT.0,
            "pubkey",
            "event",
            &tags,
            Some("gittr.ee"),
        )
        .expect("decision");
        assert_eq!(
            decision,
            AdmissionDecision::Reject {
                reason: "repository announcement missing relays tags".to_string(),
            }
        );
    }

    #[test]
    fn admission_rejects_announcement_without_host() {
        let announcement = RepoAnnouncement {
            identifier: "repo".to_string(),
            name: None,
            description: None,
            root_commit: None,
            clone: vec!["https://relay.other.dev/npub1example/repo.git".to_string()],
            web: Vec::new(),
            relays: vec!["wss://relay.other.dev".to_string()],
            blossoms: Vec::new(),
            hashtags: Vec::new(),
            maintainers: Vec::new(),
        };
        let tags = announcement.to_tags();
        let decision = evaluate_admission(
            KIND_GIT_REPO_ANNOUNCEMENT.0,
            "pubkey",
            "event",
            &tags,
            Some("gittr.ee"),
        )
        .expect("decision");
        assert_eq!(
            decision,
            AdmissionDecision::Reject {
                reason: "repository announcement does not list relay host".to_string(),
            }
        );
    }

    #[test]
    fn admission_rejects_announcement_without_host_param() {
        let announcement = RepoAnnouncement {
            identifier: "repo".to_string(),
            name: None,
            description: None,
            root_commit: None,
            clone: vec!["https://gittr.ee/npub1example/repo.git".to_string()],
            web: Vec::new(),
            relays: vec!["wss://gittr.ee".to_string()],
            blossoms: Vec::new(),
            hashtags: Vec::new(),
            maintainers: Vec::new(),
        };
        let tags = announcement.to_tags();
        let decision =
            evaluate_admission(KIND_GIT_REPO_ANNOUNCEMENT.0, "pubkey", "event", &tags, None)
                .expect("decision");
        assert_eq!(
            decision,
            AdmissionDecision::Reject {
                reason: "missing relay host for announcement check".to_string(),
            }
        );
    }

    #[test]
    fn admission_requires_related_filters_for_other_kinds() {
        let expected_filters = crate::event_filters::build_related_event_filters(
            KIND_GIT_PATCH.0,
            "pubkey",
            "eventid",
            &[],
        );
        let decision =
            evaluate_admission(KIND_GIT_PATCH.0, "pubkey", "eventid", &[], Some("relay"))
                .expect("decision");
        assert_eq!(
            decision,
            AdmissionDecision::RequiresRelatedEvents {
                filters: expected_filters,
            }
        );
    }

    #[test]
    fn admission_rejects_announcement_with_invalid_tags_payload() {
        let err = evaluate_admission(
            KIND_GIT_REPO_ANNOUNCEMENT.0,
            "pubkey",
            "event",
            &[vec!["d".to_string()]],
            Some("gittr.ee"),
        )
        .unwrap_err();
        assert!(matches!(err, crate::CoreError::MissingField("d")));
    }

    #[test]
    fn admission_errors_for_invalid_relay_host_format() {
        let announcement = RepoAnnouncement {
            identifier: "repo".to_string(),
            name: None,
            description: None,
            root_commit: None,
            clone: vec!["https://git.example/repo.git".to_string()],
            web: Vec::new(),
            relays: vec!["wss://gittr.ee".to_string()],
            blossoms: Vec::new(),
            hashtags: Vec::new(),
            maintainers: Vec::new(),
        };
        let tags = announcement.to_tags();
        let err = evaluate_admission(
            KIND_GIT_REPO_ANNOUNCEMENT.0,
            "pubkey",
            "event",
            &tags,
            Some("://invalid"),
        )
        .unwrap_err();
        assert_eq!(err.to_string(), "invalid field grasp_url: ://invalid");
    }
}
