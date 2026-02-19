use crate::kinds::{KIND_GIT_REPO_ANNOUNCEMENT, KIND_GIT_REPO_STATE};
use crate::{RepoAnnouncement, RepoState};
use std::collections::HashSet;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NostrEvent {
    pub kind: u32,
    pub pubkey: String,
    pub created_at: i64,
    pub tags: Vec<Vec<String>>,
}

impl NostrEvent {
    pub fn new(
        kind: u32,
        pubkey: impl Into<String>,
        created_at: i64,
        tags: Vec<Vec<String>>,
    ) -> Self {
        Self {
            kind,
            pubkey: pubkey.into(),
            created_at,
            tags,
        }
    }
}

pub fn find_repo_announcement(
    events: &[NostrEvent],
    pubkey: &str,
    identifier: &str,
) -> Option<RepoAnnouncement> {
    events.iter().find_map(|event| {
        if event.kind != KIND_GIT_REPO_ANNOUNCEMENT.0 || event.pubkey != pubkey {
            return None;
        }
        let announcement = RepoAnnouncement::from_tags(&event.tags).ok()?;
        if announcement.identifier == identifier {
            Some(announcement)
        } else {
            None
        }
    })
}

pub fn collect_maintainers(events: &[NostrEvent], pubkey: &str, identifier: &str) -> Vec<String> {
    let mut checked = HashSet::new();
    collect_maintainers_inner(events, pubkey, identifier, &mut checked);
    let mut maintainers: Vec<String> = checked.into_iter().collect();
    maintainers.sort();
    maintainers
}

fn collect_maintainers_inner(
    events: &[NostrEvent],
    pubkey: &str,
    identifier: &str,
    checked: &mut HashSet<String>,
) {
    if checked.contains(pubkey) {
        return;
    }

    let Some(announcement) = find_repo_announcement(events, pubkey, identifier) else {
        return;
    };
    checked.insert(pubkey.to_string());

    for maintainer in announcement.maintainers {
        collect_maintainers_inner(events, &maintainer, &announcement.identifier, checked);
    }
}

pub fn latest_state_from_maintainers(
    events: &[NostrEvent],
    maintainers: &[String],
) -> Option<RepoState> {
    if maintainers.is_empty() {
        return None;
    }

    let maintainer_set: HashSet<&str> = maintainers.iter().map(String::as_str).collect();
    let latest = events
        .iter()
        .filter(|event| {
            event.kind == KIND_GIT_REPO_STATE.0 && maintainer_set.contains(event.pubkey.as_str())
        })
        .max_by_key(|event| event.created_at)?;

    RepoState::from_tags(&latest.tags).ok()
}

pub fn collect_clone_urls(
    events: &[NostrEvent],
    maintainers: &[String],
    identifier: &str,
) -> Vec<String> {
    if maintainers.is_empty() {
        return Vec::new();
    }

    let maintainer_set: HashSet<&str> = maintainers.iter().map(String::as_str).collect();
    let mut clones = Vec::new();

    for event in events {
        if event.kind != KIND_GIT_REPO_ANNOUNCEMENT.0 {
            continue;
        }
        if !maintainer_set.contains(event.pubkey.as_str()) {
            continue;
        }
        let Ok(announcement) = RepoAnnouncement::from_tags(&event.tags) else {
            continue;
        };
        if announcement.identifier != identifier {
            continue;
        }

        for clone in announcement.clone {
            let trimmed = clone.trim_end_matches('/').to_string();
            if trimmed.is_empty() {
                continue;
            }
            if !clones.iter().any(|existing| existing == &trimmed) {
                clones.push(trimmed);
            }
        }
    }

    clones
}

#[cfg(test)]
mod tests {
    use super::NostrEvent;
    use super::collect_clone_urls;
    use super::collect_maintainers;
    use super::find_repo_announcement;
    use super::latest_state_from_maintainers;
    use crate::kinds::{KIND_GIT_REPO_ANNOUNCEMENT, KIND_GIT_REPO_STATE};
    use crate::{RepoAnnouncement, RepoState};
    use std::collections::HashMap;

