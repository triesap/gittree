#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NostrKind(pub u32);

pub const KIND_GIT_REPO_ANNOUNCEMENT: NostrKind = NostrKind(30617);
pub const KIND_GIT_REPO_STATE: NostrKind = NostrKind(30618);
pub const KIND_GIT_PATCH: NostrKind = NostrKind(1617);
pub const KIND_GIT_PULL_REQUEST: NostrKind = NostrKind(1618);
pub const KIND_GIT_PULL_REQUEST_UPDATE: NostrKind = NostrKind(1619);
pub const KIND_GIT_ISSUE: NostrKind = NostrKind(1621);
pub const KIND_GIT_STATUS_OPEN: NostrKind = NostrKind(1630);
pub const KIND_GIT_STATUS_APPLIED: NostrKind = NostrKind(1631);
pub const KIND_GIT_STATUS_CLOSED: NostrKind = NostrKind(1632);
pub const KIND_GIT_STATUS_DRAFT: NostrKind = NostrKind(1633);
pub const KIND_USER_GRASP_LIST: NostrKind = NostrKind(10317);

pub fn status_kinds() -> [NostrKind; 4] {
    [
        KIND_GIT_STATUS_OPEN,
        KIND_GIT_STATUS_APPLIED,
        KIND_GIT_STATUS_CLOSED,
        KIND_GIT_STATUS_DRAFT,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_constants_match_nip34() {
        assert_eq!(KIND_GIT_REPO_ANNOUNCEMENT.0, 30617);
        assert_eq!(KIND_GIT_REPO_STATE.0, 30618);
        assert_eq!(KIND_GIT_PATCH.0, 1617);
        assert_eq!(KIND_GIT_PULL_REQUEST.0, 1618);
        assert_eq!(KIND_GIT_PULL_REQUEST_UPDATE.0, 1619);
        assert_eq!(KIND_GIT_ISSUE.0, 1621);
        assert_eq!(KIND_GIT_STATUS_OPEN.0, 1630);
        assert_eq!(KIND_GIT_STATUS_APPLIED.0, 1631);
        assert_eq!(KIND_GIT_STATUS_CLOSED.0, 1632);
        assert_eq!(KIND_GIT_STATUS_DRAFT.0, 1633);
        assert_eq!(KIND_USER_GRASP_LIST.0, 10317);
    }

    #[test]
    fn status_kinds_returns_expected_set() {
        assert_eq!(
            status_kinds(),
            [
                KIND_GIT_STATUS_OPEN,
                KIND_GIT_STATUS_APPLIED,
                KIND_GIT_STATUS_CLOSED,
                KIND_GIT_STATUS_DRAFT
            ]
        );
    }
}
