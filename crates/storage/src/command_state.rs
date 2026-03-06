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
