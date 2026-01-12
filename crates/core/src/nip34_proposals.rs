use crate::nip34_common::{is_hex_hash, is_hex_len, validate_repo_address, HEX_EVENT_ID_LEN, HEX_PUBKEY_LEN};
use crate::tags::{extend_unique, join_tag_values, push_unique};
use crate::{CoreError, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullRequest {
    pub repo_address: String,
    pub repo_refs: Vec<String>,
    pub mentions: Vec<String>,
    pub subject: Option<String>,
    pub labels: Vec<String>,
    pub tip_commit: String,
    pub clone: Vec<String>,
    pub branch_name: Option<String>,
    pub revision_of: Option<String>,
    pub merge_base: Option<String>,
}

impl PullRequest {
    pub fn from_tags(tags: &[Vec<String>]) -> Result<Self> {
        let mut repo_address = None;
        let mut repo_refs = Vec::new();
        let mut mentions = Vec::new();
        let mut subject = None;
        let mut labels = Vec::new();
        let mut tip_commit = None;
        let mut clone = Vec::new();
        let mut branch_name = None;
        let mut revision_of = None;
        let mut merge_base = None;

        for tag in tags {
            match tag.as_slice() {
                [t, value, ..] if t == "a" => repo_address = Some(value.clone()),
                [t, value, ..] if t == "r" => push_unique(&mut repo_refs, value),
                [t, value, ..] if t == "p" => push_unique(&mut mentions, value),
                [t, value, ..] if t == "subject" => subject = Some(value.clone()),
                [t, value, ..] if t == "t" => push_unique(&mut labels, value),
                [t, value, ..] if t == "c" => tip_commit = Some(value.clone()),
                [t, values @ ..] if t == "clone" => extend_unique(&mut clone, values),
                [t, value, ..] if t == "branch-name" => branch_name = Some(value.clone()),
                [t, value, ..] if t == "e" => revision_of = Some(value.clone()),
                [t, value, ..] if t == "merge-base" => merge_base = Some(value.clone()),
                _ => {}
            }
        }

        Ok(Self {
            repo_address: repo_address.ok_or(CoreError::MissingField("a"))?,
            repo_refs,
            mentions,
            subject,
            labels,
            tip_commit: tip_commit.ok_or(CoreError::MissingField("c"))?,
            clone,
            branch_name,
            revision_of,
            merge_base,
        })
    }

    pub fn to_tags(&self) -> Vec<Vec<String>> {
        let mut tags = Vec::new();

        tags.push(vec!["a".to_string(), self.repo_address.clone()]);

        for repo_ref in &self.repo_refs {
            tags.push(vec!["r".to_string(), repo_ref.clone()]);
        }

        for mention in &self.mentions {
            tags.push(vec!["p".to_string(), mention.clone()]);
        }

        if let Some(subject) = &self.subject {
            tags.push(vec!["subject".to_string(), subject.clone()]);
        }

        for label in &self.labels {
            tags.push(vec!["t".to_string(), label.clone()]);
        }

        tags.push(vec!["c".to_string(), self.tip_commit.clone()]);

        if !self.clone.is_empty() {
            tags.push(join_tag_values("clone", &self.clone));
        }

        if let Some(branch_name) = &self.branch_name {
            tags.push(vec!["branch-name".to_string(), branch_name.clone()]);
        }

        if let Some(revision_of) = &self.revision_of {
            tags.push(vec!["e".to_string(), revision_of.clone()]);
        }

        if let Some(merge_base) = &self.merge_base {
            tags.push(vec!["merge-base".to_string(), merge_base.clone()]);
        }

        tags
    }

    pub fn validate(&self) -> Result<()> {
        validate_repo_address(&self.repo_address)?;

        if self.tip_commit.trim().is_empty() {
            return Err(CoreError::MissingField("c"));
        }

        if !is_hex_hash(&self.tip_commit) {
            return Err(CoreError::InvalidField {
                field: "c",
                value: self.tip_commit.clone(),
            });
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

        for repo_ref in &self.repo_refs {
            if !is_hex_hash(repo_ref) {
                return Err(CoreError::InvalidField {
                    field: "r",
                    value: repo_ref.clone(),
                });
            }
        }

        for mention in &self.mentions {
            if !is_hex_len(mention, HEX_PUBKEY_LEN) {
                return Err(CoreError::InvalidField {
                    field: "p",
                    value: mention.clone(),
                });
            }
        }

        if let Some(subject) = &self.subject {
            if subject.trim().is_empty() {
                return Err(CoreError::InvalidField {
                    field: "subject",
                    value: subject.clone(),
                });
            }
        }

        if let Some(branch_name) = &self.branch_name {
            if branch_name.trim().is_empty() {
                return Err(CoreError::InvalidField {
                    field: "branch-name",
                    value: branch_name.clone(),
                });
            }
        }

        if let Some(revision_of) = &self.revision_of {
            if !is_hex_len(revision_of, HEX_EVENT_ID_LEN) {
                return Err(CoreError::InvalidField {
                    field: "e",
                    value: revision_of.clone(),
                });
            }
        }

        if let Some(merge_base) = &self.merge_base {
            if !is_hex_hash(merge_base) {
                return Err(CoreError::InvalidField {
                    field: "merge-base",
                    value: merge_base.clone(),
                });
            }
        }

        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullRequestUpdate {
    pub repo_address: String,
    pub repo_refs: Vec<String>,
    pub mentions: Vec<String>,
    pub root_event_id: String,
    pub root_author: String,
    pub tip_commit: String,
    pub clone: Vec<String>,
    pub merge_base: Option<String>,
}

impl PullRequestUpdate {
    pub fn from_tags(tags: &[Vec<String>]) -> Result<Self> {
        let mut repo_address = None;
        let mut repo_refs = Vec::new();
        let mut mentions = Vec::new();
        let mut root_event_id = None;
        let mut root_author = None;
        let mut tip_commit = None;
        let mut clone = Vec::new();
        let mut merge_base = None;

        for tag in tags {
            match tag.as_slice() {
                [t, value, ..] if t == "a" => repo_address = Some(value.clone()),
                [t, value, ..] if t == "r" => push_unique(&mut repo_refs, value),
                [t, value, ..] if t == "p" => push_unique(&mut mentions, value),
                [t, value, ..] if t == "E" => root_event_id = Some(value.clone()),
                [t, value, ..] if t == "P" => root_author = Some(value.clone()),
                [t, value, ..] if t == "c" => tip_commit = Some(value.clone()),
                [t, values @ ..] if t == "clone" => extend_unique(&mut clone, values),
                [t, value, ..] if t == "merge-base" => merge_base = Some(value.clone()),
                _ => {}
            }
        }

        Ok(Self {
            repo_address: repo_address.ok_or(CoreError::MissingField("a"))?,
            repo_refs,
            mentions,
            root_event_id: root_event_id.ok_or(CoreError::MissingField("E"))?,
            root_author: root_author.ok_or(CoreError::MissingField("P"))?,
            tip_commit: tip_commit.ok_or(CoreError::MissingField("c"))?,
            clone,
            merge_base,
        })
    }

    pub fn to_tags(&self) -> Vec<Vec<String>> {
        let mut tags = Vec::new();

        tags.push(vec!["a".to_string(), self.repo_address.clone()]);

        for repo_ref in &self.repo_refs {
            tags.push(vec!["r".to_string(), repo_ref.clone()]);
        }

        for mention in &self.mentions {
            tags.push(vec!["p".to_string(), mention.clone()]);
        }

        tags.push(vec!["E".to_string(), self.root_event_id.clone()]);
        tags.push(vec!["P".to_string(), self.root_author.clone()]);
        tags.push(vec!["c".to_string(), self.tip_commit.clone()]);

        if !self.clone.is_empty() {
            tags.push(join_tag_values("clone", &self.clone));
        }

        if let Some(merge_base) = &self.merge_base {
            tags.push(vec!["merge-base".to_string(), merge_base.clone()]);
        }

        tags
    }

    pub fn validate(&self) -> Result<()> {
        validate_repo_address(&self.repo_address)?;

        if !is_hex_len(&self.root_event_id, HEX_EVENT_ID_LEN) {
            return Err(CoreError::InvalidField {
                field: "E",
                value: self.root_event_id.clone(),
            });
        }

        if !is_hex_len(&self.root_author, HEX_PUBKEY_LEN) {
            return Err(CoreError::InvalidField {
                field: "P",
                value: self.root_author.clone(),
            });
        }

        if !is_hex_hash(&self.tip_commit) {
            return Err(CoreError::InvalidField {
                field: "c",
                value: self.tip_commit.clone(),
            });
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

        for repo_ref in &self.repo_refs {
            if !is_hex_hash(repo_ref) {
                return Err(CoreError::InvalidField {
                    field: "r",
                    value: repo_ref.clone(),
                });
            }
        }

        for mention in &self.mentions {
            if !is_hex_len(mention, HEX_PUBKEY_LEN) {
                return Err(CoreError::InvalidField {
                    field: "p",
                    value: mention.clone(),
                });
            }
        }

        if let Some(merge_base) = &self.merge_base {
            if !is_hex_hash(merge_base) {
                return Err(CoreError::InvalidField {
                    field: "merge-base",
                    value: merge_base.clone(),
                });
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::PullRequest;
    use super::PullRequestUpdate;

    fn hex_of(byte: u8, len: usize) -> String {
        format!("{:02x}", byte).repeat(len / 2)
    }

    #[test]
    fn pull_request_round_trips_tags() {
        let pubkey = hex_of(0x11, 64);
        let pr = PullRequest {
            repo_address: format!("30617:{pubkey}:repo"),
            repo_refs: vec![hex_of(0x22, 40)],
            mentions: vec![hex_of(0x33, 64)],
            subject: Some("Add feature".to_string()),
            labels: vec!["enhancement".to_string(), "review".to_string()],
            tip_commit: hex_of(0x44, 40),
            clone: vec![
                "https://git.example/repo.git".to_string(),
                "https://mirror.example/repo.git".to_string(),
            ],
            branch_name: Some("feature/add".to_string()),
            revision_of: Some(hex_of(0x55, 64)),
            merge_base: Some(hex_of(0x66, 40)),
        };

        let tags = pr.to_tags();
        let parsed = PullRequest::from_tags(&tags).expect("parse");
        assert_eq!(parsed, pr);
        parsed.validate().expect("valid");
    }

    #[test]
    fn pull_request_requires_clone_and_tip() {
        let pubkey = hex_of(0x11, 64);
        let pr = PullRequest {
            repo_address: format!("30617:{pubkey}:repo"),
            repo_refs: Vec::new(),
            mentions: Vec::new(),
            subject: None,
            labels: Vec::new(),
            tip_commit: "".to_string(),
            clone: Vec::new(),
            branch_name: None,
            revision_of: None,
            merge_base: None,
        };

        assert!(pr.validate().is_err());
    }

    #[test]
    fn pull_request_update_round_trips_tags() {
        let pubkey = hex_of(0x11, 64);
        let update = PullRequestUpdate {
            repo_address: format!("30617:{pubkey}:repo"),
            repo_refs: vec![hex_of(0x22, 40)],
            mentions: vec![hex_of(0x33, 64)],
            root_event_id: hex_of(0x44, 64),
            root_author: hex_of(0x55, 64),
            tip_commit: hex_of(0x66, 40),
            clone: vec!["https://git.example/repo.git".to_string()],
            merge_base: Some(hex_of(0x77, 40)),
        };

        let tags = update.to_tags();
        let parsed = PullRequestUpdate::from_tags(&tags).expect("parse");
        assert_eq!(parsed, update);
        parsed.validate().expect("valid");
    }
}
