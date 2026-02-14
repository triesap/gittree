use crate::RepoState;
use crate::git_refs::{StateRefMatch, is_pr_branch_ref, match_state_ref};
use crate::refs::is_nostr_ref_name;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefUpdate<'a> {
    pub old_rev: &'a str,
    pub new_rev: &'a str,
    pub ref_name: &'a str,
}

impl<'a> RefUpdate<'a> {
    pub fn new(old_rev: &'a str, new_rev: &'a str, ref_name: &'a str) -> Self {
        Self {
            old_rev,
            new_rev,
            ref_name,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateDecision {
    Accept,
    Reject { reason: String },
}

pub fn evaluate_ref_update(update: &RefUpdate<'_>, state: Option<&RepoState>) -> UpdateDecision {
    if update.ref_name.starts_with("refs/nostr/") {
        return if is_nostr_ref_name(update.ref_name) {
            UpdateDecision::Accept
        } else {
            UpdateDecision::Reject {
                reason: "refs/nostr/<event-id> must use a valid event id".to_string(),
            }
        };
    }

    let Some(state) = state else {
        return UpdateDecision::Reject {
            reason: "state event not available; cannot validate non-nostr refs".to_string(),
        };
    };

    if let Err(err) = state.validate() {
        return UpdateDecision::Reject {
            reason: format!("invalid repo state: {err}"),
        };
    }

    if is_pr_branch_ref(update.ref_name) {
        return UpdateDecision::Reject {
            reason: "refs/heads/pr/* must be pushed via nostr".to_string(),
        };
    }

    match match_state_ref(update.ref_name, update.new_rev, state) {
        StateRefMatch::Match => UpdateDecision::Accept,
        StateRefMatch::Missing => UpdateDecision::Reject {
            reason: format!(
                "{ref_name} not found in nostr state event",
                ref_name = update.ref_name
            ),
        },
        StateRefMatch::Mismatch { expected } => UpdateDecision::Reject {
            reason: format!(
                "cannot push {ref_name} to {new} as nostr state event is at {expected}",
                ref_name = update.ref_name,
                new = short_hash(update.new_rev),
                expected = short_hash(&expected),
            ),
        },
        StateRefMatch::Unsupported => UpdateDecision::Reject {
            reason: format!("unsupported ref {ref_name}", ref_name = update.ref_name),
        },
    }
}

pub fn evaluate_updates(updates: &[RefUpdate<'_>], state: Option<&RepoState>) -> UpdateDecision {
    let validated_state = match state {
        Some(state) => {
            if let Err(err) = state.validate() {
                return UpdateDecision::Reject {
                    reason: format!("invalid repo state: {err}"),
                };
            }
            Some(state)
        }
        None => None,
    };

    for update in updates {
        let decision = evaluate_ref_update(update, validated_state);
        if matches!(decision, UpdateDecision::Reject { .. }) {
            return decision;
        }
    }

    UpdateDecision::Accept
}

fn short_hash(value: &str) -> String {
    let len = value.len().min(7);
    value[..len].to_string()
}

#[cfg(test)]
mod tests {
    use super::RefUpdate;
    use super::UpdateDecision;
    use super::evaluate_ref_update;
    use super::evaluate_updates;
    use crate::RepoState;
    use std::collections::HashMap;

    fn sample_state() -> RepoState {
        let mut state = HashMap::new();
        state.insert(
            "refs/heads/main".to_string(),
            "0123456789abcdef0123456789abcdef01234567".to_string(),
        );
        state.insert("HEAD".to_string(), "ref: refs/heads/main".to_string());
        RepoState {
            identifier: "repo".to_string(),
            state,
        }
    }

    fn invalid_state_missing_head() -> RepoState {
        let mut state = HashMap::new();
        state.insert(
            "refs/heads/main".to_string(),
            "0123456789abcdef0123456789abcdef01234567".to_string(),
        );
        RepoState {
            identifier: "repo".to_string(),
            state,
        }
    }

    #[test]
    fn accepts_valid_nostr_ref_without_state() {
        let update = RefUpdate::new(
            "0000000000000000000000000000000000000000",
            "1111111111111111111111111111111111111111",
            "refs/nostr/0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        );
        let decision = evaluate_ref_update(&update, None);
        assert_eq!(decision, UpdateDecision::Accept);
    }

    #[test]
    fn rejects_invalid_nostr_ref() {
        let update = RefUpdate::new(
            "0000000000000000000000000000000000000000",
            "1111111111111111111111111111111111111111",
            "refs/nostr/invalid",
        );
        let decision = evaluate_ref_update(&update, None);
        assert_ne!(decision, UpdateDecision::Accept);
    }

    #[test]
    fn rejects_non_nostr_when_state_missing() {
        let update = RefUpdate::new(
            "0000000000000000000000000000000000000000",
            "0123456789abcdef0123456789abcdef01234567",
            "refs/heads/main",
        );
        let decision = evaluate_ref_update(&update, None);
        assert_ne!(decision, UpdateDecision::Accept);
    }

    #[test]
    fn rejects_pr_branch_ref() {
        let update = RefUpdate::new(
            "0000000000000000000000000000000000000000",
            "0123456789abcdef0123456789abcdef01234567",
            "refs/heads/pr/123",
        );
        let decision = evaluate_ref_update(&update, Some(&sample_state()));
        assert_ne!(decision, UpdateDecision::Accept);
    }

    #[test]
    fn rejects_when_state_event_is_invalid() {
        let update = RefUpdate::new(
            "0000000000000000000000000000000000000000",
            "0123456789abcdef0123456789abcdef01234567",
            "refs/heads/main",
        );
        let decision = evaluate_ref_update(&update, Some(&invalid_state_missing_head()));
        match decision {
            UpdateDecision::Reject { reason } => {
                assert!(reason.contains("invalid repo state"));
                assert!(reason.contains("HEAD"));
            }
            UpdateDecision::Accept => panic!("expected invalid-state rejection"),
        }
    }

    #[test]
    fn accepts_branch_when_matches_state() {
        let update = RefUpdate::new(
            "0000000000000000000000000000000000000000",
            "0123456789abcdef0123456789abcdef01234567",
            "refs/heads/main",
        );
        let decision = evaluate_ref_update(&update, Some(&sample_state()));
        assert_eq!(decision, UpdateDecision::Accept);
    }

    #[test]
    fn rejects_branch_when_mismatch() {
        let update = RefUpdate::new(
            "0000000000000000000000000000000000000000",
            "1111111111111111111111111111111111111111",
            "refs/heads/main",
        );
        let decision = evaluate_ref_update(&update, Some(&sample_state()));
        match decision {
            UpdateDecision::Reject { reason } => {
                assert!(reason.contains("cannot push refs/heads/main"));
                assert!(reason.contains("1111111"));
                assert!(reason.contains("0123456"));
            }
            UpdateDecision::Accept => panic!("expected rejection"),
        }
    }

    #[test]
    fn rejects_missing_ref() {
        let update = RefUpdate::new(
            "0000000000000000000000000000000000000000",
            "0123456789abcdef0123456789abcdef01234567",
            "refs/heads/dev",
        );
        let decision = evaluate_ref_update(&update, Some(&sample_state()));
        assert_ne!(decision, UpdateDecision::Accept);
    }

    #[test]
    fn rejects_unsupported_ref() {
        let update = RefUpdate::new(
            "0000000000000000000000000000000000000000",
            "0123456789abcdef0123456789abcdef01234567",
            "refs/notes/review",
        );
        let decision = evaluate_ref_update(&update, Some(&sample_state()));
        assert_ne!(decision, UpdateDecision::Accept);
    }

    #[test]
    fn evaluate_updates_returns_first_rejection() {
        let updates = vec![
            RefUpdate::new(
                "0000000000000000000000000000000000000000",
                "0123456789abcdef0123456789abcdef01234567",
                "refs/heads/main",
            ),
            RefUpdate::new(
                "0000000000000000000000000000000000000000",
                "1111111111111111111111111111111111111111",
                "refs/heads/main",
            ),
        ];

        let decision = evaluate_updates(&updates, Some(&sample_state()));
        assert_ne!(decision, UpdateDecision::Accept);
    }

    #[test]
    fn evaluate_updates_rejects_invalid_state_before_processing_updates() {
        let updates = vec![RefUpdate::new(
            "0000000000000000000000000000000000000000",
            "0123456789abcdef0123456789abcdef01234567",
            "refs/heads/main",
        )];

        let decision = evaluate_updates(&updates, Some(&invalid_state_missing_head()));
        match decision {
            UpdateDecision::Reject { reason } => {
                assert!(reason.contains("invalid repo state"));
                assert!(reason.contains("HEAD"));
            }
            UpdateDecision::Accept => panic!("expected invalid-state rejection"),
        }
    }

    #[test]
    fn evaluate_updates_accepts_empty_update_list_without_state() {
        let updates: Vec<RefUpdate<'static>> = Vec::new();
        let decision = evaluate_updates(&updates, None);
        assert_eq!(decision, UpdateDecision::Accept);
    }

    #[test]
    fn evaluate_updates_accepts_when_all_updates_match_state() {
        let updates = vec![RefUpdate::new(
            "0000000000000000000000000000000000000000",
            "0123456789abcdef0123456789abcdef01234567",
            "refs/heads/main",
        )];

        let decision = evaluate_updates(&updates, Some(&sample_state()));
        assert_eq!(decision, UpdateDecision::Accept);
    }
}
