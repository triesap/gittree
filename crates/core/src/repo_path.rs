use crate::{CoreError, Result};
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoPath {
    pub npub: String,
    pub identifier: String,
    pub pubkey: String,
}

pub fn parse_repo_path(path: impl AsRef<Path>) -> Result<RepoPath> {
    let path = path.as_ref();
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or(CoreError::MissingField("repo_path"))?;

    let identifier = name.strip_suffix(".git").unwrap_or(name).to_string();
    if identifier.trim().is_empty() {
        return Err(CoreError::MissingField("identifier"));
    }

    let npub = path
        .parent()
        .and_then(|parent| parent.file_name())
        .and_then(|name| name.to_str())
        .ok_or(CoreError::MissingField("npub"))?
        .to_string();

    let pubkey = decode_npub_pubkey(&npub)?;

    Ok(RepoPath {
        npub,
        identifier,
        pubkey,
    })
}

fn decode_npub_pubkey(npub: &str) -> Result<String> {
    let (hrp, data) = bech32::decode(npub).map_err(|_| CoreError::InvalidField {
        field: "npub",
        value: npub.to_string(),
    })?;

    if hrp.as_str() != "npub" {
        return Err(CoreError::InvalidField {
            field: "npub",
            value: npub.to_string(),
        });
    }

    if data.len() != 32 {
        return Err(CoreError::InvalidField {
            field: "npub",
            value: npub.to_string(),
        });
    }

    Ok(bytes_to_hex(&data))
}

fn bytes_to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{:02x}", byte)).collect()
}

#[cfg(test)]
mod tests {
    use super::parse_repo_path;
    use std::path::Path;

    const SAMPLE_NPUB: &str = "npub1gjttreegkzys8jlhdnfm3qe39h2gka79cpndd0jsms5fk7tuhcnsdw56jq";

    #[test]
    fn parse_repo_path_handles_git_suffix() {
        let path = Path::new("/var/lib/gittree")
            .join(SAMPLE_NPUB)
            .join("repo.git");
        let parsed = parse_repo_path(&path).expect("parse repo path");
        assert_eq!(parsed.npub, SAMPLE_NPUB);
        assert_eq!(parsed.identifier, "repo");
        assert_eq!(parsed.pubkey.len(), 64);
        assert!(parsed.pubkey.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn parse_repo_path_handles_no_git_suffix() {
        let path = Path::new("/var/lib/gittree").join(SAMPLE_NPUB).join("repo");
        let parsed = parse_repo_path(&path).expect("parse repo path");
        assert_eq!(parsed.identifier, "repo");
    }

    #[test]
    fn parse_repo_path_rejects_missing_parent() {
        let path = Path::new("repo.git");
        assert!(parse_repo_path(path).is_err());
    }

    #[test]
    fn parse_repo_path_rejects_invalid_npub() {
        let path = Path::new("/var/lib/gittree")
            .join("invalid")
            .join("repo.git");
        assert!(parse_repo_path(&path).is_err());
    }

    #[test]
    fn parse_repo_path_rejects_empty_identifier() {
        let path = Path::new("/var/lib/gittree").join(SAMPLE_NPUB).join(".git");
        assert!(parse_repo_path(&path).is_err());
    }
}
