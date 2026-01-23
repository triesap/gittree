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
}
