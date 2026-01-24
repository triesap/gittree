use crate::nip34_common::{RepoAddress, is_hex_len};
use crate::{CoreError, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForgejoRepo {
    pub owner: String,
    pub name: String,
}

impl ForgejoRepo {
    pub fn new(owner: impl Into<String>, name: impl Into<String>) -> Result<Self> {
        let owner = owner.into();
        let name = name.into();
        if owner.trim().is_empty() || name.trim().is_empty() {
            return Err(CoreError::MissingField("forgejo_repo"));
        }
        if owner.contains('/') || name.contains('/') {
            return Err(CoreError::InvalidField {
                field: "forgejo_repo",
                value: format!("{owner}/{name}"),
            });
        }
        if owner.chars().any(|ch| ch.is_whitespace()) || name.chars().any(|ch| ch.is_whitespace()) {
            return Err(CoreError::InvalidField {
                field: "forgejo_repo",
                value: format!("{owner}/{name}"),
            });
        }
        Ok(Self { owner, name })
    }

    pub fn parse(value: &str) -> Result<Self> {
        let mut parts = value.splitn(3, '/');
        let owner = parts.next().unwrap_or("");
        let name = parts.next().unwrap_or("");
        let extra = parts.next();
        if extra.is_some() {
            return Err(CoreError::InvalidField {
                field: "forgejo_repo",
                value: value.to_string(),
            });
        }
        Self::new(owner, name)
    }

    pub fn full_name(&self) -> String {
        format!("{}/{}", self.owner, self.name)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoMapping {
    pub forgejo: ForgejoRepo,
    pub pubkey: String,
    pub identifier: String,
}

impl RepoMapping {
    pub fn new(
        forgejo_owner: impl Into<String>,
        forgejo_repo: impl Into<String>,
        pubkey: impl Into<String>,
        identifier: impl Into<String>,
    ) -> Result<Self> {
        let pubkey = pubkey.into();
        let identifier = identifier.into();
        if identifier.trim().is_empty() {
            return Err(CoreError::MissingField("identifier"));
        }
        if !is_hex_len(&pubkey, 64) {
            return Err(CoreError::InvalidField {
                field: "pubkey",
                value: pubkey,
            });
        }
        let forgejo = ForgejoRepo::new(forgejo_owner, forgejo_repo)?;
        let _address = RepoAddress::new(pubkey.clone(), identifier.clone())?;
        Ok(Self {
            forgejo,
            pubkey,
            identifier,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{ForgejoRepo, RepoMapping};

    #[test]
    fn forgejo_repo_parses_full_name() {
        let repo = ForgejoRepo::parse("owner/repo").expect("repo");
        assert_eq!(repo.owner, "owner");
        assert_eq!(repo.name, "repo");
        assert_eq!(repo.full_name(), "owner/repo");
    }

    #[test]
    fn forgejo_repo_rejects_missing_parts() {
        assert!(ForgejoRepo::parse("owner").is_err());
        assert!(ForgejoRepo::parse("/repo").is_err());
        assert!(ForgejoRepo::parse("owner/").is_err());
    }

    #[test]
    fn forgejo_repo_rejects_extra_segments() {
        assert!(ForgejoRepo::parse("owner/repo/extra").is_err());
    }

    #[test]
    fn forgejo_repo_rejects_whitespace() {
        assert!(ForgejoRepo::parse("own er/repo").is_err());
        assert!(ForgejoRepo::parse("owner/re po").is_err());
    }

    #[test]
    fn repo_mapping_accepts_valid_fields() {
        let pubkey = "11".repeat(32);
        let mapping = RepoMapping::new("owner", "repo", pubkey.clone(), "repo")
            .expect("mapping");
        assert_eq!(mapping.pubkey, pubkey);
        assert_eq!(mapping.identifier, "repo");
        assert_eq!(mapping.forgejo.full_name(), "owner/repo");
    }

    #[test]
    fn repo_mapping_rejects_invalid_pubkey() {
        assert!(RepoMapping::new("owner", "repo", "bad", "repo").is_err());
    }

    #[test]
    fn repo_mapping_rejects_missing_identifier() {
        let pubkey = "11".repeat(32);
        assert!(RepoMapping::new("owner", "repo", pubkey, "").is_err());
    }
}
