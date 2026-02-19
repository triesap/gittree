use crate::grasp::{extract_npub, normalize_grasp_server_url};
use crate::tags::{extend_unique, join_tag_values, push_unique};
use crate::{CoreError, Result};
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoAnnouncement {
    pub identifier: String,
    pub name: Option<String>,
    pub description: Option<String>,
    pub root_commit: Option<String>,
    pub clone: Vec<String>,
    pub web: Vec<String>,
    pub relays: Vec<String>,
    pub blossoms: Vec<String>,
    pub hashtags: Vec<String>,
    pub maintainers: Vec<String>,
}

impl RepoAnnouncement {
    pub fn from_tags(tags: &[Vec<String>]) -> Result<Self> {
        let mut identifier = None;
        let mut name = None;
        let mut description = None;
        let mut root_commit = None;
        let mut clone = Vec::new();
        let mut web = Vec::new();
        let mut relays = Vec::new();
        let mut blossoms = Vec::new();
        let mut hashtags = Vec::new();
        let mut maintainers = Vec::new();

        for tag in tags {
            match tag.as_slice() {
                [t, id, ..] if t == "d" => identifier = Some(id.clone()),
                [t, value, ..] if t == "name" => name = Some(value.clone()),
                [t, value, ..] if t == "description" => description = Some(value.clone()),
                [t, commit] if t == "r" => root_commit = Some(commit.clone()),
                [t, commit, marker] if t == "r" && marker == "euc" => {
                    root_commit = Some(commit.clone())
                }
                [t, values @ ..] if t == "clone" => extend_unique(&mut clone, values),
                [t, values @ ..] if t == "web" => extend_unique(&mut web, values),
                [t, values @ ..] if t == "relays" => extend_unique(&mut relays, values),
                [t, values @ ..] if t == "blossoms" => extend_unique(&mut blossoms, values),
                [t, values @ ..] if t == "maintainers" => extend_unique(&mut maintainers, values),
                [t, value, ..] if t == "t" => push_unique(&mut hashtags, value),
                _ => {}
            }
        }

        let identifier = identifier.ok_or(CoreError::MissingField("d"))?;

        Ok(Self {
            identifier,
            name,
            description,
            root_commit,
            clone,
            web,
            relays,
            blossoms,
            hashtags,
            maintainers,
        })
    }

    pub fn to_tags(&self) -> Vec<Vec<String>> {
        let mut tags = Vec::new();

        tags.push(vec!["d".to_string(), self.identifier.clone()]);

        if let Some(name) = &self.name {
            tags.push(vec!["name".to_string(), name.clone()]);
        }

        if let Some(description) = &self.description {
            tags.push(vec!["description".to_string(), description.clone()]);
        }

        if let Some(commit) = &self.root_commit {
            tags.push(vec!["r".to_string(), commit.clone(), "euc".to_string()]);
        }

        if !self.clone.is_empty() {
            tags.push(join_tag_values("clone", &self.clone));
        }

        if !self.web.is_empty() {
            tags.push(join_tag_values("web", &self.web));
        }

        if !self.relays.is_empty() {
            tags.push(join_tag_values("relays", &self.relays));
        }

        if !self.blossoms.is_empty() {
            tags.push(join_tag_values("blossoms", &self.blossoms));
        }

        if !self.maintainers.is_empty() {
            tags.push(join_tag_values("maintainers", &self.maintainers));
        }

        for hashtag in &self.hashtags {
            tags.push(vec!["t".to_string(), hashtag.clone()]);
        }

        tags
    }

    pub fn lists_grasp_host(&self, host: &str) -> Result<bool> {
        let host = normalize_grasp_host_for_compare(host)?;
        let listed_in_relays = self
            .relays
            .iter()
            .filter_map(|url| normalize_grasp_host_for_compare(url).ok())
            .any(|normalized| normalized == host);
        Ok(listed_in_relays)
    }

