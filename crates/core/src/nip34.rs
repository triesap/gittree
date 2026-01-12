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

        let identifier =
            identifier.ok_or(CoreError::MissingField("d"))?;

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

    pub fn validate(&self) -> Result<()> {
        if self.identifier.trim().is_empty() {
            return Err(CoreError::MissingField("d"));
        }

        if self.clone.is_empty() {
            return Err(CoreError::MissingField("clone"));
        }

        if self.relays.is_empty() {
            return Err(CoreError::MissingField("relays"));
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

fn is_hex40(value: &str) -> bool {
    value.len() == 40 && value.chars().all(|c| c.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::RepoAnnouncement;
    use super::RepoState;
    use std::collections::HashMap;

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
            maintainers: vec!["0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".to_string()],
        };

        let tags = announcement.to_tags();
        let parsed = RepoAnnouncement::from_tags(&tags).expect("parse tags");

        assert_eq!(parsed, announcement);
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
}