    fn hex_of(byte: u8, len: usize) -> String {
        format!("{:02x}", byte).repeat(len / 2)
    }

    fn announcement(identifier: &str, maintainers: Vec<String>) -> RepoAnnouncement {
        RepoAnnouncement {
            identifier: identifier.to_string(),
            name: None,
            description: None,
            root_commit: None,
            clone: vec!["https://gittr.ee/npub1example/repo.git".to_string()],
            web: Vec::new(),
            relays: vec!["wss://gittr.ee".to_string()],
            blossoms: Vec::new(),
            hashtags: Vec::new(),
            maintainers,
        }
    }

    fn state_with_commit(identifier: &str, commit: &str) -> RepoState {
        let mut state = HashMap::new();
        state.insert("HEAD".to_string(), "ref: refs/heads/main".to_string());
        state.insert("refs/heads/main".to_string(), commit.to_string());
        RepoState {
            identifier: identifier.to_string(),
            state,
        }
    }

    #[test]
    fn find_repo_announcement_matches_pubkey_and_identifier() {
        let pubkey = hex_of(0x11, 64);
        let tags = announcement("repo", Vec::new()).to_tags();
        let events = vec![NostrEvent::new(
            KIND_GIT_REPO_ANNOUNCEMENT.0,
            pubkey.clone(),
            0,
            tags,
        )];

        let found = find_repo_announcement(&events, &pubkey, "repo").expect("found");
        assert_eq!(found.identifier, "repo");
    }

    #[test]
    fn find_repo_announcement_returns_none_for_identifier_mismatch() {
        let pubkey = hex_of(0x11, 64);
        let events = vec![NostrEvent::new(
            KIND_GIT_REPO_ANNOUNCEMENT.0,
            pubkey.clone(),
            0,
            announcement("other", Vec::new()).to_tags(),
        )];

        let found = find_repo_announcement(&events, &pubkey, "repo");
        assert!(found.is_none());
    }

    #[test]
    fn find_repo_announcement_returns_none_for_invalid_tags() {
        let pubkey = hex_of(0x11, 64);
        let events = vec![NostrEvent::new(
            KIND_GIT_REPO_ANNOUNCEMENT.0,
            pubkey.clone(),
            0,
            vec![vec!["x".to_string(), "value".to_string()]],
        )];

        let found = find_repo_announcement(&events, &pubkey, "repo");
        assert!(found.is_none());
    }

    #[test]
    fn find_repo_announcement_skips_wrong_kind_and_pubkey() {
        let pubkey = hex_of(0x11, 64);
        let other_pubkey = hex_of(0x22, 64);
        let events = vec![
            NostrEvent::new(
                KIND_GIT_REPO_STATE.0,
                pubkey.clone(),
                0,
                announcement("repo", Vec::new()).to_tags(),
            ),
            NostrEvent::new(
                KIND_GIT_REPO_ANNOUNCEMENT.0,
                other_pubkey,
                0,
                announcement("repo", Vec::new()).to_tags(),
            ),
        ];

        let found = find_repo_announcement(&events, &pubkey, "repo");
        assert!(found.is_none());
    }

    #[test]
    fn collect_maintainers_recurses_over_maintainers() {
        let alice = hex_of(0x11, 64);
        let bob = hex_of(0x22, 64);
        let carol = hex_of(0x33, 64);

        let events = vec![
            NostrEvent::new(
                KIND_GIT_REPO_ANNOUNCEMENT.0,
                alice.clone(),
                0,
                announcement("repo", vec![bob.clone()]).to_tags(),
            ),
            NostrEvent::new(
                KIND_GIT_REPO_ANNOUNCEMENT.0,
                bob.clone(),
                0,
                announcement("repo", vec![carol.clone()]).to_tags(),
            ),
            NostrEvent::new(
                KIND_GIT_REPO_ANNOUNCEMENT.0,
                carol.clone(),
                0,
                announcement("repo", Vec::new()).to_tags(),
            ),
        ];

        let mut maintainers = collect_maintainers(&events, &alice, "repo");
        maintainers.sort();
        assert_eq!(maintainers, vec![alice, bob, carol]);
    }

