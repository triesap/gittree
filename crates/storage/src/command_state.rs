use crate::StorageError;
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandStatus {
    Ok,
    Error,
}

impl CommandStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            CommandStatus::Ok => "ok",
            CommandStatus::Error => "error",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct CommandLogRecord {
    pub event_id: Vec<u8>,
    pub pubkey: Vec<u8>,
    pub namespace: String,
    pub action: String,
    pub target: Option<String>,
    pub args_json: Value,
    pub status: CommandStatus,
    pub code: String,
    pub message: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccountLifecycle {
    Active,
    Deleted,
}

impl AccountLifecycle {
    pub fn as_str(&self) -> &'static str {
        match self {
            AccountLifecycle::Active => "active",
            AccountLifecycle::Deleted => "deleted",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountStateRecord {
    pub pubkey: Vec<u8>,
    pub status: AccountLifecycle,
    pub created_at: i64,
    pub updated_at: i64,
    pub deleted_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProfileVisibilityV1 {
    Public,
    Private,
}

impl ProfileVisibilityV1 {
    pub fn as_str(&self) -> &'static str {
        match self {
            ProfileVisibilityV1::Public => "public",
            ProfileVisibilityV1::Private => "private",
        }
    }

    pub fn parse(value: &str) -> Result<Self, StorageError> {
        match value {
            "public" => Ok(Self::Public),
            "private" => Ok(Self::Private),
            _ => Err(StorageError::InvalidField {
                field: "visibility",
                value: value.to_string(),
            }),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileStateRecord {
    pub pubkey: Vec<u8>,
    pub display_name: Option<String>,
    pub bio: Option<String>,
    pub avatar_url: Option<String>,
    pub website_url: Option<String>,
    pub location: Option<String>,
    pub visibility: ProfileVisibilityV1,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepoVisibilityV1 {
    Public,
    Private,
}

impl RepoVisibilityV1 {
    pub fn as_str(&self) -> &'static str {
        match self {
            RepoVisibilityV1::Public => "public",
            RepoVisibilityV1::Private => "private",
        }
    }

    pub fn parse(value: &str) -> Result<Self, StorageError> {
        match value {
            "public" => Ok(Self::Public),
            "private" => Ok(Self::Private),
            _ => Err(StorageError::InvalidField {
                field: "visibility",
                value: value.to_string(),
            }),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoStateV1Record {
    pub owner_pubkey: Vec<u8>,
    pub repo_name: String,
    pub description: Option<String>,
    pub website_url: Option<String>,
    pub visibility: RepoVisibilityV1,
    pub default_branch: String,
    pub archived: bool,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoMaintainerV1Record {
    pub owner_pubkey: Vec<u8>,
    pub repo_name: String,
    pub maintainer_pubkey: Vec<u8>,
    pub active: bool,
    pub updated_at: i64,
}

#[cfg(test)]
mod tests {
    use super::{
        AccountLifecycle, AccountStateRecord, CommandLogRecord, CommandStatus, ProfileStateRecord,
        ProfileVisibilityV1, RepoMaintainerV1Record, RepoStateV1Record, RepoVisibilityV1,
    };
    use crate::StorageError;
    use serde_json::json;

    #[test]
    fn enum_string_mappings_cover_all_variants() {
        assert_eq!(CommandStatus::Ok.as_str(), "ok");
        assert_eq!(CommandStatus::Error.as_str(), "error");
        assert_eq!(AccountLifecycle::Active.as_str(), "active");
        assert_eq!(AccountLifecycle::Deleted.as_str(), "deleted");
        assert_eq!(ProfileVisibilityV1::Public.as_str(), "public");
        assert_eq!(ProfileVisibilityV1::Private.as_str(), "private");
        assert_eq!(RepoVisibilityV1::Public.as_str(), "public");
        assert_eq!(RepoVisibilityV1::Private.as_str(), "private");
    }

    #[test]
    fn visibility_parsers_accept_known_values() {
        assert_eq!(
            ProfileVisibilityV1::parse("public").expect("profile visibility"),
            ProfileVisibilityV1::Public
        );
        assert_eq!(
            ProfileVisibilityV1::parse("private").expect("profile visibility"),
            ProfileVisibilityV1::Private
        );
        assert_eq!(
            RepoVisibilityV1::parse("public").expect("repo visibility"),
            RepoVisibilityV1::Public
        );
        assert_eq!(
            RepoVisibilityV1::parse("private").expect("repo visibility"),
            RepoVisibilityV1::Private
        );
    }

    #[test]
    fn visibility_parsers_reject_unknown_values() {
        let profile_err = ProfileVisibilityV1::parse("friends-only").expect_err("profile parse");
        assert!(matches!(
            profile_err,
            StorageError::InvalidField { field, value }
            if field == "visibility" && value == "friends-only"
        ));

        let repo_err = RepoVisibilityV1::parse("internal").expect_err("repo parse");
        assert!(matches!(
            repo_err,
            StorageError::InvalidField { field, value }
            if field == "visibility" && value == "internal"
        ));
    }

    #[test]
    fn record_structs_round_trip_clone_and_eq() {
        let command = CommandLogRecord {
            event_id: vec![1; 32],
            pubkey: vec![2; 32],
            namespace: "account".to_string(),
            action: "create".to_string(),
            target: Some("demo".to_string()),
            args_json: json!({"visibility":"public"}),
            status: CommandStatus::Ok,
            code: "ok".to_string(),
            message: "ok".to_string(),
            created_at: 123,
        };
        assert_eq!(command, command.clone());

        let account = AccountStateRecord {
            pubkey: vec![3; 32],
            status: AccountLifecycle::Active,
            created_at: 10,
            updated_at: 20,
            deleted_at: None,
        };
        assert_eq!(account, account.clone());

        let profile = ProfileStateRecord {
            pubkey: vec![4; 32],
            display_name: Some("alice".to_string()),
            bio: Some("hello".to_string()),
            avatar_url: Some("https://gittr.ee/a.png".to_string()),
            website_url: Some("https://gittr.ee".to_string()),
            location: Some("earth".to_string()),
            visibility: ProfileVisibilityV1::Public,
            updated_at: 30,
        };
        assert_eq!(profile, profile.clone());

        let repo = RepoStateV1Record {
            owner_pubkey: vec![5; 32],
            repo_name: "demo".to_string(),
            description: Some("repo".to_string()),
            website_url: Some("https://gittr.ee/demo".to_string()),
            visibility: RepoVisibilityV1::Private,
            default_branch: "main".to_string(),
            archived: false,
            updated_at: 40,
        };
        assert_eq!(repo, repo.clone());

        let maintainer = RepoMaintainerV1Record {
            owner_pubkey: vec![6; 32],
            repo_name: "demo".to_string(),
            maintainer_pubkey: vec![7; 32],
            active: true,
            updated_at: 50,
        };
        assert_eq!(maintainer, maintainer.clone());
    }
}
