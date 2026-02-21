use crate::nip34_common::{
    HEX_EVENT_ID_LEN, HEX_PUBKEY_LEN, is_hex_hash, is_hex_len, validate_repo_address,
};
use crate::tags::push_unique;
use crate::{CoreError, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitterTag {
    pub name: String,
    pub email: String,
    pub timestamp: i64,
    pub timezone_minutes: i32,
}

impl CommitterTag {
    fn from_tag(tag: &[String]) -> Result<Self> {
        let name = tag.get(1).cloned().unwrap_or_default();
        let email = tag.get(2).cloned().unwrap_or_default();
        let timestamp = tag.get(3).and_then(|value| value.parse().ok());
        let timezone_minutes = tag.get(4).and_then(|value| value.parse().ok());

        if name.trim().is_empty()
            || email.trim().is_empty()
            || timestamp.is_none()
            || timezone_minutes.is_none()
        {
            return Err(CoreError::InvalidField {
                field: "committer",
                value: tag.join(" "),
            });
        }

        Ok(Self {
            name,
            email,
            timestamp: timestamp.expect("checked"),
            timezone_minutes: timezone_minutes.expect("checked"),
        })
    }

    fn to_tag(&self) -> Vec<String> {
        vec![
            "committer".to_string(),
            self.name.clone(),
            self.email.clone(),
            self.timestamp.to_string(),
            self.timezone_minutes.to_string(),
        ]
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Patch {
    pub repo_address: String,
    pub repo_refs: Vec<String>,
    pub mentions: Vec<String>,
    pub root_event: Option<String>,
    pub reply_event: Option<String>,
    pub is_root: bool,
    pub is_root_revision: bool,
    pub commit: Option<String>,
    pub parent_commit: Option<String>,
    pub commit_pgp_sig: Option<String>,
    pub committer: Option<CommitterTag>,
}

impl Patch {
    pub fn from_tags(tags: &[Vec<String>]) -> Result<Self> {
        let mut repo_address = None;
        let mut repo_refs = Vec::new();
        let mut mentions = Vec::new();
        let mut root_event = None;
        let mut reply_event = None;
        let mut is_root = false;
        let mut is_root_revision = false;
        let mut commit = None;
        let mut parent_commit = None;
        let mut commit_pgp_sig = None;
        let mut committer = None;

        for tag in tags {
            match tag.as_slice() {
                [t, value, ..] if t == "a" => repo_address = Some(value.clone()),
                [t, value, ..] if t == "r" => push_unique(&mut repo_refs, value),
                [t, value, ..] if t == "p" => push_unique(&mut mentions, value),
                [t, value, ..] if t == "t" && value == "root" => is_root = true,
                [t, value, ..] if t == "t" && value == "root-revision" => is_root_revision = true,
                [t, value, ..] if t == "commit" => commit = Some(value.clone()),
                [t, value, ..] if t == "parent-commit" => parent_commit = Some(value.clone()),
                [t, value, ..] if t == "commit-pgp-sig" => commit_pgp_sig = Some(value.clone()),
                [t, ..] if t == "committer" => {
                    committer = Some(CommitterTag::from_tag(tag)?);
                }
                _ => {
                    if let Some((event_id, marker)) = parse_e_tag(tag) {
                        match marker {
                            Some("root") => root_event = Some(event_id.to_string()),
                            Some("reply") => reply_event = Some(event_id.to_string()),
                            _ => {}
                        }
                    }
                }
            }
        }

        Ok(Self {
            repo_address: repo_address.ok_or(CoreError::MissingField("a"))?,
            repo_refs,
            mentions,
            root_event,
            reply_event,
            is_root,
            is_root_revision,
            commit,
            parent_commit,
            commit_pgp_sig,
            committer,
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

        if self.is_root {
            tags.push(vec!["t".to_string(), "root".to_string()]);
        }

        if self.is_root_revision {
            tags.push(vec!["t".to_string(), "root-revision".to_string()]);
        }

        if let Some(commit) = &self.commit {
            tags.push(vec!["commit".to_string(), commit.clone()]);
        }

        if let Some(parent_commit) = &self.parent_commit {
            tags.push(vec!["parent-commit".to_string(), parent_commit.clone()]);
        }

        if let Some(sig) = &self.commit_pgp_sig {
            tags.push(vec!["commit-pgp-sig".to_string(), sig.clone()]);
        }

        if let Some(root_event) = &self.root_event {
            tags.push(vec![
                "e".to_string(),
                root_event.clone(),
                "".to_string(),
                "root".to_string(),
            ]);
        }

        if let Some(reply_event) = &self.reply_event {
            tags.push(vec![
                "e".to_string(),
                reply_event.clone(),
                "".to_string(),
                "reply".to_string(),
            ]);
        }

        if let Some(committer) = &self.committer {
            tags.push(committer.to_tag());
        }

        tags
    }

    pub fn validate(&self) -> Result<()> {
        validate_repo_address(&self.repo_address)?;

        if self.is_root_revision && !self.is_root {
            return Err(CoreError::InvalidField {
                field: "t",
                value: "root-revision without root".to_string(),
            });
        }

        if let Some(root_event) = &self.root_event {
            if !is_hex_len(root_event, HEX_EVENT_ID_LEN) {
                return Err(CoreError::InvalidField {
                    field: "e",
                    value: root_event.clone(),
                });
            }
        }

        if let Some(reply_event) = &self.reply_event {
            if !is_hex_len(reply_event, HEX_EVENT_ID_LEN) {
                return Err(CoreError::InvalidField {
                    field: "e",
                    value: reply_event.clone(),
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

        if let Some(commit) = &self.commit {
            if !is_hex_hash(commit) {
                return Err(CoreError::InvalidField {
                    field: "commit",
                    value: commit.clone(),
                });
            }
        }

        if let Some(parent_commit) = &self.parent_commit {
            if !is_hex_hash(parent_commit) {
                return Err(CoreError::InvalidField {
                    field: "parent-commit",
                    value: parent_commit.clone(),
                });
            }
        }

        Ok(())
    }
}

fn parse_e_tag(tag: &[String]) -> Option<(&str, Option<&str>)> {
    if tag.len() < 2 || tag.first().map(|t| t.as_str()) != Some("e") {
        return None;
    }

    let event_id = tag[1].as_str();
    let marker = tag.get(3).map(String::as_str);
    Some((event_id, marker))
}

#[cfg(test)]
mod tests {
    use super::CommitterTag;
    use super::Patch;
    use super::parse_e_tag;

    fn hex_of(byte: u8, len: usize) -> String {
        format!("{:02x}", byte).repeat(len / 2)
    }

    #[test]
    fn patch_round_trips_tags() {
        let pubkey = hex_of(0x11, 64);
        let patch = Patch {
            repo_address: format!("30617:{pubkey}:repo"),
            repo_refs: vec![hex_of(0x22, 40)],
            mentions: vec![hex_of(0x33, 64)],
            root_event: Some(hex_of(0x44, 64)),
            reply_event: Some(hex_of(0x55, 64)),
            is_root: true,
            is_root_revision: true,
            commit: Some(hex_of(0x66, 40)),
            parent_commit: Some(hex_of(0x77, 40)),
            commit_pgp_sig: Some("".to_string()),
            committer: Some(CommitterTag {
                name: "Alice".to_string(),
                email: "alice@example.com".to_string(),
                timestamp: 1700000000,
                timezone_minutes: -60,
            }),
        };

        let tags = patch.to_tags();
        let parsed = Patch::from_tags(&tags).expect("parse");
        assert_eq!(parsed, patch);
        parsed.validate().expect("valid");
    }

    #[test]
    fn patch_validation_rejects_bad_commit() {
        let pubkey = hex_of(0x11, 64);
        let patch = Patch {
            repo_address: format!("30617:{pubkey}:repo"),
            repo_refs: Vec::new(),
            mentions: Vec::new(),
            root_event: None,
            reply_event: None,
            is_root: false,
            is_root_revision: false,
            commit: Some("bad".to_string()),
            parent_commit: None,
            commit_pgp_sig: None,
            committer: None,
        };

        assert!(patch.validate().is_err());
    }

    #[test]
    fn patch_rejects_root_revision_without_root() {
        let pubkey = hex_of(0x11, 64);
        let patch = Patch {
            repo_address: format!("30617:{pubkey}:repo"),
            repo_refs: Vec::new(),
            mentions: Vec::new(),
            root_event: None,
            reply_event: None,
            is_root: false,
            is_root_revision: true,
            commit: None,
            parent_commit: None,
            commit_pgp_sig: None,
            committer: None,
        };

        assert!(patch.validate().is_err());
    }

    #[test]
    fn patch_from_tags_requires_repo_address() {
        let tags = vec![vec!["t".to_string(), "root".to_string()]];
        let err = Patch::from_tags(&tags).unwrap_err();
        assert!(matches!(err, crate::CoreError::MissingField("a")));
    }

    #[test]
    fn patch_from_tags_rejects_invalid_committer_tag() {
        let pubkey = hex_of(0x11, 64);
        let tags = vec![
            vec!["a".to_string(), format!("30617:{pubkey}:repo")],
            vec!["committer".to_string(), "Alice".to_string()],
        ];
        assert!(matches!(
            Patch::from_tags(&tags),
            Err(crate::CoreError::InvalidField {
                field: "committer",
                ..
            })
        ));
    }

    #[test]
    fn patch_from_tags_ignores_unhandled_e_markers() {
        let pubkey = hex_of(0x11, 64);
        let tags = vec![
            vec!["a".to_string(), format!("30617:{pubkey}:repo")],
            vec![
                "e".to_string(),
                hex_of(0x22, 64),
                "".to_string(),
                "other".to_string(),
            ],
            vec!["e".to_string(), hex_of(0x33, 64)],
            vec!["x-unknown".to_string(), "ignored".to_string()],
        ];

        let patch = Patch::from_tags(&tags).expect("patch");
        assert!(patch.root_event.is_none());
        assert!(patch.reply_event.is_none());
    }

    #[test]
    fn patch_validate_accepts_without_parent_commit() {
        let pubkey = hex_of(0x11, 64);
        let patch = Patch {
            repo_address: format!("30617:{pubkey}:repo"),
            repo_refs: vec![hex_of(0x22, 40)],
            mentions: vec![hex_of(0x33, 64)],
            root_event: Some(hex_of(0x44, 64)),
            reply_event: None,
            is_root: true,
            is_root_revision: false,
            commit: Some(hex_of(0x55, 40)),
            parent_commit: None,
            commit_pgp_sig: None,
            committer: None,
        };
        patch.validate().expect("valid patch");
    }

    #[test]
    fn patch_to_tags_skips_optional_fields_when_absent() {
        let pubkey = hex_of(0x11, 64);
        let patch = Patch {
            repo_address: format!("30617:{pubkey}:repo"),
            repo_refs: Vec::new(),
            mentions: Vec::new(),
            root_event: None,
            reply_event: None,
            is_root: false,
            is_root_revision: false,
            commit: None,
            parent_commit: None,
            commit_pgp_sig: None,
            committer: None,
        };

        let tags = patch.to_tags();
        assert_eq!(
            tags,
            vec![vec!["a".to_string(), format!("30617:{pubkey}:repo")]]
        );
    }

    #[test]
    fn patch_validation_rejects_invalid_optional_fields() {
        let pubkey = hex_of(0x11, 64);
        let base = Patch {
            repo_address: format!("30617:{pubkey}:repo"),
            repo_refs: Vec::new(),
            mentions: Vec::new(),
            root_event: None,
            reply_event: None,
            is_root: false,
            is_root_revision: false,
            commit: None,
            parent_commit: None,
            commit_pgp_sig: None,
            committer: None,
        };

        let mut patch = base.clone();
        patch.root_event = Some("bad".to_string());
        assert!(matches!(
            patch.validate(),
            Err(crate::CoreError::InvalidField { field: "e", .. })
        ));

        let mut patch = base.clone();
        patch.reply_event = Some("bad".to_string());
        assert!(matches!(
            patch.validate(),
            Err(crate::CoreError::InvalidField { field: "e", .. })
        ));

        let mut patch = base.clone();
        patch.repo_refs = vec!["bad".to_string()];
        assert!(matches!(
            patch.validate(),
            Err(crate::CoreError::InvalidField { field: "r", .. })
        ));

        let mut patch = base.clone();
        patch.mentions = vec!["bad".to_string()];
        assert!(matches!(
            patch.validate(),
            Err(crate::CoreError::InvalidField { field: "p", .. })
        ));

        let mut patch = base;
        patch.parent_commit = Some("bad".to_string());
        assert!(matches!(
            patch.validate(),
            Err(crate::CoreError::InvalidField {
                field: "parent-commit",
                ..
            })
        ));
    }

    #[test]
    fn parse_e_tag_handles_short_and_non_e_tags() {
        assert!(parse_e_tag(&[]).is_none());
        assert!(parse_e_tag(&["e".to_string()]).is_none());
        assert!(parse_e_tag(&["p".to_string(), "abc".to_string()]).is_none());
        assert_eq!(
            parse_e_tag(&[
                "e".to_string(),
                "deadbeef".to_string(),
                "".to_string(),
                "root".to_string()
            ]),
            Some(("deadbeef", Some("root")))
        );
    }

    #[test]
    fn patch_validation_rejects_invalid_repo_address() {
        let patch = Patch {
            repo_address: "not-a-repo-address".to_string(),
            repo_refs: Vec::new(),
            mentions: Vec::new(),
            root_event: None,
            reply_event: None,
            is_root: false,
            is_root_revision: false,
            commit: None,
            parent_commit: None,
            commit_pgp_sig: None,
            committer: None,
        };
        assert!(matches!(
            patch.validate(),
            Err(crate::CoreError::InvalidField { field: "a", .. })
        ));
    }
}
