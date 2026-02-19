use crate::nip34_common::{
    HEX_EVENT_ID_LEN, HEX_PUBKEY_LEN, is_hex_hash, is_hex_len, validate_repo_address,
};
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
    use crate::CoreError;

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
    fn pull_request_validate_accepts_without_optional_subject_branch_or_merge_base() {
        let pubkey = hex_of(0x11, 64);
        let pr = PullRequest {
            repo_address: format!("30617:{pubkey}:repo"),
            repo_refs: vec![hex_of(0x22, 40)],
            mentions: vec![hex_of(0x33, 64)],
            subject: None,
            labels: Vec::new(),
            tip_commit: hex_of(0x44, 40),
            clone: vec!["https://git.example/repo.git".to_string()],
            branch_name: None,
            revision_of: None,
            merge_base: None,
        };
        pr.validate().expect("valid pull request");
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

    #[test]
    fn pull_request_update_from_tags_ignores_unknown_tags() {
        let pubkey = hex_of(0x11, 64);
        let tags = vec![
            vec!["a".to_string(), format!("30617:{pubkey}:repo")],
            vec!["E".to_string(), hex_of(0x44, 64)],
            vec!["P".to_string(), hex_of(0x55, 64)],
            vec!["c".to_string(), hex_of(0x66, 40)],
            vec!["clone".to_string(), "https://git.example/repo.git".to_string()],
            vec!["x-unknown".to_string(), "ignored".to_string()],
        ];
        let parsed = PullRequestUpdate::from_tags(&tags).expect("parse");
        assert_eq!(parsed.clone, vec!["https://git.example/repo.git".to_string()]);
    }

    #[test]
    fn pull_request_update_validate_accepts_without_merge_base() {
        let pubkey = hex_of(0x11, 64);
        let update = PullRequestUpdate {
            repo_address: format!("30617:{pubkey}:repo"),
            repo_refs: vec![hex_of(0x22, 40)],
            mentions: vec![hex_of(0x33, 64)],
            root_event_id: hex_of(0x44, 64),
            root_author: hex_of(0x55, 64),
            tip_commit: hex_of(0x66, 40),
            clone: vec!["https://git.example/repo.git".to_string()],
            merge_base: None,
        };
        update.validate().expect("valid update");
    }

    #[test]
    fn pull_request_from_tags_requires_repo_and_tip() {
        let missing_repo = PullRequest::from_tags(&[vec!["c".to_string(), hex_of(0x11, 40)]])
            .expect_err("missing repo address should fail");
        assert!(matches!(missing_repo, CoreError::MissingField("a")));

        let missing_tip = PullRequest::from_tags(&[vec![
            "a".to_string(),
            format!("30617:{}:repo", hex_of(0x11, 64)),
        ]])
        .expect_err("missing tip commit should fail");
        assert!(matches!(missing_tip, CoreError::MissingField("c")));
    }

    #[test]
    fn pull_request_validate_rejects_invalid_optional_fields() {
        let pubkey = hex_of(0x11, 64);
        let valid = PullRequest {
            repo_address: format!("30617:{pubkey}:repo"),
            repo_refs: vec![hex_of(0x22, 40)],
            mentions: vec![hex_of(0x33, 64)],
            subject: Some("subject".to_string()),
            labels: vec!["bug".to_string()],
            tip_commit: hex_of(0x44, 40),
            clone: vec!["https://git.example/repo.git".to_string()],
            branch_name: Some("feature/x".to_string()),
            revision_of: Some(hex_of(0x55, 64)),
            merge_base: Some(hex_of(0x66, 40)),
        };

        let mut bad_subject = valid.clone();
        bad_subject.subject = Some("   ".to_string());
        assert!(matches!(
            bad_subject.validate(),
            Err(CoreError::InvalidField { field: "subject", .. })
        ));

        let mut bad_branch = valid.clone();
        bad_branch.branch_name = Some("   ".to_string());
        assert!(matches!(
            bad_branch.validate(),
            Err(CoreError::InvalidField {
                field: "branch-name",
                ..
            })
        ));

        let mut bad_revision = valid.clone();
        bad_revision.revision_of = Some("abcd".to_string());
        assert!(matches!(
            bad_revision.validate(),
            Err(CoreError::InvalidField { field: "e", .. })
        ));

        let mut bad_merge_base = valid.clone();
        bad_merge_base.merge_base = Some("abcd".to_string());
        assert!(matches!(
            bad_merge_base.validate(),
            Err(CoreError::InvalidField {
                field: "merge-base",
                ..
            })
        ));
    }

    #[test]
    fn pull_request_update_from_tags_requires_root_author_and_clone() {
        let pubkey = hex_of(0x11, 64);
        let missing_root_author = PullRequestUpdate::from_tags(&[
            vec!["a".to_string(), format!("30617:{pubkey}:repo")],
            vec!["E".to_string(), hex_of(0x22, 64)],
            vec!["c".to_string(), hex_of(0x33, 40)],
            vec!["clone".to_string(), "https://git.example/repo.git".to_string()],
        ])
        .expect_err("missing root author should fail");
        assert!(matches!(missing_root_author, CoreError::MissingField("P")));

        let missing_clone = PullRequestUpdate {
            repo_address: format!("30617:{pubkey}:repo"),
            repo_refs: Vec::new(),
            mentions: Vec::new(),
            root_event_id: hex_of(0x44, 64),
            root_author: hex_of(0x55, 64),
            tip_commit: hex_of(0x66, 40),
            clone: Vec::new(),
            merge_base: None,
        };
        assert!(matches!(
            missing_clone.validate(),
            Err(CoreError::MissingField("clone"))
        ));
    }

    #[test]
    fn pull_request_update_validate_rejects_invalid_fields() {
        let pubkey = hex_of(0x11, 64);
        let valid = PullRequestUpdate {
            repo_address: format!("30617:{pubkey}:repo"),
            repo_refs: vec![hex_of(0x22, 40)],
            mentions: vec![hex_of(0x33, 64)],
            root_event_id: hex_of(0x44, 64),
            root_author: hex_of(0x55, 64),
            tip_commit: hex_of(0x66, 40),
            clone: vec!["https://git.example/repo.git".to_string()],
            merge_base: Some(hex_of(0x77, 40)),
        };

        let mut bad_root_event = valid.clone();
        bad_root_event.root_event_id = "abcd".to_string();
        assert!(matches!(
            bad_root_event.validate(),
            Err(CoreError::InvalidField { field: "E", .. })
        ));

        let mut bad_root_author = valid.clone();
        bad_root_author.root_author = "abcd".to_string();
        assert!(matches!(
            bad_root_author.validate(),
            Err(CoreError::InvalidField { field: "P", .. })
        ));

        let mut bad_tip_commit = valid.clone();
        bad_tip_commit.tip_commit = "abcd".to_string();
        assert!(matches!(
            bad_tip_commit.validate(),
            Err(CoreError::InvalidField { field: "c", .. })
        ));

        let mut bad_clone_entry = valid.clone();
        bad_clone_entry.clone = vec![" ".to_string()];
        assert!(matches!(
            bad_clone_entry.validate(),
            Err(CoreError::InvalidField {
                field: "clone",
                ..
            })
        ));

        let mut bad_merge_base = valid.clone();
        bad_merge_base.merge_base = Some("abcd".to_string());
        assert!(matches!(
            bad_merge_base.validate(),
            Err(CoreError::InvalidField {
                field: "merge-base",
                ..
            })
        ));
    }

    #[test]
    fn pull_request_from_tags_ignores_unknown_tags_and_deduplicates_values() {
        let pubkey = hex_of(0x11, 64);
        let ref_commit = hex_of(0x22, 40);
        let mention = hex_of(0x33, 64);
        let tags = vec![
            vec!["a".to_string(), format!("30617:{pubkey}:repo")],
            vec!["r".to_string(), ref_commit.clone()],
            vec!["r".to_string(), ref_commit.clone()],
            vec!["p".to_string(), mention.clone()],
            vec!["p".to_string(), mention.clone()],
            vec!["t".to_string(), "bug".to_string()],
            vec!["t".to_string(), "bug".to_string()],
            vec!["c".to_string(), hex_of(0x44, 40)],
            vec![
                "clone".to_string(),
                "https://git.example/repo.git".to_string(),
                "https://git.example/repo.git".to_string(),
            ],
            vec!["unknown".to_string(), "ignored".to_string()],
        ];

        let parsed = PullRequest::from_tags(&tags).expect("parse");
        assert_eq!(parsed.repo_refs, vec![ref_commit]);
        assert_eq!(parsed.mentions, vec![mention]);
        assert_eq!(parsed.labels, vec!["bug".to_string()]);
        assert_eq!(parsed.clone, vec!["https://git.example/repo.git".to_string()]);
    }

    #[test]
    fn pull_request_to_tags_skips_empty_optional_fields() {
        let pubkey = hex_of(0x11, 64);
        let pr = PullRequest {
            repo_address: format!("30617:{pubkey}:repo"),
            repo_refs: Vec::new(),
            mentions: Vec::new(),
            subject: None,
            labels: Vec::new(),
            tip_commit: hex_of(0x44, 40),
            clone: Vec::new(),
            branch_name: None,
            revision_of: None,
            merge_base: None,
        };

        let tags = pr.to_tags();
        assert!(!tags.iter().any(|tag| tag.first().is_some_and(|name| name == "subject")));
        assert!(!tags.iter().any(|tag| tag.first().is_some_and(|name| name == "clone")));
        assert!(!tags.iter().any(|tag| tag.first().is_some_and(|name| name == "branch-name")));
        assert!(!tags.iter().any(|tag| tag.first().is_some_and(|name| name == "e")));
        assert!(!tags.iter().any(|tag| tag.first().is_some_and(|name| name == "merge-base")));
    }

    #[test]
    fn pull_request_validate_rejects_repo_address_refs_and_mentions() {
        let pubkey = hex_of(0x11, 64);
        let valid = PullRequest {
            repo_address: format!("30617:{pubkey}:repo"),
            repo_refs: vec![hex_of(0x22, 40)],
            mentions: vec![hex_of(0x33, 64)],
            subject: Some("subject".to_string()),
            labels: Vec::new(),
            tip_commit: hex_of(0x44, 40),
            clone: vec!["https://git.example/repo.git".to_string()],
            branch_name: Some("feature/x".to_string()),
            revision_of: None,
            merge_base: None,
        };

        let mut bad_repo_address = valid.clone();
        bad_repo_address.repo_address = "30617:nothex:repo".to_string();
        assert!(matches!(
            bad_repo_address.validate(),
            Err(CoreError::InvalidField { field: "a", .. })
        ));

        let mut bad_repo_ref = valid.clone();
        bad_repo_ref.repo_refs = vec!["not-a-hex-hash".to_string()];
        assert!(matches!(
            bad_repo_ref.validate(),
            Err(CoreError::InvalidField { field: "r", .. })
        ));

        let mut bad_mention = valid.clone();
        bad_mention.mentions = vec!["not-a-pubkey".to_string()];
        assert!(matches!(
            bad_mention.validate(),
            Err(CoreError::InvalidField { field: "p", .. })
        ));
    }

    #[test]
    fn pull_request_validate_rejects_invalid_tip_and_clone_entries() {
        let pubkey = hex_of(0x11, 64);
        let valid = PullRequest {
            repo_address: format!("30617:{pubkey}:repo"),
            repo_refs: Vec::new(),
            mentions: Vec::new(),
            subject: None,
            labels: Vec::new(),
            tip_commit: hex_of(0x44, 40),
            clone: vec!["https://git.example/repo.git".to_string()],
            branch_name: None,
            revision_of: None,
            merge_base: None,
        };

        let mut bad_tip = valid.clone();
        bad_tip.tip_commit = "not-a-hash".to_string();
        assert!(matches!(
            bad_tip.validate(),
            Err(CoreError::InvalidField { field: "c", .. })
        ));

        let mut missing_clone = valid.clone();
        missing_clone.clone = Vec::new();
        assert!(matches!(
            missing_clone.validate(),
            Err(CoreError::MissingField("clone"))
        ));

        let mut bad_clone_entry = valid.clone();
        bad_clone_entry.clone = vec!["   ".to_string()];
        assert!(matches!(
            bad_clone_entry.validate(),
            Err(CoreError::InvalidField {
                field: "clone",
                ..
            })
        ));
    }

    #[test]
    fn pull_request_update_from_tags_requires_repo_and_root_event() {
        let pubkey = hex_of(0x11, 64);
        let missing_repo = PullRequestUpdate::from_tags(&[
            vec!["E".to_string(), hex_of(0x22, 64)],
            vec!["P".to_string(), pubkey.clone()],
            vec!["c".to_string(), hex_of(0x33, 40)],
            vec!["clone".to_string(), "https://git.example/repo.git".to_string()],
        ])
        .expect_err("missing repo should fail");
        assert!(matches!(missing_repo, CoreError::MissingField("a")));

        let missing_root_event = PullRequestUpdate::from_tags(&[
            vec!["a".to_string(), format!("30617:{pubkey}:repo")],
            vec!["P".to_string(), pubkey.clone()],
            vec!["c".to_string(), hex_of(0x33, 40)],
            vec!["clone".to_string(), "https://git.example/repo.git".to_string()],
        ])
        .expect_err("missing root event should fail");
        assert!(matches!(missing_root_event, CoreError::MissingField("E")));

        let missing_tip = PullRequestUpdate::from_tags(&[
            vec!["a".to_string(), format!("30617:{pubkey}:repo")],
            vec!["E".to_string(), hex_of(0x22, 64)],
            vec!["P".to_string(), pubkey],
            vec!["clone".to_string(), "https://git.example/repo.git".to_string()],
        ])
        .expect_err("missing tip commit should fail");
        assert!(matches!(missing_tip, CoreError::MissingField("c")));
    }

    #[test]
    fn pull_request_update_to_tags_skips_empty_optional_fields() {
        let pubkey = hex_of(0x11, 64);
        let update = PullRequestUpdate {
            repo_address: format!("30617:{pubkey}:repo"),
            repo_refs: Vec::new(),
            mentions: Vec::new(),
            root_event_id: hex_of(0x44, 64),
            root_author: hex_of(0x55, 64),
            tip_commit: hex_of(0x66, 40),
            clone: Vec::new(),
            merge_base: None,
        };

        let tags = update.to_tags();
        assert!(!tags.iter().any(|tag| tag.first().is_some_and(|name| name == "clone")));
        assert!(!tags.iter().any(|tag| tag.first().is_some_and(|name| name == "merge-base")));
    }

    #[test]
    fn pull_request_update_validate_rejects_repo_address_refs_and_mentions() {
        let pubkey = hex_of(0x11, 64);
        let valid = PullRequestUpdate {
            repo_address: format!("30617:{pubkey}:repo"),
            repo_refs: vec![hex_of(0x22, 40)],
            mentions: vec![hex_of(0x33, 64)],
            root_event_id: hex_of(0x44, 64),
            root_author: hex_of(0x55, 64),
            tip_commit: hex_of(0x66, 40),
            clone: vec!["https://git.example/repo.git".to_string()],
            merge_base: None,
        };

        let mut bad_repo_address = valid.clone();
        bad_repo_address.repo_address = "30617:nothex:repo".to_string();
        assert!(matches!(
            bad_repo_address.validate(),
            Err(CoreError::InvalidField { field: "a", .. })
        ));

        let mut bad_repo_ref = valid.clone();
        bad_repo_ref.repo_refs = vec!["not-a-hex-hash".to_string()];
        assert!(matches!(
            bad_repo_ref.validate(),
            Err(CoreError::InvalidField { field: "r", .. })
        ));

        let mut bad_mention = valid.clone();
        bad_mention.mentions = vec!["not-a-pubkey".to_string()];
        assert!(matches!(
            bad_mention.validate(),
            Err(CoreError::InvalidField { field: "p", .. })
        ));
    }
}
