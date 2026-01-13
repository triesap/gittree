use crate::kinds::{
    KIND_GIT_ISSUE, KIND_GIT_PATCH, KIND_GIT_PULL_REQUEST, KIND_GIT_PULL_REQUEST_UPDATE,
    KIND_GIT_REPO_ANNOUNCEMENT, KIND_GIT_REPO_STATE, KIND_GIT_STATUS_APPLIED, KIND_GIT_STATUS_CLOSED,
    KIND_GIT_STATUS_DRAFT, KIND_GIT_STATUS_OPEN, KIND_USER_GRASP_LIST, NostrKind,
};
use crate::{
    CoreError, Issue, Patch, PullRequest, PullRequestUpdate, RepoAnnouncement, RepoState,
    StatusEvent, UserGraspList,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Nip34Event {
    RepoAnnouncement(RepoAnnouncement),
    RepoState(RepoState),
    Patch(Patch),
    PullRequest(PullRequest),
    PullRequestUpdate(PullRequestUpdate),
    Issue(Issue),
    Status { kind: NostrKind, event: StatusEvent },
    UserGraspList(UserGraspList),
}

impl Nip34Event {
    pub fn parse(kind: u32, tags: &[Vec<String>]) -> Result<Self, CoreError> {
        match kind {
            k if k == KIND_GIT_REPO_ANNOUNCEMENT.0 => {
                Ok(Nip34Event::RepoAnnouncement(RepoAnnouncement::from_tags(tags)?))
            }
            k if k == KIND_GIT_REPO_STATE.0 => {
                Ok(Nip34Event::RepoState(RepoState::from_tags(tags)?))
            }
            k if k == KIND_GIT_PATCH.0 => Ok(Nip34Event::Patch(Patch::from_tags(tags)?)),
            k if k == KIND_GIT_PULL_REQUEST.0 => Ok(Nip34Event::PullRequest(
                PullRequest::from_tags(tags)?,
            )),
            k if k == KIND_GIT_PULL_REQUEST_UPDATE.0 => Ok(Nip34Event::PullRequestUpdate(
                PullRequestUpdate::from_tags(tags)?,
            )),
            k if k == KIND_GIT_ISSUE.0 => Ok(Nip34Event::Issue(Issue::from_tags(tags)?)),
            k if is_status_kind(k) => Ok(Nip34Event::Status {
                kind: NostrKind(k),
                event: StatusEvent::from_tags(tags)?,
            }),
            k if k == KIND_USER_GRASP_LIST.0 => {
                Ok(Nip34Event::UserGraspList(UserGraspList::from_tags(tags)?))
            }
            _ => Err(CoreError::InvalidField {
                field: "kind",
                value: kind.to_string(),
            }),
        }
    }

    pub fn parse_validated(kind: u32, tags: &[Vec<String>]) -> Result<Self, CoreError> {
        let event = Self::parse(kind, tags)?;
        event.validate()?;
        Ok(event)
    }

    pub fn kind(&self) -> NostrKind {
        match self {
            Nip34Event::RepoAnnouncement(_) => KIND_GIT_REPO_ANNOUNCEMENT,
            Nip34Event::RepoState(_) => KIND_GIT_REPO_STATE,
            Nip34Event::Patch(_) => KIND_GIT_PATCH,
            Nip34Event::PullRequest(_) => KIND_GIT_PULL_REQUEST,
            Nip34Event::PullRequestUpdate(_) => KIND_GIT_PULL_REQUEST_UPDATE,
            Nip34Event::Issue(_) => KIND_GIT_ISSUE,
            Nip34Event::Status { kind, .. } => *kind,
            Nip34Event::UserGraspList(_) => KIND_USER_GRASP_LIST,
        }
    }

    pub fn to_tags(&self) -> Vec<Vec<String>> {
        match self {
            Nip34Event::RepoAnnouncement(event) => event.to_tags(),
            Nip34Event::RepoState(event) => event.to_tags(),
            Nip34Event::Patch(event) => event.to_tags(),
            Nip34Event::PullRequest(event) => event.to_tags(),
            Nip34Event::PullRequestUpdate(event) => event.to_tags(),
            Nip34Event::Issue(event) => event.to_tags(),
            Nip34Event::Status { event, .. } => event.to_tags(),
            Nip34Event::UserGraspList(event) => event.to_tags(),
        }
    }

    pub fn validate(&self) -> Result<(), CoreError> {
        match self {
            Nip34Event::RepoAnnouncement(event) => event.validate(),
            Nip34Event::RepoState(event) => event.validate(),
            Nip34Event::Patch(event) => event.validate(),
            Nip34Event::PullRequest(event) => event.validate(),
            Nip34Event::PullRequestUpdate(event) => event.validate(),
            Nip34Event::Issue(event) => event.validate(),
            Nip34Event::Status { event, .. } => event.validate(),
            Nip34Event::UserGraspList(event) => event.validate(),
        }
    }
}

fn is_status_kind(kind: u32) -> bool {
    kind == KIND_GIT_STATUS_OPEN.0
        || kind == KIND_GIT_STATUS_APPLIED.0
        || kind == KIND_GIT_STATUS_CLOSED.0
        || kind == KIND_GIT_STATUS_DRAFT.0
}

#[cfg(test)]
mod tests {
    use super::Nip34Event;
    use crate::kinds::{
        KIND_GIT_PULL_REQUEST, KIND_GIT_REPO_ANNOUNCEMENT, KIND_GIT_STATUS_OPEN,
        KIND_USER_GRASP_LIST,
    };
    use crate::{PullRequest, RepoAnnouncement, StatusEvent, UserGraspList};

    fn hex_of(byte: u8, len: usize) -> String {
        format!("{:02x}", byte).repeat(len / 2)
    }

    #[test]
    fn parses_repo_announcement_by_kind() {
        let announcement = RepoAnnouncement {
            identifier: "repo".to_string(),
            name: None,
            description: None,
            root_commit: None,
            clone: vec!["https://git.example/repo.git".to_string()],
            web: Vec::new(),
            relays: vec!["wss://relay.example".to_string()],
            blossoms: Vec::new(),
            hashtags: Vec::new(),
            maintainers: Vec::new(),
        };
        let tags = announcement.to_tags();
        let event =
            Nip34Event::parse(KIND_GIT_REPO_ANNOUNCEMENT.0, &tags).expect("parse");
        assert_eq!(event.kind(), KIND_GIT_REPO_ANNOUNCEMENT);
        assert!(matches!(event, Nip34Event::RepoAnnouncement(_)));
    }

    #[test]
    fn parses_pull_request_by_kind() {
        let pubkey = hex_of(0x11, 64);
        let pr = PullRequest {
            repo_address: format!("30617:{pubkey}:repo"),
            repo_refs: Vec::new(),
            mentions: Vec::new(),
            subject: None,
            labels: Vec::new(),
            tip_commit: hex_of(0x22, 40),
            clone: vec!["https://git.example/repo.git".to_string()],
            branch_name: None,
            revision_of: None,
            merge_base: None,
        };
        let tags = pr.to_tags();
        let event = Nip34Event::parse(KIND_GIT_PULL_REQUEST.0, &tags).expect("parse");
        assert_eq!(event.kind(), KIND_GIT_PULL_REQUEST);
        assert!(matches!(event, Nip34Event::PullRequest(_)));
    }

    #[test]
    fn parses_status_by_kind() {
        let status = StatusEvent {
            root_event: hex_of(0x33, 64),
            reply_events: Vec::new(),
            mentions: Vec::new(),
            repo_address: None,
            repo_refs: Vec::new(),
            applied_refs: Vec::new(),
            merge_commit: None,
            applied_as_commits: Vec::new(),
        };
        let tags = status.to_tags();
        let event = Nip34Event::parse(KIND_GIT_STATUS_OPEN.0, &tags).expect("parse");
        assert_eq!(event.kind(), KIND_GIT_STATUS_OPEN);
        assert!(matches!(event, Nip34Event::Status { .. }));
    }

    #[test]
    fn parses_grasp_list_by_kind() {
        let list = UserGraspList {
            urls: vec!["wss://relay.example".to_string()],
        };
        let tags = list.to_tags();
        let event = Nip34Event::parse(KIND_USER_GRASP_LIST.0, &tags).expect("parse");
        assert_eq!(event.kind(), KIND_USER_GRASP_LIST);
        assert!(matches!(event, Nip34Event::UserGraspList(_)));
    }

    #[test]
    fn rejects_unknown_kind() {
        let err = Nip34Event::parse(9999, &[]).unwrap_err();
        assert!(matches!(err, crate::CoreError::InvalidField { field: "kind", .. }));
    }

    #[test]
    fn parse_validated_rejects_invalid_announcement() {
        let announcement = RepoAnnouncement {
            identifier: "repo".to_string(),
            name: None,
            description: None,
            root_commit: None,
            clone: Vec::new(),
            web: Vec::new(),
            relays: vec!["wss://relay.example".to_string()],
            blossoms: Vec::new(),
            hashtags: Vec::new(),
            maintainers: Vec::new(),
        };
        let tags = announcement.to_tags();
        let err =
            Nip34Event::parse_validated(KIND_GIT_REPO_ANNOUNCEMENT.0, &tags).unwrap_err();
        assert!(matches!(err, crate::CoreError::MissingField("clone")));
    }

    #[test]
    fn parse_validated_accepts_grasp_list() {
        let list = UserGraspList {
            urls: vec!["wss://relay.example".to_string()],
        };
        let tags = list.to_tags();
        let event = Nip34Event::parse_validated(KIND_USER_GRASP_LIST.0, &tags).expect("parse");
        assert!(matches!(event, Nip34Event::UserGraspList(_)));
    }
}