    #[test]
    fn collect_maintainers_handles_cycles() {
        let alice = hex_of(0x11, 64);
        let bob = hex_of(0x22, 64);

        let events = vec![
            NostrEvent::new(
                KIND_GIT_REPO_ANNOUNCEMENT.0,
                alice.clone(),
                0,
                announcement("repo", vec![bob.clone()]).to_tags(),
            ),
            NostrEvent::new(
                KIND_GIT_REPO_ANNOUNCEMENT.0,
                bob.clone(),
                0,
                announcement("repo", vec![alice.clone()]).to_tags(),
            ),
        ];

        let mut maintainers = collect_maintainers(&events, &alice, "repo");
        maintainers.sort();
        assert_eq!(maintainers, vec![alice, bob]);
    }

    #[test]
    fn collect_maintainers_returns_empty_when_missing_announcement() {
        let alice = hex_of(0x11, 64);
        let events = Vec::new();
        let maintainers = collect_maintainers(&events, &alice, "repo");
        assert!(maintainers.is_empty());
    }

    #[test]
    fn latest_state_from_maintainers_picks_latest() {
        let alice = hex_of(0x11, 64);
        let mallory = hex_of(0x99, 64);
        let commit_old = hex_of(0xaa, 40);
        let commit_new = hex_of(0xbb, 40);

        let events = vec![
            NostrEvent::new(
                KIND_GIT_REPO_STATE.0,
                alice.clone(),
                10,
                state_with_commit("repo", &commit_old).to_tags(),
            ),
            NostrEvent::new(
                KIND_GIT_REPO_STATE.0,
                alice.clone(),
                20,
                state_with_commit("repo", &commit_new).to_tags(),
            ),
            NostrEvent::new(
                KIND_GIT_REPO_STATE.0,
                mallory,
                30,
                state_with_commit("repo", "0123456789abcdef0123456789abcdef01234567").to_tags(),
            ),
        ];

        let maintainers = vec![alice];
        let state = latest_state_from_maintainers(&events, &maintainers).expect("state");
        assert_eq!(
            state.state.get("refs/heads/main").expect("ref"),
            &commit_new
        );
    }

    #[test]
    fn latest_state_from_maintainers_returns_none_for_empty_maintainers() {
        let events = vec![NostrEvent::new(
            KIND_GIT_REPO_STATE.0,
            hex_of(0x11, 64),
            10,
            state_with_commit("repo", "0123456789abcdef0123456789abcdef01234567").to_tags(),
        )];
        assert!(latest_state_from_maintainers(&events, &[]).is_none());
    }

    #[test]
    fn latest_state_from_maintainers_returns_none_when_no_maintainer_state_matches() {
        let alice = hex_of(0x11, 64);
        let bob = hex_of(0x22, 64);
        let events = vec![NostrEvent::new(
            KIND_GIT_REPO_STATE.0,
            bob,
            10,
            state_with_commit("repo", "0123456789abcdef0123456789abcdef01234567").to_tags(),
        )];
        assert!(latest_state_from_maintainers(&events, &[alice]).is_none());
    }

    #[test]
    fn latest_state_from_maintainers_returns_none_for_invalid_state_tags() {
        let alice = hex_of(0x11, 64);
        let events = vec![NostrEvent::new(
            KIND_GIT_REPO_STATE.0,
            alice.clone(),
            10,
            vec![vec!["x".to_string(), "invalid".to_string()]],
        )];
        assert!(latest_state_from_maintainers(&events, &[alice]).is_none());
    }

