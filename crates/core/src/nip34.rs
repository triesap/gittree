use crate::{CoreError, Result};

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
}

fn push_unique(target: &mut Vec<String>, value: &str) {
    if !target.iter().any(|item| item == value) {
        target.push(value.to_string());
    }
}

fn extend_unique(target: &mut Vec<String>, values: &[String]) {
    for value in values {
        push_unique(target, value);
    }
}

fn join_tag_values(kind: &str, values: &[String]) -> Vec<String> {
    let mut tag = Vec::with_capacity(values.len() + 1);
    tag.push(kind.to_string());
    tag.extend(values.iter().cloned());
    tag
}

#[cfg(test)]
mod tests {
    use super::RepoAnnouncement;

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
}
