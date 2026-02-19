use crate::nip34_common::{HEX_PUBKEY_LEN, is_hex_len, validate_repo_address};
use crate::tags::push_unique;
use crate::{CoreError, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    pub repo_address: String,
    pub mentions: Vec<String>,
    pub subject: Option<String>,
    pub labels: Vec<String>,
}

impl Issue {
    pub fn from_tags(tags: &[Vec<String>]) -> Result<Self> {
        let mut repo_address = None;
        let mut mentions = Vec::new();
        let mut subject = None;
        let mut labels = Vec::new();

        for tag in tags {
            match tag.as_slice() {
                [t, value, ..] if t == "a" => repo_address = Some(value.clone()),
                [t, value, ..] if t == "p" => push_unique(&mut mentions, value),
                [t, value, ..] if t == "subject" => subject = Some(value.clone()),
                [t, value, ..] if t == "t" => push_unique(&mut labels, value),
                _ => {}
            }
        }

        Ok(Self {
            repo_address: repo_address.ok_or(CoreError::MissingField("a"))?,
            mentions,
            subject,
            labels,
        })
    }

    pub fn to_tags(&self) -> Vec<Vec<String>> {
        let mut tags = Vec::new();

        tags.push(vec!["a".to_string(), self.repo_address.clone()]);

        for mention in &self.mentions {
            tags.push(vec!["p".to_string(), mention.clone()]);
        }

        if let Some(subject) = &self.subject {
            tags.push(vec!["subject".to_string(), subject.clone()]);
        }

        for label in &self.labels {
            tags.push(vec!["t".to_string(), label.clone()]);
        }

        tags
    }

    pub fn validate(&self) -> Result<()> {
        validate_repo_address(&self.repo_address)?;

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

        for label in &self.labels {
            if label.trim().is_empty() {
                return Err(CoreError::InvalidField {
                    field: "t",
                    value: label.clone(),
                });
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::Issue;

    fn hex_of(byte: u8, len: usize) -> String {
        format!("{:02x}", byte).repeat(len / 2)
    }

    #[test]
    fn issue_round_trips_tags() {
        let pubkey = hex_of(0x11, 64);
        let issue = Issue {
            repo_address: format!("30617:{pubkey}:repo"),
            mentions: vec![hex_of(0x22, 64)],
            subject: Some("Bug report".to_string()),
            labels: vec!["bug".to_string(), "critical".to_string()],
        };

        let tags = issue.to_tags();
        let parsed = Issue::from_tags(&tags).expect("parse");
        assert_eq!(parsed, issue);
        parsed.validate().expect("valid");
    }

    #[test]
    fn issue_to_tags_omits_subject_when_none() {
        let pubkey = hex_of(0x11, 64);
        let issue = Issue {
            repo_address: format!("30617:{pubkey}:repo"),
            mentions: vec![hex_of(0x22, 64)],
            subject: None,
            labels: vec!["bug".to_string()],
        };

        let tags = issue.to_tags();
        assert_eq!(tags.len(), 3);
        assert_eq!(tags[0], vec!["a".to_string(), format!("30617:{pubkey}:repo")]);
        assert_eq!(tags[1], vec!["p".to_string(), hex_of(0x22, 64)]);
        assert_eq!(tags[2], vec!["t".to_string(), "bug".to_string()]);
    }

    #[test]
    fn issue_rejects_empty_subject() {
        let pubkey = hex_of(0x11, 64);
        let issue = Issue {
            repo_address: format!("30617:{pubkey}:repo"),
            mentions: Vec::new(),
            subject: Some("".to_string()),
            labels: Vec::new(),
        };

        assert!(issue.validate().is_err());
    }

    #[test]
    fn issue_from_tags_requires_repo_address() {
        let tags = vec![vec!["subject".to_string(), "missing repo".to_string()]];
        let err = Issue::from_tags(&tags).unwrap_err();
        assert!(matches!(err, crate::CoreError::MissingField("a")));
    }

    #[test]
    fn issue_from_tags_dedupes_mentions_and_labels() {
        let pubkey = hex_of(0x11, 64);
        let mention = hex_of(0x22, 64);
        let tags = vec![
            vec!["a".to_string(), format!("30617:{pubkey}:repo")],
            vec!["p".to_string(), mention.clone()],
            vec!["p".to_string(), mention.clone()],
            vec!["t".to_string(), "bug".to_string()],
            vec!["t".to_string(), "bug".to_string()],
        ];

        let issue = Issue::from_tags(&tags).expect("issue");
        assert_eq!(issue.mentions, vec![mention]);
        assert_eq!(issue.labels, vec!["bug".to_string()]);
    }

    #[test]
    fn issue_from_tags_ignores_unknown_tags() {
        let pubkey = hex_of(0x11, 64);
        let tags = vec![
            vec!["a".to_string(), format!("30617:{pubkey}:repo")],
            vec!["x-unknown".to_string(), "value".to_string()],
        ];
        let issue = Issue::from_tags(&tags).expect("issue");
        assert_eq!(issue.repo_address, format!("30617:{pubkey}:repo"));
        assert!(issue.mentions.is_empty());
        assert!(issue.labels.is_empty());
    }

    #[test]
    fn issue_validate_rejects_invalid_mentions_and_labels() {
        let pubkey = hex_of(0x11, 64);
        let bad_mention = Issue {
            repo_address: format!("30617:{pubkey}:repo"),
            mentions: vec!["nothex".to_string()],
            subject: None,
            labels: Vec::new(),
        };
        assert!(matches!(
            bad_mention.validate(),
            Err(crate::CoreError::InvalidField { field: "p", .. })
        ));

        let bad_label = Issue {
            repo_address: format!("30617:{pubkey}:repo"),
            mentions: Vec::new(),
            subject: None,
            labels: vec![" ".to_string()],
        };
        assert!(matches!(
            bad_label.validate(),
            Err(crate::CoreError::InvalidField { field: "t", .. })
        ));
    }

    #[test]
    fn issue_validate_rejects_invalid_repo_address() {
        let issue = Issue {
            repo_address: "bad".to_string(),
            mentions: Vec::new(),
            subject: None,
            labels: Vec::new(),
        };

        assert!(matches!(
            issue.validate(),
            Err(crate::CoreError::InvalidField { field: "a", .. })
        ));
    }
}