    #[test]
    fn collect_clone_urls_filters_by_maintainers_and_identifier() {
        let alice = hex_of(0x11, 64);
        let bob = hex_of(0x22, 64);

        let events = vec![
            NostrEvent::new(
                KIND_GIT_REPO_ANNOUNCEMENT.0,
                alice.clone(),
                0,
                announcement("repo", vec![bob.clone()]).to_tags(),
            ),
            NostrEvent::new(
                KIND_GIT_REPO_ANNOUNCEMENT.0,
                bob.clone(),
                0,
                RepoAnnouncement {
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
                }
                .to_tags(),
            ),
            NostrEvent::new(
                KIND_GIT_REPO_ANNOUNCEMENT.0,
                bob.clone(),
                0,
                RepoAnnouncement {
                    identifier: "other".to_string(),
                    name: None,
                    description: None,
                    root_commit: None,
                    clone: vec!["https://git.example/other.git".to_string()],
                    web: Vec::new(),
                    relays: vec!["wss://gittr.ee".to_string()],
                    blossoms: Vec::new(),
                    hashtags: Vec::new(),
                    maintainers: Vec::new(),
                }
                .to_tags(),
            ),
        ];

        let clones = collect_clone_urls(&events, &[alice, bob], "repo");
        assert_eq!(
            clones,
            vec![
                "https://gittr.ee/npub1example/repo.git".to_string(),
                "https://git.example/repo.git".to_string(),
            ]
        );
    }

    #[test]
    fn collect_clone_urls_dedupes_and_trims() {
        let alice = hex_of(0x11, 64);
        let events = vec![NostrEvent::new(
            KIND_GIT_REPO_ANNOUNCEMENT.0,
            alice.clone(),
            0,
            RepoAnnouncement {
                identifier: "repo".to_string(),
                name: None,
                description: None,
                root_commit: None,
                clone: vec![
                    "https://git.example/repo.git/".to_string(),
                    "https://git.example/repo.git".to_string(),
                ],
                web: Vec::new(),
                relays: vec!["wss://gittr.ee".to_string()],
                blossoms: Vec::new(),
                hashtags: Vec::new(),
                maintainers: Vec::new(),
            }
            .to_tags(),
        )];

        let clones = collect_clone_urls(&events, &[alice], "repo");
        assert_eq!(clones, vec!["https://git.example/repo.git".to_string()]);
    }

    #[test]
    fn collect_clone_urls_skips_non_announcements_invalid_events_and_empty_clones() {
        let alice = hex_of(0x11, 64);
        let bob = hex_of(0x22, 64);
        let events = vec![
            NostrEvent::new(
                KIND_GIT_REPO_STATE.0,
                alice.clone(),
                0,
                state_with_commit("repo", "0123456789abcdef0123456789abcdef01234567").to_tags(),
            ),
            NostrEvent::new(
                KIND_GIT_REPO_ANNOUNCEMENT.0,
                bob,
                0,
                announcement("repo", Vec::new()).to_tags(),
            ),
            NostrEvent::new(
                KIND_GIT_REPO_ANNOUNCEMENT.0,
                alice.clone(),
                0,
                vec![vec!["x".to_string(), "y".to_string()]],
            ),
            NostrEvent::new(
                KIND_GIT_REPO_ANNOUNCEMENT.0,
                alice.clone(),
                0,
                RepoAnnouncement {
                    identifier: "repo".to_string(),
                    name: None,
                    description: None,
                    root_commit: None,
                    clone: vec!["/".to_string()],
                    web: Vec::new(),
                    relays: vec!["wss://gittr.ee".to_string()],
                    blossoms: Vec::new(),
                    hashtags: Vec::new(),
                    maintainers: Vec::new(),
                }
                .to_tags(),
            ),
            NostrEvent::new(
                KIND_GIT_REPO_ANNOUNCEMENT.0,
                alice.clone(),
                0,
                RepoAnnouncement {
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
                }
                .to_tags(),
            ),
        ];

        let clones = collect_clone_urls(&events, &[alice], "repo");
        assert_eq!(clones, vec!["https://git.example/repo.git".to_string()]);
    }

    #[test]
    fn collect_clone_urls_returns_empty_without_maintainers() {
        let events = vec![NostrEvent::new(
            KIND_GIT_REPO_ANNOUNCEMENT.0,
            hex_of(0x11, 64),
            0,
            announcement("repo", Vec::new()).to_tags(),
        )];

        let clones = collect_clone_urls(&events, &[], "repo");
        assert!(clones.is_empty());
    }
}