    pub fn grasp_servers(&self) -> Vec<String> {
        detect_grasp_servers(&self.clone, &self.relays, &self.identifier)
    }

    pub fn maintainer_keys(&self) -> Vec<String> {
        if self.maintainers.is_empty() {
            Vec::new()
        } else {
            self.maintainers.clone()
        }
    }

    pub fn validate(&self) -> Result<()> {
        if self.identifier.trim().is_empty() {
            return Err(CoreError::MissingField("d"));
        }

        if let Some(commit) = &self.root_commit {
            if !is_hex40(commit) {
                return Err(CoreError::InvalidField {
                    field: "r",
                    value: commit.clone(),
                });
            }
        }

        if self.clone.is_empty() {
            return Err(CoreError::MissingField("clone"));
        }

        for clone in &self.clone {
            if clone.trim().is_empty() {
                return Err(CoreError::InvalidField {
                    field: "clone",
                    value: clone.clone(),
                });
            }
        }

        if self.relays.is_empty() {
            return Err(CoreError::MissingField("relays"));
        }

        for relay in &self.relays {
            if relay.trim().is_empty() {
                return Err(CoreError::InvalidField {
                    field: "relays",
                    value: relay.clone(),
                });
            }
        }

        for maintainer in &self.maintainers {
            if !is_hex64(maintainer) {
                return Err(CoreError::InvalidField {
                    field: "maintainers",
                    value: maintainer.clone(),
                });
            }
        }

        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoState {
    pub identifier: String,
    pub state: HashMap<String, String>,
}

impl RepoState {
    pub fn from_tags(tags: &[Vec<String>]) -> Result<Self> {
        let mut identifier = None;
        let mut state = HashMap::new();

        for tag in tags {
            match tag.as_slice() {
                [t, id, ..] if t == "d" => identifier = Some(id.clone()),
                [name, value, ..] if is_state_ref(name) && is_state_value(value) => {
                    state.insert(name.to_string(), value.to_string());
                }
                _ => {}
            }
        }

        let identifier = identifier.ok_or(CoreError::MissingField("d"))?;

        Ok(Self { identifier, state })
    }

    pub fn to_tags(&self) -> Vec<Vec<String>> {
        let mut tags = Vec::new();
        tags.push(vec!["d".to_string(), self.identifier.clone()]);

        let mut keys: Vec<&String> = self.state.keys().collect();
        keys.sort();

        for key in keys {
            if let Some(value) = self.state.get(key) {
                tags.push(vec![key.clone(), value.clone()]);
            }
        }

        tags
    }

    pub fn head_ref(&self) -> Option<String> {
        let head = self.state.get("HEAD")?;
        parse_head_ref(head).map(|value| value.to_string())
    }

    pub fn head_commit(&self) -> Option<String> {
        let head = self.state.get("HEAD")?;
        if let Some(target) = parse_head_ref(head) {
            self.state.get(target).cloned()
        } else if is_hex40(head) {
            Some(head.clone())
        } else {
            None
        }
    }

    pub fn ref_map(&self) -> HashMap<String, String> {
        self.state
            .iter()
            .filter(|(key, _)| key.as_str() != "HEAD")
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect()
    }

    pub fn validate(&self) -> Result<()> {
        if self.identifier.trim().is_empty() {
            return Err(CoreError::MissingField("d"));
        }

        for (name, value) in &self.state {
            if !is_state_ref(name) {
                return Err(CoreError::InvalidTag {
                    tag: "state",
                    value: name.clone(),
                });
            }

            if !is_state_value(value) {
                return Err(CoreError::InvalidTag {
                    tag: "state",
                    value: value.clone(),
                });
            }
        }

        if !self.state.is_empty() && !self.state.contains_key("HEAD") {
            return Err(CoreError::MissingField("HEAD"));
        }

        if let Some(head) = self.state.get("HEAD") {
            if let Some(target) = parse_head_ref(head) {
                if !self.state.contains_key(target) {
                    return Err(CoreError::InvalidTag {
                        tag: "state",
                        value: head.clone(),
                    });
                }
            }
        }

        Ok(())
    }
}

fn is_state_ref(name: &str) -> bool {
    (name == "HEAD" || name.starts_with("refs/heads/") || name.starts_with("refs/tags"))
        && !name.ends_with("^{}")
}

fn is_state_value(value: &str) -> bool {
    is_hex40(value) || value.starts_with("ref: refs/")
}

fn parse_head_ref(value: &str) -> Option<&str> {
    if let Some(rest) = value.strip_prefix("ref: ") {
        Some(rest)
    } else if value.starts_with("refs/") {
        Some(value)
    } else {
        None
    }
}

fn is_hex40(value: &str) -> bool {
    value.len() == 40 && value.chars().all(|c| c.is_ascii_hexdigit())
}

fn is_hex64(value: &str) -> bool {
    value.len() == 64 && value.chars().all(|c| c.is_ascii_hexdigit())
}

fn normalize_grasp_host_for_compare(input: &str) -> Result<String> {
    let normalized = normalize_grasp_server_url(input)?;
    Ok(normalized.trim_start_matches("http://").to_string())
}

fn detect_grasp_servers(
    clone_urls: &[String],
    relay_urls: &[String],
    identifier: &str,
) -> Vec<String> {
    let relays: Vec<String> = relay_urls
        .iter()
        .filter_map(|r| normalize_grasp_server_url(r).ok())
        .collect();

    let mut servers = Vec::new();
    for url in clone_urls {
        let Ok(normalized) = normalize_grasp_server_url(url) else {
            continue;
        };
        if servers.contains(&normalized) {
            continue;
        }

        let matches_identifier = if let Ok(npub) = extract_npub(url) {
            url.contains(&format!("/{npub}/{identifier}.git"))
        } else {
            false
        };
        if !matches_identifier {
            continue;
        }

        if !relays.iter().any(|r| r == &normalized) {
            continue;
        }

        servers.push(normalized);
    }

    servers
}

#[cfg(test)]
mod tests {
    use super::RepoAnnouncement;
    use super::RepoState;
    use std::collections::HashMap;

    const SAMPLE_NPUB: &str = "npub1gjttreegkzys8jlhdnfm3qe39h2gka79cpndd0jsms5fk7tuhcnsdw56jq";

    #[test]
    fn announcement_round_trips_tags() {
        let announcement = RepoAnnouncement {
            identifier: "repo".to_string(),
            name: Some("Gittree".to_string()),
            description: Some("Example repository".to_string()),
            root_commit: Some("0123456789abcdef0123456789abcdef01234567".to_string()),
            clone: vec!["https://git.example/repo.git".to_string()],
            web: vec!["https://git.example/repo".to_string()],
            relays: vec!["wss://relay.example".to_string()],
            blossoms: vec!["https://blossom.example".to_string()],
            hashtags: vec!["nostr".to_string(), "git".to_string()],
            maintainers: vec![
                "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".to_string(),
            ],
        };

        let tags = announcement.to_tags();
        let parsed = RepoAnnouncement::from_tags(&tags).expect("parse tags");

        assert_eq!(parsed, announcement);
    }

    #[test]
    fn announcement_from_tags_parses_short_root_and_ignores_unknown_tags() {
        let root = "0123456789abcdef0123456789abcdef01234567".to_string();
        let tags = vec![
            vec!["d".to_string(), "repo".to_string()],
            vec!["r".to_string(), root.clone()],
            vec![
                "clone".to_string(),
                "https://git.example/repo.git".to_string(),
            ],
            vec!["relays".to_string(), "wss://relay.example".to_string()],
            vec!["t".to_string(), "nostr".to_string()],
            vec!["t".to_string(), "nostr".to_string()],
            vec!["x-ignored".to_string(), "value".to_string()],
        ];

        let parsed = RepoAnnouncement::from_tags(&tags).expect("parse");
        assert_eq!(parsed.root_commit, Some(root));
        assert_eq!(parsed.hashtags, vec!["nostr".to_string()]);
    }

    #[test]
    fn announcement_from_tags_requires_identifier() {
        let tags = vec![
            vec![
                "clone".to_string(),
                "https://git.example/repo.git".to_string(),
            ],
            vec!["relays".to_string(), "wss://relay.example".to_string()],
        ];
        let err = RepoAnnouncement::from_tags(&tags).expect_err("missing identifier should fail");
        assert!(matches!(err, crate::CoreError::MissingField("d")));
    }

    #[test]
    fn announcement_to_tags_omits_optional_fields_when_empty() {
        let announcement = RepoAnnouncement {
            identifier: "repo".to_string(),
            name: None,
            description: None,
            root_commit: None,
            clone: Vec::new(),
            web: Vec::new(),
            relays: Vec::new(),
            blossoms: Vec::new(),
            hashtags: Vec::new(),
            maintainers: Vec::new(),
        };

        let tags = announcement.to_tags();
        assert_eq!(tags, vec![vec!["d".to_string(), "repo".to_string()]]);
    }

    #[test]
    fn announcement_validation_requires_identifier() {
        let announcement = RepoAnnouncement {
            identifier: "".to_string(),
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

        assert!(matches!(
            announcement.validate(),
            Err(crate::CoreError::MissingField("d"))
        ));
    }

    #[test]
    fn announcement_validation_requires_clone() {
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

        assert!(matches!(
            announcement.validate(),
            Err(crate::CoreError::MissingField("clone"))
        ));
    }

    #[test]
    fn announcement_validation_requires_relays() {
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

        assert!(matches!(
            announcement.validate(),
            Err(crate::CoreError::MissingField("relays"))
        ));
    }

    #[test]
    fn announcement_validation_rejects_bad_root_commit() {
        let announcement = RepoAnnouncement {
            identifier: "repo".to_string(),
            name: None,
            description: None,
            root_commit: Some("bad".to_string()),
            clone: vec!["https://git.example/repo.git".to_string()],
            web: Vec::new(),
            relays: vec!["wss://relay.example".to_string()],
            blossoms: Vec::new(),
            hashtags: Vec::new(),
            maintainers: Vec::new(),
        };

        assert!(matches!(
            announcement.validate(),
            Err(crate::CoreError::InvalidField { field: "r", .. })
        ));
    }

    #[test]
    fn announcement_validation_accepts_valid_root_commit() {
        let announcement = RepoAnnouncement {
            identifier: "repo".to_string(),
            name: None,
            description: None,
            root_commit: Some("0123456789abcdef0123456789abcdef01234567".to_string()),
            clone: vec!["https://git.example/repo.git".to_string()],
            web: Vec::new(),
            relays: vec!["wss://relay.example".to_string()],
            blossoms: Vec::new(),
            hashtags: Vec::new(),
            maintainers: Vec::new(),
        };

        announcement.validate().expect("valid root commit");
    }

    #[test]
    fn announcement_validation_rejects_bad_maintainer() {
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
            maintainers: vec!["npub1bad".to_string()],
        };

        assert!(matches!(
            announcement.validate(),
            Err(crate::CoreError::InvalidField {
                field: "maintainers",
                ..
            })
        ));
    }

    #[test]
    fn announcement_validation_rejects_empty_clone_or_relay_entries() {
        let bad_clone = RepoAnnouncement {
            identifier: "repo".to_string(),
            name: None,
            description: None,
            root_commit: None,
            clone: vec!["   ".to_string()],
            web: Vec::new(),
            relays: vec!["wss://relay.example".to_string()],
            blossoms: Vec::new(),
            hashtags: Vec::new(),
            maintainers: Vec::new(),
        };
        assert!(matches!(
            bad_clone.validate(),
            Err(crate::CoreError::InvalidField { field: "clone", .. })
        ));

        let bad_relay = RepoAnnouncement {
            identifier: "repo".to_string(),
            name: None,
            description: None,
            root_commit: None,
            clone: vec!["https://git.example/repo.git".to_string()],
            web: Vec::new(),
            relays: vec!["   ".to_string()],
            blossoms: Vec::new(),
            hashtags: Vec::new(),
            maintainers: Vec::new(),
        };
        assert!(matches!(
            bad_relay.validate(),
            Err(crate::CoreError::InvalidField {
                field: "relays",
                ..
            })
        ));
    }

    #[test]
    fn announcement_lists_grasp_host_when_clone_and_relay_present() {
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

        let listed = announcement
            .lists_grasp_host("gittr.ee")
            .expect("host check");
        assert!(listed);
    }

    #[test]
    fn announcement_lists_grasp_host_rejects_invalid_host_input() {
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
        assert!(announcement.lists_grasp_host("://invalid").is_err());
    }

    #[test]
    fn announcement_lists_grasp_host_requires_relay_list() {
        let announcement = RepoAnnouncement {
            identifier: "repo".to_string(),
            name: None,
            description: None,
            root_commit: None,
            clone: vec!["https://gittr.ee/npub1example/repo.git".to_string()],
            web: Vec::new(),
            relays: Vec::new(),
            blossoms: Vec::new(),
            hashtags: Vec::new(),
            maintainers: Vec::new(),
        };

        let listed = announcement
            .lists_grasp_host("gittr.ee")
            .expect("host check");
        assert!(!listed);
    }

    #[test]
    fn announcement_lists_grasp_host_when_relay_listed_only() {
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

        let listed = announcement
            .lists_grasp_host("gittr.ee")
            .expect("host check");
        assert!(listed);
    }

    #[test]
    fn announcement_lists_grasp_host_ignores_invalid_urls() {
        let announcement = RepoAnnouncement {
            identifier: "repo".to_string(),
            name: None,
            description: None,
            root_commit: None,
            clone: vec![
                "https://gittr.ee/npub1example/repo.git".to_string(),
                "not-a-url".to_string(),
            ],
            web: Vec::new(),
            relays: vec!["wss://gittr.ee".to_string()],
            blossoms: Vec::new(),
            hashtags: Vec::new(),
            maintainers: Vec::new(),
        };

        let listed = announcement
            .lists_grasp_host("gittr.ee")
            .expect("host check");
        assert!(listed);
    }

    #[test]
    fn announcement_lists_grasp_host_skips_invalid_relays_and_non_matches() {
        let announcement = RepoAnnouncement {
            identifier: "repo".to_string(),
            name: None,
            description: None,
            root_commit: None,
            clone: vec!["https://gittr.ee/npub1example/repo.git".to_string()],
            web: Vec::new(),
            relays: vec!["not-a-url".to_string(), "wss://other.example".to_string()],
            blossoms: Vec::new(),
            hashtags: Vec::new(),
            maintainers: Vec::new(),
        };

        let listed = announcement
            .lists_grasp_host("gittr.ee")
            .expect("host check");
        assert!(!listed);
    }

    #[test]
    fn announcement_grasp_servers_detects_matching_host() {
        let announcement = RepoAnnouncement {
            identifier: "repo".to_string(),
            name: None,
            description: None,
            root_commit: None,
            clone: vec![format!("https://gittr.ee/{}/repo.git", SAMPLE_NPUB)],
            web: Vec::new(),
            relays: vec!["wss://gittr.ee".to_string()],
            blossoms: Vec::new(),
            hashtags: Vec::new(),
            maintainers: Vec::new(),
        };

        let servers = announcement.grasp_servers();
        assert_eq!(servers, vec!["gittr.ee".to_string()]);
    }

    #[test]
    fn announcement_grasp_servers_requires_relay_match() {
        let announcement = RepoAnnouncement {
            identifier: "repo".to_string(),
            name: None,
            description: None,
            root_commit: None,
            clone: vec![format!("https://gittr.ee/{}/repo.git", SAMPLE_NPUB)],
            web: Vec::new(),
            relays: vec!["wss://other.example".to_string()],
            blossoms: Vec::new(),
            hashtags: Vec::new(),
            maintainers: Vec::new(),
        };

        let servers = announcement.grasp_servers();
        assert!(servers.is_empty());
    }

    #[test]
    fn announcement_grasp_servers_requires_identifier_match() {
        let announcement = RepoAnnouncement {
            identifier: "repo".to_string(),
            name: None,
            description: None,
            root_commit: None,
            clone: vec![format!("https://gittr.ee/{}/other.git", SAMPLE_NPUB)],
            web: Vec::new(),
            relays: vec!["wss://gittr.ee".to_string()],
            blossoms: Vec::new(),
            hashtags: Vec::new(),
            maintainers: Vec::new(),
        };

        let servers = announcement.grasp_servers();
        assert!(servers.is_empty());
    }

    #[test]
    fn announcement_grasp_servers_skips_invalid_duplicate_and_non_npub_urls() {
        let valid = format!("https://gittr.ee/{}/repo.git", SAMPLE_NPUB);
        let announcement = RepoAnnouncement {
            identifier: "repo".to_string(),
            name: None,
            description: None,
            root_commit: None,
            clone: vec![
                valid.clone(),
                valid.clone(),
                "https://gittr.ee/notnpub/repo.git".to_string(),
                "://invalid".to_string(),
            ],
            web: Vec::new(),
            relays: vec!["wss://gittr.ee".to_string()],
            blossoms: Vec::new(),
            hashtags: Vec::new(),
            maintainers: Vec::new(),
        };

        let servers = announcement.grasp_servers();
        assert_eq!(servers, vec!["gittr.ee".to_string()]);
    }

    #[test]
    fn announcement_maintainer_keys_returns_cloned_list() {
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
            maintainers: vec!["11".repeat(32)],
        };

        let keys = announcement.maintainer_keys();
        assert_eq!(keys, vec!["11".repeat(32)]);
    }

    #[test]
    fn announcement_maintainer_keys_returns_empty_for_missing_maintainers() {
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
        assert!(announcement.maintainer_keys().is_empty());
    }

    #[test]
    fn state_round_trips_tags() {
        let mut state = HashMap::new();
        state.insert(
            "refs/heads/main".to_string(),
            "0123456789abcdef0123456789abcdef01234567".to_string(),
        );
        state.insert(
            "refs/tags/v1.0.0".to_string(),
            "0123456789abcdef0123456789abcdef01234567".to_string(),
        );
        state.insert("HEAD".to_string(), "ref: refs/heads/main".to_string());

        let repo_state = RepoState {
            identifier: "repo".to_string(),
            state,
        };

        let tags = repo_state.to_tags();
        let parsed = RepoState::from_tags(&tags).expect("parse tags");

        assert_eq!(parsed, repo_state);
    }

    #[test]
    fn state_from_tags_requires_identifier() {
        let tags = vec![vec![
            "refs/heads/main".to_string(),
            "0123456789abcdef0123456789abcdef01234567".to_string(),
        ]];
        let err = RepoState::from_tags(&tags).expect_err("missing identifier should fail");
        assert!(matches!(err, crate::CoreError::MissingField("d")));
    }

    #[test]
    fn state_from_tags_ignores_invalid_or_unknown_tags() {
        let tags = vec![
            vec!["d".to_string(), "repo".to_string()],
            vec![
                "refs/heads/main".to_string(),
                "0123456789abcdef0123456789abcdef01234567".to_string(),
            ],
            vec!["HEAD".to_string(), "ref: refs/heads/main".to_string()],
            vec!["refs/heads/dev".to_string(), "bad".to_string()],
            vec!["x-ignored".to_string(), "value".to_string()],
        ];

        let parsed = RepoState::from_tags(&tags).expect("parse");
        assert_eq!(parsed.identifier, "repo");
        assert_eq!(
            parsed.state.get("refs/heads/main"),
            Some(&"0123456789abcdef0123456789abcdef01234567".to_string())
        );
        assert_eq!(
            parsed.state.get("HEAD"),
            Some(&"ref: refs/heads/main".to_string())
        );
        assert!(!parsed.state.contains_key("refs/heads/dev"));
    }

    #[test]
    fn state_head_ref_parses_symbolic() {
        let mut state = HashMap::new();
        state.insert("HEAD".to_string(), "ref: refs/heads/main".to_string());
        let repo_state = RepoState {
            identifier: "repo".to_string(),
            state,
        };
        assert_eq!(repo_state.head_ref(), Some("refs/heads/main".to_string()));
    }

    #[test]
    fn state_head_ref_returns_none_without_head() {
        let repo_state = RepoState {
            identifier: "repo".to_string(),
            state: HashMap::new(),
        };
        assert_eq!(repo_state.head_ref(), None);
    }

    #[test]
    fn state_head_commit_returns_none_without_head() {
        let repo_state = RepoState {
            identifier: "repo".to_string(),
            state: HashMap::new(),
        };
        assert_eq!(repo_state.head_commit(), None);
    }

    #[test]
    fn state_head_commit_resolves_symbolic() {
        let mut state = HashMap::new();
        state.insert("HEAD".to_string(), "ref: refs/heads/main".to_string());
        state.insert(
            "refs/heads/main".to_string(),
            "0123456789abcdef0123456789abcdef01234567".to_string(),
        );
        let repo_state = RepoState {
            identifier: "repo".to_string(),
            state,
        };
        assert_eq!(
            repo_state.head_commit(),
            Some("0123456789abcdef0123456789abcdef01234567".to_string())
        );
    }

    #[test]
    fn state_head_commit_handles_direct_ref_hash_and_invalid_head() {
        let mut ref_head_state = HashMap::new();
        ref_head_state.insert("HEAD".to_string(), "refs/heads/main".to_string());
        ref_head_state.insert(
            "refs/heads/main".to_string(),
            "0123456789abcdef0123456789abcdef01234567".to_string(),
        );
        let ref_head_repo_state = RepoState {
            identifier: "repo".to_string(),
            state: ref_head_state,
        };
        assert_eq!(
            ref_head_repo_state.head_commit(),
            Some("0123456789abcdef0123456789abcdef01234567".to_string())
        );

        let mut hash_head_state = HashMap::new();
        hash_head_state.insert(
            "HEAD".to_string(),
            "0123456789abcdef0123456789abcdef01234567".to_string(),
        );
        let hash_head_repo_state = RepoState {
            identifier: "repo".to_string(),
            state: hash_head_state,
        };
        assert_eq!(
            hash_head_repo_state.head_commit(),
            Some("0123456789abcdef0123456789abcdef01234567".to_string())
        );

        let mut invalid_head_state = HashMap::new();
        invalid_head_state.insert("HEAD".to_string(), "invalid".to_string());
        let invalid_head_repo_state = RepoState {
            identifier: "repo".to_string(),
            state: invalid_head_state,
        };
        assert_eq!(invalid_head_repo_state.head_commit(), None);
    }

    #[test]
    fn state_ref_map_excludes_head() {
        let mut state = HashMap::new();
        state.insert("HEAD".to_string(), "ref: refs/heads/main".to_string());
        state.insert(
            "refs/heads/main".to_string(),
            "0123456789abcdef0123456789abcdef01234567".to_string(),
        );
        let repo_state = RepoState {
            identifier: "repo".to_string(),
            state,
        };
        let refs = repo_state.ref_map();
        assert!(!refs.contains_key("HEAD"));
        assert!(refs.contains_key("refs/heads/main"));
    }

    #[test]
    fn state_ref_map_with_only_head_is_empty() {
        let mut state = HashMap::new();
        state.insert("HEAD".to_string(), "ref: refs/heads/main".to_string());
        let repo_state = RepoState {
            identifier: "repo".to_string(),
            state,
        };
        assert!(repo_state.ref_map().is_empty());
    }

    #[test]
    fn state_validation_requires_identifier() {
        let repo_state = RepoState {
            identifier: "".to_string(),
            state: HashMap::new(),
        };

        assert!(matches!(
            repo_state.validate(),
            Err(crate::CoreError::MissingField("d"))
        ));
    }

    #[test]
    fn state_validation_accepts_empty_state() {
        let repo_state = RepoState {
            identifier: "repo".to_string(),
            state: HashMap::new(),
        };
        repo_state.validate().expect("empty state is valid");
    }

    #[test]
    fn state_validation_requires_head_when_state_present() {
        let mut state = HashMap::new();
        state.insert(
            "refs/heads/main".to_string(),
            "0123456789abcdef0123456789abcdef01234567".to_string(),
        );
        let repo_state = RepoState {
            identifier: "repo".to_string(),
            state,
        };

        assert!(matches!(
            repo_state.validate(),
            Err(crate::CoreError::MissingField("HEAD"))
        ));
    }

    #[test]
    fn state_validation_rejects_invalid_ref_name() {
        let mut state = HashMap::new();
        state.insert(
            "refs/pull/1".to_string(),
            "0123456789abcdef0123456789abcdef01234567".to_string(),
        );
        let repo_state = RepoState {
            identifier: "repo".to_string(),
            state,
        };

        assert!(matches!(
            repo_state.validate(),
            Err(crate::CoreError::InvalidTag { .. })
        ));
    }

    #[test]
    fn state_validation_rejects_invalid_ref_value() {
        let mut state = HashMap::new();
        state.insert("refs/heads/main".to_string(), "invalid".to_string());
        state.insert("HEAD".to_string(), "ref: refs/heads/main".to_string());
        let repo_state = RepoState {
            identifier: "repo".to_string(),
            state,
        };

        assert!(matches!(
            repo_state.validate(),
            Err(crate::CoreError::InvalidTag { .. })
        ));
    }

    #[test]
    fn state_validation_rejects_head_ref_missing_target() {
        let mut state = HashMap::new();
        state.insert("HEAD".to_string(), "ref: refs/heads/main".to_string());
        let repo_state = RepoState {
            identifier: "repo".to_string(),
            state,
        };

        assert!(matches!(
            repo_state.validate(),
            Err(crate::CoreError::InvalidTag { .. })
        ));
    }

    #[test]
    fn state_validation_accepts_head_hash_and_rejects_direct_ref_value() {
        let mut head_hash_state = HashMap::new();
        head_hash_state.insert(
            "HEAD".to_string(),
            "0123456789abcdef0123456789abcdef01234567".to_string(),
        );
        head_hash_state.insert(
            "refs/heads/main".to_string(),
            "0123456789abcdef0123456789abcdef01234567".to_string(),
        );
        let head_hash_repo_state = RepoState {
            identifier: "repo".to_string(),
            state: head_hash_state,
        };
        head_hash_repo_state.validate().expect("head hash should validate");

        let mut direct_ref_state = HashMap::new();
        direct_ref_state.insert("HEAD".to_string(), "refs/heads/main".to_string());
        direct_ref_state.insert(
            "refs/heads/main".to_string(),
            "0123456789abcdef0123456789abcdef01234567".to_string(),
        );
        let direct_ref_repo_state = RepoState {
            identifier: "repo".to_string(),
            state: direct_ref_state,
        };
        assert!(matches!(
            direct_ref_repo_state.validate(),
            Err(crate::CoreError::InvalidTag {
                tag: "state",
                value
            }) if value == "refs/heads/main"
        ));
    }
}
