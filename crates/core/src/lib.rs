#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum CoreError {
    MissingField(&'static str),
    InvalidField { field: &'static str, value: String },
    InvalidTag { tag: &'static str, value: String },
}

pub type Result<T> = std::result::Result<T, CoreError>;

impl std::fmt::Display for CoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CoreError::MissingField(field) => write!(f, "missing required field: {field}"),
            CoreError::InvalidField { field, value } => {
                write!(f, "invalid field {field}: {value}")
            }
            CoreError::InvalidTag { tag, value } => write!(f, "invalid tag {tag}: {value}"),
        }
    }
}

impl std::error::Error for CoreError {}

pub mod admission;
pub mod control_events;
pub mod event_filters;
pub mod event_refs;
pub mod git_receive;
pub mod git_refs;
pub mod grasp;
pub mod kinds;
pub mod nip11;
pub mod nip34;
pub mod nip34_common;
pub mod nip34_events;
pub mod nip34_grasp_list;
pub mod nip34_issues;
pub mod nip34_patches;
pub mod nip34_proposals;
pub mod nip34_status;
pub mod relay_compat;
pub mod refs;
pub mod repo_events;
pub mod repo_mapping;
pub mod repo_path;
pub mod tags;

pub use admission::{AdmissionDecision, evaluate_admission};
pub use control_events::ControlAction;
pub use event_filters::{EventFilter, build_related_event_filters};
pub use event_refs::{
    EventReferences, collect_event_references, collect_event_references_with_self,
};
pub use git_receive::{RefUpdate, UpdateDecision, evaluate_ref_update, evaluate_updates};
pub use git_refs::{StateRefMatch, is_branch_ref, is_pr_branch_ref, is_tag_ref, match_state_ref};
pub use grasp::{
    extract_npub, format_grasp_server_url_as_blossom_url, format_grasp_server_url_as_clone_url,
    format_grasp_server_url_as_relay_url, is_grasp_server_clone_url, is_grasp_server_in_list,
    normalize_grasp_server_url,
};
pub use kinds::{
    KIND_GIT_ISSUE, KIND_GIT_PATCH, KIND_GIT_PULL_REQUEST, KIND_GIT_PULL_REQUEST_UPDATE,
    KIND_GIT_REPO_ANNOUNCEMENT, KIND_GIT_REPO_STATE, KIND_GIT_STATUS_APPLIED,
    KIND_GIT_STATUS_CLOSED, KIND_GIT_STATUS_DRAFT, KIND_GIT_STATUS_OPEN, KIND_USER_GRASP_LIST,
    KIND_GITTREE_CONTROL, NostrKind, is_nip34_kind, nip34_required_kinds, status_kinds,
};
pub use nip11::RelayInfoDocument;
pub use nip34::RepoAnnouncement;
pub use nip34::RepoState;
pub use nip34_events::Nip34Event;
pub use nip34_grasp_list::UserGraspList;
pub use nip34_issues::Issue;
pub use nip34_patches::{CommitterTag, Patch};
pub use nip34_proposals::{PullRequest, PullRequestUpdate};
pub use nip34_status::{StatusAppliedRef, StatusEvent};
pub use relay_compat::{
    ActiveProbeEvidence, RelayCapability, RelayCapabilitySet, RelayCompatibilityReport,
    capabilities_from_nip11, merge_active_probe_evidence,
};
pub use refs::{is_nostr_ref_name, parse_nostr_ref};
pub use repo_events::{
    NostrEvent, collect_clone_urls, collect_maintainers, find_repo_announcement,
    latest_state_from_maintainers,
};
pub use repo_mapping::{ForgejoRepo, RepoMapping};
pub use repo_path::{RepoPath, parse_repo_path};

#[cfg(test)]
mod tests {
    use super::CoreError;

    #[test]
    fn displays_missing_field() {
        let error = CoreError::MissingField("relays");
        assert_eq!(error.to_string(), "missing required field: relays");
    }

    #[test]
    fn displays_invalid_field() {
        let error = CoreError::InvalidField {
            field: "clone",
            value: "not-a-url".to_string(),
        };
        assert_eq!(error.to_string(), "invalid field clone: not-a-url");
    }

    #[test]
    fn displays_invalid_tag() {
        let error = CoreError::InvalidTag {
            tag: "e",
            value: "missing-id".to_string(),
        };
        assert_eq!(error.to_string(), "invalid tag e: missing-id");
    }

    #[test]
    fn core_error_exposes_no_source() {
        let error = CoreError::MissingField("identifier");
        let dyn_error: &dyn std::error::Error = &error;
        assert!(dyn_error.source().is_none());
    }
}
