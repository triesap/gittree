use crate::RepoState;

const HEADS_PREFIX: &str = "refs/heads/";
const TAGS_PREFIX: &str = "refs/tags/";
const PR_PREFIX: &str = "refs/heads/pr/";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StateRefMatch {
    Match,
    Missing,
    Mismatch { expected: String },
    Unsupported,
}

pub fn is_branch_ref(name: &str) -> bool {
    name.starts_with(HEADS_PREFIX)
}

pub fn is_tag_ref(name: &str) -> bool {
    name.starts_with(TAGS_PREFIX)
}

pub fn is_pr_branch_ref(name: &str) -> bool {
    name.starts_with(PR_PREFIX)
}

pub fn match_state_ref(ref_name: &str, new_rev: &str, state: &RepoState) -> StateRefMatch {
    if !is_branch_ref(ref_name) && !is_tag_ref(ref_name) {
        return StateRefMatch::Unsupported;
    }

    match state.state.get(ref_name) {
        Some(expected) if expected == new_rev => StateRefMatch::Match,
        Some(expected) => StateRefMatch::Mismatch {
            expected: expected.clone(),
        },
        None => StateRefMatch::Missing,
    }
}

#[cfg(test)]
mod tests {
    use super::is_branch_ref;
    use super::is_pr_branch_ref;
    use super::is_tag_ref;
    use super::match_state_ref;
    use super::StateRefMatch;
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

    #[test]
    fn branch_and_tag_detection() {
        assert!(is_branch_ref("refs/heads/main"));
        assert!(is_tag_ref("refs/tags/v1.0.0"));
        assert!(is_pr_branch_ref("refs/heads/pr/123"));
        assert!(!is_branch_ref("refs/notes/1"));
    }

    #[test]
    fn match_state_ref_matches_branch() {
        let state = sample_state();
        let result = match_state_ref(
            "refs/heads/main",
            "0123456789abcdef0123456789abcdef01234567",
            &state,
        );
        assert_eq!(result, StateRefMatch::Match);
    }

    #[test]
    fn match_state_ref_reports_mismatch() {
        let state = sample_state();
        let result = match_state_ref("refs/heads/main", "bad", &state);
        assert_eq!(
            result,
            StateRefMatch::Mismatch {
                expected: "0123456789abcdef0123456789abcdef01234567".to_string()
            }
        );
    }

    #[test]
    fn match_state_ref_reports_missing() {
        let state = sample_state();
        let result = match_state_ref("refs/heads/dev", "deadbeef", &state);
        assert_eq!(result, StateRefMatch::Missing);
    }

    #[test]
    fn match_state_ref_reports_unsupported() {
        let state = sample_state();
        let result = match_state_ref("refs/notes/review", "deadbeef", &state);
        assert_eq!(result, StateRefMatch::Unsupported);
    }
}
