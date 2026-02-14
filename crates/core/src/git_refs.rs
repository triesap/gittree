use crate::RepoState;

const HEADS_PREFIX: &str = "refs/heads/";
const TAGS_PREFIX: &str = "refs/tags/";
const PR_PREFIX: &str = "refs/heads/pr/";
const HEAD_REF: &str = "HEAD";

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

pub fn is_head_ref(name: &str) -> bool {
    name == HEAD_REF
}

pub fn match_state_ref(ref_name: &str, new_rev: &str, state: &RepoState) -> StateRefMatch {
    if !is_branch_ref(ref_name) && !is_tag_ref(ref_name) && !is_head_ref(ref_name) {
        return StateRefMatch::Unsupported;
    }

    let expected = match state.state.get(ref_name) {
        Some(expected) => expected,
        None => return StateRefMatch::Missing,
    };

    match resolve_ref_value(expected, &state.state) {
        Some(value) if value == new_rev => StateRefMatch::Match,
        Some(value) => StateRefMatch::Mismatch { expected: value },
        None => StateRefMatch::Missing,
    }
}

fn resolve_ref_value<'a>(
    value: &'a str,
    state: &'a std::collections::HashMap<String, String>,
) -> Option<String> {
    if let Some(target) = value.strip_prefix("ref: ") {
        state.get(target).cloned()
    } else {
        Some(value.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::StateRefMatch;
    use super::is_branch_ref;
    use super::is_head_ref;
    use super::is_pr_branch_ref;
    use super::is_tag_ref;
    use super::match_state_ref;
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
        assert!(is_head_ref("HEAD"));
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
    fn match_state_ref_follows_symbolic_ref() {
        let mut state = std::collections::HashMap::new();
        state.insert("HEAD".to_string(), "ref: refs/heads/main".to_string());
        state.insert(
            "refs/heads/main".to_string(),
            "0123456789abcdef0123456789abcdef01234567".to_string(),
        );
        let repo_state = RepoState {
            identifier: "repo".to_string(),
            state,
        };
        let result = match_state_ref(
            "HEAD",
            "0123456789abcdef0123456789abcdef01234567",
            &repo_state,
        );
        assert_eq!(result, StateRefMatch::Match);
    }

    #[test]
    fn match_state_ref_reports_missing_for_unresolved_symbolic_ref() {
        let mut state = std::collections::HashMap::new();
        state.insert("HEAD".to_string(), "ref: refs/heads/main".to_string());
        let repo_state = RepoState {
            identifier: "repo".to_string(),
            state,
        };
        let result = match_state_ref("HEAD", "deadbeef", &repo_state);
        assert_eq!(result, StateRefMatch::Missing);
    }

    #[test]
    fn match_state_ref_reports_unsupported() {
        let state = sample_state();
        let result = match_state_ref("refs/notes/review", "deadbeef", &state);
        assert_eq!(result, StateRefMatch::Unsupported);
    }
}
