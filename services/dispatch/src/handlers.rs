use crate::DispatchError;
use async_trait::async_trait;
use gittree_app_core::pubkey_bytes_from_npub;
use gittree_core::{CommandArg, CommandNamespace, ParsedCommand};
use gittree_storage::{
    AccountLifecycle, AccountStateRecord, CommandLogRecord, CommandStatus, PostgresRepositories,
    ProfileStateRecord, ProfileVisibilityV1, RepoMaintainerV1Record, RepoStateV1Record,
    RepoVisibilityV1,
};
use serde_json::{Map, Value};

#[derive(Debug, Clone, PartialEq)]
pub struct CommandExecutionInput {
    pub event_id: Vec<u8>,
    pub actor_pubkey: Vec<u8>,
    pub parsed: ParsedCommand,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandExecutionOutput {
    pub status: CommandStatus,
    pub code: String,
    pub message: String,
}

#[async_trait]
pub trait CommandStore: Send + Sync {
    async fn insert_command_log(&self, record: &CommandLogRecord) -> Result<bool, DispatchError>;
    async fn update_command_log_outcome(
        &self,
        event_id: &[u8],
        status: CommandStatus,
        code: &str,
        message: &str,
    ) -> Result<(), DispatchError>;
    async fn account_state(&self, pubkey: &[u8]) -> Result<Option<AccountStateRecord>, DispatchError>;
    async fn upsert_account_state(&self, record: &AccountStateRecord) -> Result<(), DispatchError>;
    async fn profile_state(&self, pubkey: &[u8]) -> Result<Option<ProfileStateRecord>, DispatchError>;
    async fn upsert_profile_state(&self, record: &ProfileStateRecord) -> Result<(), DispatchError>;
    async fn repo_state(
        &self,
        owner_pubkey: &[u8],
        repo_name: &str,
    ) -> Result<Option<RepoStateV1Record>, DispatchError>;
    async fn upsert_repo_state(&self, record: &RepoStateV1Record) -> Result<(), DispatchError>;
    async fn set_repo_maintainer(&self, record: &RepoMaintainerV1Record) -> Result<(), DispatchError>;
    async fn list_active_repo_maintainers(
        &self,
        owner_pubkey: &[u8],
        repo_name: &str,
    ) -> Result<std::collections::HashSet<Vec<u8>>, DispatchError>;
}

#[async_trait]
impl CommandStore for PostgresRepositories {
    async fn insert_command_log(&self, record: &CommandLogRecord) -> Result<bool, DispatchError> {
        self.v1_insert_command_log(record)
            .await
            .map_err(DispatchError::Storage)
    }

    async fn update_command_log_outcome(
        &self,
        event_id: &[u8],
        status: CommandStatus,
        code: &str,
        message: &str,
    ) -> Result<(), DispatchError> {
        self.v1_update_command_log_outcome(event_id, status, code, message)
            .await
            .map_err(DispatchError::Storage)
    }

    async fn account_state(&self, pubkey: &[u8]) -> Result<Option<AccountStateRecord>, DispatchError> {
        self.v1_account_state(pubkey)
            .await
            .map_err(DispatchError::Storage)
    }

    async fn upsert_account_state(&self, record: &AccountStateRecord) -> Result<(), DispatchError> {
        self.v1_upsert_account_state(record)
            .await
            .map_err(DispatchError::Storage)
    }

    async fn profile_state(&self, pubkey: &[u8]) -> Result<Option<ProfileStateRecord>, DispatchError> {
        self.v1_profile_state(pubkey)
            .await
            .map_err(DispatchError::Storage)
    }

    async fn upsert_profile_state(&self, record: &ProfileStateRecord) -> Result<(), DispatchError> {
        self.v1_upsert_profile_state(record)
            .await
            .map_err(DispatchError::Storage)
    }

    async fn repo_state(
        &self,
        owner_pubkey: &[u8],
        repo_name: &str,
    ) -> Result<Option<RepoStateV1Record>, DispatchError> {
        self.v1_repo_state(owner_pubkey, repo_name)
            .await
            .map_err(DispatchError::Storage)
    }

    async fn upsert_repo_state(&self, record: &RepoStateV1Record) -> Result<(), DispatchError> {
        self.v1_upsert_repo_state(record)
            .await
            .map_err(DispatchError::Storage)
    }

    async fn set_repo_maintainer(&self, record: &RepoMaintainerV1Record) -> Result<(), DispatchError> {
        self.v1_set_repo_maintainer(record)
            .await
            .map_err(DispatchError::Storage)
    }

    async fn list_active_repo_maintainers(
        &self,
        owner_pubkey: &[u8],
        repo_name: &str,
    ) -> Result<std::collections::HashSet<Vec<u8>>, DispatchError> {
        self.v1_list_active_repo_maintainers(owner_pubkey, repo_name)
            .await
            .map_err(DispatchError::Storage)
    }
}

pub async fn execute_command(
    store: &dyn CommandStore,
    input: CommandExecutionInput,
) -> Result<CommandExecutionOutput, DispatchError> {
    let mut log_record = CommandLogRecord {
        event_id: input.event_id.clone(),
        pubkey: input.actor_pubkey.clone(),
        namespace: namespace_name(input.parsed.namespace).to_string(),
        action: input.parsed.action.clone(),
        target: input.parsed.target.clone(),
        args_json: args_to_json(&input.parsed.args),
        status: CommandStatus::Ok,
        code: "accepted".to_string(),
        message: "accepted".to_string(),
        created_at: input.created_at,
    };

    let inserted = store.insert_command_log(&log_record).await?;
    if !inserted {
        return Ok(CommandExecutionOutput {
            status: CommandStatus::Ok,
            code: "duplicate".to_string(),
            message: "already processed".to_string(),
        });
    }

    let outcome = match apply_command(store, &input).await {
        Ok(outcome) => outcome,
        Err(err) => CommandExecutionOutput {
            status: CommandStatus::Error,
            code: "internal".to_string(),
            message: err.to_string(),
        },
    };

    log_record.status = outcome.status.clone();
    log_record.code = outcome.code.clone();
    log_record.message = outcome.message.clone();

    store
        .update_command_log_outcome(
            &log_record.event_id,
            log_record.status.clone(),
            &log_record.code,
            &log_record.message,
        )
        .await?;

    Ok(outcome)
}

async fn apply_command(
    store: &dyn CommandStore,
    input: &CommandExecutionInput,
) -> Result<CommandExecutionOutput, DispatchError> {
    match input.parsed.namespace {
        CommandNamespace::Account => apply_account(store, input).await,
        CommandNamespace::Profile => apply_profile(store, input).await,
        CommandNamespace::Repo => apply_repo(store, input).await,
    }
}

async fn apply_account(
    store: &dyn CommandStore,
    input: &CommandExecutionInput,
) -> Result<CommandExecutionOutput, DispatchError> {
    let now = input.created_at;
    let existing = store.account_state(&input.actor_pubkey).await?;
    match input.parsed.action.as_str() {
        "create" => {
            if existing.is_none() {
                store
                    .upsert_account_state(&AccountStateRecord {
                        pubkey: input.actor_pubkey.clone(),
                        status: AccountLifecycle::Active,
                        created_at: now,
                        updated_at: now,
                        deleted_at: None,
                    })
                    .await?;
                Ok(ok("account_created", "account created"))
            } else {
                Ok(ok("account_exists", "account already exists"))
            }
        }
        "delete" => {
            let created_at = existing.as_ref().map(|record| record.created_at).unwrap_or(now);
            store
                .upsert_account_state(&AccountStateRecord {
                    pubkey: input.actor_pubkey.clone(),
                    status: AccountLifecycle::Deleted,
                    created_at,
                    updated_at: now,
                    deleted_at: Some(now),
                })
                .await?;
            Ok(ok("account_deleted", "account deleted"))
        }
        "status" => {
            let message = existing
                .map(|record| format!("account {}", record.status.as_str()))
                .unwrap_or_else(|| "account missing".to_string());
            Ok(ok("account_status", &message))
        }
        _ => Ok(err("invalid_command", "unsupported account action")),
    }
}

async fn apply_profile(
    store: &dyn CommandStore,
    input: &CommandExecutionInput,
) -> Result<CommandExecutionOutput, DispatchError> {
    let now = input.created_at;
    let mut profile = store
        .profile_state(&input.actor_pubkey)
        .await?
        .unwrap_or(ProfileStateRecord {
            pubkey: input.actor_pubkey.clone(),
            display_name: None,
            bio: None,
            avatar_url: None,
            website_url: None,
            location: None,
            visibility: ProfileVisibilityV1::Private,
            updated_at: now,
        });

    match input.parsed.action.as_str() {
        "set" => {
            for argument in &input.parsed.args {
                if let CommandArg::KeyValue { key, value } = argument {
                    match key.as_str() {
                        "name" => profile.display_name = Some(value.clone()),
                        "bio" => profile.bio = Some(value.clone()),
                        "avatar" => profile.avatar_url = Some(value.clone()),
                        "website" => profile.website_url = Some(value.clone()),
                        "location" => profile.location = Some(value.clone()),
                        _ => {}
                    }
                }
            }
            profile.updated_at = now;
            store.upsert_profile_state(&profile).await?;
            Ok(ok("profile_updated", "profile updated"))
        }
        "visibility" => {
            if let [CommandArg::Positional(value)] = input.parsed.args.as_slice() {
                profile.visibility = if value == "public" {
                    ProfileVisibilityV1::Public
                } else {
                    ProfileVisibilityV1::Private
                };
                profile.updated_at = now;
                store.upsert_profile_state(&profile).await?;
                Ok(ok("profile_visibility_updated", "profile visibility updated"))
            } else {
                Ok(err("invalid_args", "profile visibility requires public|private"))
            }
        }
        _ => Ok(err("invalid_command", "unsupported profile action")),
    }
}

async fn apply_repo(
    store: &dyn CommandStore,
    input: &CommandExecutionInput,
) -> Result<CommandExecutionOutput, DispatchError> {
    let repo_name = match input.parsed.target.as_deref() {
        Some(name) => name,
        None => return Ok(err("invalid_args", "repo target is required")),
    };
    let now = input.created_at;

    match input.parsed.action.as_str() {
        "create" => {
            let existing = store.repo_state(&input.actor_pubkey, repo_name).await?;
            if existing.is_some() {
                return Ok(ok("repo_exists", "repo already exists"));
            }
            store
                .upsert_repo_state(&RepoStateV1Record {
                    owner_pubkey: input.actor_pubkey.clone(),
                    repo_name: repo_name.to_string(),
                    description: None,
                    website_url: None,
                    visibility: RepoVisibilityV1::Private,
                    default_branch: "main".to_string(),
                    archived: false,
                    updated_at: now,
                })
                .await?;
            store
                .set_repo_maintainer(&RepoMaintainerV1Record {
                    owner_pubkey: input.actor_pubkey.clone(),
                    repo_name: repo_name.to_string(),
                    maintainer_pubkey: input.actor_pubkey.clone(),
                    active: true,
                    updated_at: now,
                })
                .await?;
            Ok(ok("repo_created", "repo created"))
        }
        "update" => {
            let mut state = match store.repo_state(&input.actor_pubkey, repo_name).await? {
                Some(state) => state,
                None => return Ok(err("not_found", "repo not found")),
            };
            if !actor_is_maintainer(store, &input.actor_pubkey, repo_name).await? {
                return Ok(err("unauthorized", "actor is not a maintainer"));
            }
            for arg in &input.parsed.args {
                if let CommandArg::KeyValue { key, value } = arg {
                    match key.as_str() {
                        "description" => state.description = Some(value.clone()),
                        "website" => state.website_url = Some(value.clone()),
                        "visibility" => {
                            state.visibility = if value == "public" {
                                RepoVisibilityV1::Public
                            } else {
                                RepoVisibilityV1::Private
                            }
                        }
                        "default_branch" => state.default_branch = value.clone(),
                        _ => {}
                    }
                }
            }
            state.updated_at = now;
            store.upsert_repo_state(&state).await?;
            Ok(ok("repo_updated", "repo updated"))
        }
        "archive" | "unarchive" => {
            let mut state = match store.repo_state(&input.actor_pubkey, repo_name).await? {
                Some(state) => state,
                None => return Ok(err("not_found", "repo not found")),
            };
            if !actor_is_maintainer(store, &input.actor_pubkey, repo_name).await? {
                return Ok(err("unauthorized", "actor is not a maintainer"));
            }
            state.archived = input.parsed.action == "archive";
            state.updated_at = now;
            store.upsert_repo_state(&state).await?;
            Ok(ok("repo_state_updated", "repo archive state updated"))
        }
        "maintainers" => {
            if !actor_is_maintainer(store, &input.actor_pubkey, repo_name).await? {
                return Ok(err("unauthorized", "actor is not a maintainer"));
            }
            match input.parsed.args.as_slice() {
                [CommandArg::Positional(action), CommandArg::Positional(npub)] => {
                    let maintainer_pubkey = match pubkey_bytes_from_npub(npub) {
                        Ok(bytes) => bytes,
                        Err(_) => return Ok(err("invalid_args", "invalid maintainer npub")),
                    };
                    store
                        .set_repo_maintainer(&RepoMaintainerV1Record {
                            owner_pubkey: input.actor_pubkey.clone(),
                            repo_name: repo_name.to_string(),
                            maintainer_pubkey,
                            active: action == "add",
                            updated_at: now,
                        })
                        .await?;
                    Ok(ok("repo_maintainer_updated", "repo maintainer updated"))
                }
                _ => Ok(err("invalid_args", "repo maintainers requires add|remove and npub")),
            }
        }
        "announce" => Ok(ok("repo_announce_accepted", "repo announcement accepted")),
        "sync" => Ok(ok("repo_sync_accepted", "repo sync accepted")),
        _ => Ok(err("invalid_command", "unsupported repo action")),
    }
}

async fn actor_is_maintainer(
    store: &dyn CommandStore,
    actor: &[u8],
    repo_name: &str,
) -> Result<bool, DispatchError> {
    let maintainers = store.list_active_repo_maintainers(actor, repo_name).await?;
    Ok(maintainers.contains(actor))
}

fn args_to_json(args: &[CommandArg]) -> Value {
    let mut object = Map::new();
    let mut positional = Vec::new();

    for arg in args {
        match arg {
            CommandArg::Positional(value) => positional.push(Value::String(value.clone())),
            CommandArg::KeyValue { key, value } => {
                object.insert(key.clone(), Value::String(value.clone()));
            }
        }
    }

    object.insert("_positional".to_string(), Value::Array(positional));
    Value::Object(object)
}

fn namespace_name(namespace: CommandNamespace) -> &'static str {
    match namespace {
        CommandNamespace::Account => "account",
        CommandNamespace::Profile => "profile",
        CommandNamespace::Repo => "repo",
    }
}

fn ok(code: &str, message: &str) -> CommandExecutionOutput {
    CommandExecutionOutput {
        status: CommandStatus::Ok,
        code: code.to_string(),
        message: message.to_string(),
    }
}

fn err(code: &str, message: &str) -> CommandExecutionOutput {
    CommandExecutionOutput {
        status: CommandStatus::Error,
        code: code.to_string(),
        message: message.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CommandExecutionInput, CommandStore, args_to_json, err, execute_command, namespace_name, ok,
    };
    use crate::DispatchError;
    use async_trait::async_trait;
    use gittree_app_core::npub_from_bytes;
    use gittree_core::{CommandArg, CommandNamespace, ParsedCommand};
    use gittree_storage::{
        AccountLifecycle, AccountStateRecord, CommandLogRecord, CommandStatus,
        PostgresRepositories, ProfileStateRecord, ProfileVisibilityV1, RepoMaintainerV1Record,
        RepoStateV1Record, RepoVisibilityV1, StorageConfig, StorageError,
    };
    use serde_json::json;
    use std::collections::{HashMap, HashSet};
    use std::future::Future;
    use std::sync::Mutex;
    use std::time::Duration;

    #[derive(Default)]
    struct MemoryStore {
        command_log: Mutex<HashSet<Vec<u8>>>,
        accounts: Mutex<HashMap<Vec<u8>, AccountStateRecord>>,
        profiles: Mutex<HashMap<Vec<u8>, ProfileStateRecord>>,
        repos: Mutex<HashMap<(Vec<u8>, String), RepoStateV1Record>>,
        maintainers: Mutex<HashMap<(Vec<u8>, String), HashSet<Vec<u8>>>>,
    }

    #[derive(Default)]
    struct ApplyFailStore;

    #[derive(Clone, Copy, PartialEq, Eq)]
    enum FaultPoint {
        InsertCommandLog,
        UpdateCommandLogOutcome,
        AccountState,
        UpsertAccountState,
        ProfileState,
        UpsertProfileState,
        RepoState,
        UpsertRepoState,
        SetRepoMaintainer,
        ListActiveRepoMaintainers,
    }

    struct FaultStore {
        fault: Option<FaultPoint>,
        account: Option<AccountStateRecord>,
        profile: Option<ProfileStateRecord>,
        repo: Option<RepoStateV1Record>,
        maintainers: HashSet<Vec<u8>>,
    }

    impl FaultStore {
        fn new(fault: Option<FaultPoint>) -> Self {
            Self {
                fault,
                account: None,
                profile: None,
                repo: None,
                maintainers: HashSet::new(),
            }
        }

        fn with_account(mut self, account: AccountStateRecord) -> Self {
            self.account = Some(account);
            self
        }

        fn with_repo(mut self, repo: RepoStateV1Record) -> Self {
            self.repo = Some(repo);
            self
        }

        fn with_maintainer(mut self, maintainer: Vec<u8>) -> Self {
            self.maintainers.insert(maintainer);
            self
        }

        fn maybe_fail(&self, point: FaultPoint) -> Result<(), DispatchError> {
            if self.fault == Some(point) {
                return Err(DispatchError::Storage(StorageError::Internal {
                    message: "fault store".to_string(),
                }));
            }
            Ok(())
        }
    }

    #[async_trait]
    impl CommandStore for ApplyFailStore {
        async fn insert_command_log(
            &self,
            _record: &CommandLogRecord,
        ) -> Result<bool, DispatchError> {
            Ok(true)
        }

        async fn update_command_log_outcome(
            &self,
            _event_id: &[u8],
            _status: CommandStatus,
            _code: &str,
            _message: &str,
        ) -> Result<(), DispatchError> {
            Ok(())
        }

        async fn account_state(
            &self,
            _pubkey: &[u8],
        ) -> Result<Option<AccountStateRecord>, DispatchError> {
            Err(DispatchError::Storage(StorageError::Internal {
                message: "store boom".to_string(),
            }))
        }

        async fn upsert_account_state(
            &self,
            _record: &AccountStateRecord,
        ) -> Result<(), DispatchError> {
            Ok(())
        }

        async fn profile_state(
            &self,
            _pubkey: &[u8],
        ) -> Result<Option<ProfileStateRecord>, DispatchError> {
            Ok(None)
        }

        async fn upsert_profile_state(
            &self,
            _record: &ProfileStateRecord,
        ) -> Result<(), DispatchError> {
            Ok(())
        }

        async fn repo_state(
            &self,
            _owner_pubkey: &[u8],
            _repo_name: &str,
        ) -> Result<Option<RepoStateV1Record>, DispatchError> {
            Ok(None)
        }

        async fn upsert_repo_state(
            &self,
            _record: &RepoStateV1Record,
        ) -> Result<(), DispatchError> {
            Ok(())
        }

        async fn set_repo_maintainer(
            &self,
            _record: &RepoMaintainerV1Record,
        ) -> Result<(), DispatchError> {
            Ok(())
        }

        async fn list_active_repo_maintainers(
            &self,
            _owner_pubkey: &[u8],
            _repo_name: &str,
        ) -> Result<HashSet<Vec<u8>>, DispatchError> {
            Ok(HashSet::new())
        }
    }

    #[async_trait]
    impl CommandStore for FaultStore {
        async fn insert_command_log(
            &self,
            _record: &CommandLogRecord,
        ) -> Result<bool, DispatchError> {
            self.maybe_fail(FaultPoint::InsertCommandLog)?;
            Ok(true)
        }

        async fn update_command_log_outcome(
            &self,
            _event_id: &[u8],
            _status: CommandStatus,
            _code: &str,
            _message: &str,
        ) -> Result<(), DispatchError> {
            self.maybe_fail(FaultPoint::UpdateCommandLogOutcome)
        }

        async fn account_state(
            &self,
            _pubkey: &[u8],
        ) -> Result<Option<AccountStateRecord>, DispatchError> {
            self.maybe_fail(FaultPoint::AccountState)?;
            Ok(self.account.clone())
        }

        async fn upsert_account_state(
            &self,
            _record: &AccountStateRecord,
        ) -> Result<(), DispatchError> {
            self.maybe_fail(FaultPoint::UpsertAccountState)
        }

        async fn profile_state(
            &self,
            _pubkey: &[u8],
        ) -> Result<Option<ProfileStateRecord>, DispatchError> {
            self.maybe_fail(FaultPoint::ProfileState)?;
            Ok(self.profile.clone())
        }

        async fn upsert_profile_state(
            &self,
            _record: &ProfileStateRecord,
        ) -> Result<(), DispatchError> {
            self.maybe_fail(FaultPoint::UpsertProfileState)
        }

        async fn repo_state(
            &self,
            _owner_pubkey: &[u8],
            _repo_name: &str,
        ) -> Result<Option<RepoStateV1Record>, DispatchError> {
            self.maybe_fail(FaultPoint::RepoState)?;
            Ok(self.repo.clone())
        }

        async fn upsert_repo_state(
            &self,
            _record: &RepoStateV1Record,
        ) -> Result<(), DispatchError> {
            self.maybe_fail(FaultPoint::UpsertRepoState)
        }

        async fn set_repo_maintainer(
            &self,
            _record: &RepoMaintainerV1Record,
        ) -> Result<(), DispatchError> {
            self.maybe_fail(FaultPoint::SetRepoMaintainer)
        }

        async fn list_active_repo_maintainers(
            &self,
            _owner_pubkey: &[u8],
            _repo_name: &str,
        ) -> Result<HashSet<Vec<u8>>, DispatchError> {
            self.maybe_fail(FaultPoint::ListActiveRepoMaintainers)?;
            Ok(self.maintainers.clone())
        }
    }

    #[async_trait]
    impl CommandStore for MemoryStore {
        async fn insert_command_log(&self, record: &CommandLogRecord) -> Result<bool, DispatchError> {
            let mut log = self.command_log.lock().expect("log");
            Ok(log.insert(record.event_id.clone()))
        }

        async fn update_command_log_outcome(
            &self,
            _event_id: &[u8],
            _status: CommandStatus,
            _code: &str,
            _message: &str,
        ) -> Result<(), DispatchError> {
            Ok(())
        }

        async fn account_state(&self, pubkey: &[u8]) -> Result<Option<AccountStateRecord>, DispatchError> {
            let map = self.accounts.lock().expect("accounts");
            Ok(map.get(pubkey).cloned())
        }

        async fn upsert_account_state(&self, record: &AccountStateRecord) -> Result<(), DispatchError> {
            self.accounts
                .lock()
                .expect("accounts")
                .insert(record.pubkey.clone(), record.clone());
            Ok(())
        }

        async fn profile_state(&self, pubkey: &[u8]) -> Result<Option<ProfileStateRecord>, DispatchError> {
            let map = self.profiles.lock().expect("profiles");
            Ok(map.get(pubkey).cloned())
        }

        async fn upsert_profile_state(&self, record: &ProfileStateRecord) -> Result<(), DispatchError> {
            self.profiles
                .lock()
                .expect("profiles")
                .insert(record.pubkey.clone(), record.clone());
            Ok(())
        }

        async fn repo_state(
            &self,
            owner_pubkey: &[u8],
            repo_name: &str,
        ) -> Result<Option<RepoStateV1Record>, DispatchError> {
            let map = self.repos.lock().expect("repos");
            Ok(map
                .get(&(owner_pubkey.to_vec(), repo_name.to_string()))
                .cloned())
        }

        async fn upsert_repo_state(&self, record: &RepoStateV1Record) -> Result<(), DispatchError> {
            self.repos.lock().expect("repos").insert(
                (record.owner_pubkey.clone(), record.repo_name.clone()),
                record.clone(),
            );
            Ok(())
        }

        async fn set_repo_maintainer(&self, record: &RepoMaintainerV1Record) -> Result<(), DispatchError> {
            let mut map = self.maintainers.lock().expect("maintainers");
            let key = (record.owner_pubkey.clone(), record.repo_name.clone());
            let entry = map.entry(key).or_default();
            if record.active {
                entry.insert(record.maintainer_pubkey.clone());
            } else {
                entry.remove(&record.maintainer_pubkey);
            }
            Ok(())
        }

        async fn list_active_repo_maintainers(
            &self,
            owner_pubkey: &[u8],
            repo_name: &str,
        ) -> Result<HashSet<Vec<u8>>, DispatchError> {
            let map = self.maintainers.lock().expect("maintainers");
            Ok(map
                .get(&(owner_pubkey.to_vec(), repo_name.to_string()))
                .cloned()
                .unwrap_or_default())
        }
    }

    fn command(namespace: CommandNamespace, action: &str) -> ParsedCommand {
        ParsedCommand {
            namespace,
            action: action.to_string(),
            target: None,
            args: Vec::new(),
        }
    }

    fn parse(input: &str) -> ParsedCommand {
        gittree_core::parse_cli_command(input).expect("parse")
    }

    async fn run_with_timeout<T>(
        duration: Duration,
        future: impl Future<Output = Result<T, DispatchError>>,
    ) -> Result<T, DispatchError> {
        match tokio::time::timeout(duration, future).await {
            Ok(result) => result,
            Err(_) => Err(DispatchError::Storage(StorageError::Internal {
                message: "timeout waiting for storage call".to_string(),
            })),
        }
    }

    async fn postgres_store_with_closed_pool() -> PostgresRepositories {
        let storage = StorageConfig {
            read_connection: "postgres://gittree:gittree@127.0.0.1:5432/gittree".to_string(),
            write_connection: None,
            max_connections: 1,
            min_connections: 1,
            idle_timeout_secs: None,
            max_lifetime_secs: None,
            application_name: Some("dispatch-handlers-test".to_string()),
        };
        let pool_options = storage.pool_options().expect("pool options");
        let connect_options = storage.read_connect_options().expect("connect options");
        let pool = pool_options.connect_lazy_with(connect_options);
        pool.close().await;
        PostgresRepositories::new(pool)
    }

    #[tokio::test]
    async fn execute_account_create_and_status() {
        let store = MemoryStore::default();
        let actor = vec![7u8; 32];

        let create = CommandExecutionInput {
            event_id: vec![1u8; 32],
            actor_pubkey: actor.clone(),
            parsed: command(CommandNamespace::Account, "create"),
            created_at: 100,
        };
        let created = execute_command(&store, create).await.expect("create");
        assert_eq!(created.status, CommandStatus::Ok);

        let create_again = CommandExecutionInput {
            event_id: vec![8u8; 32],
            actor_pubkey: actor.clone(),
            parsed: command(CommandNamespace::Account, "create"),
            created_at: 100,
        };
        let exists = execute_command(&store, create_again)
            .await
            .expect("create again");
        assert_eq!(exists.code, "account_exists");

        let status = CommandExecutionInput {
            event_id: vec![2u8; 32],
            actor_pubkey: actor.clone(),
            parsed: command(CommandNamespace::Account, "status"),
            created_at: 101,
        };
        let output = execute_command(&store, status).await.expect("status");
        assert_eq!(output.code, "account_status");

        let saved = store
            .account_state(&actor)
            .await
            .expect("state")
            .expect("record");
        assert_eq!(saved.status, AccountLifecycle::Active);
    }

    #[tokio::test]
    async fn execute_profile_set_and_visibility() {
        let store = MemoryStore::default();
        let actor = vec![8u8; 32];

        let set = CommandExecutionInput {
            event_id: vec![3u8; 32],
            actor_pubkey: actor.clone(),
            parsed: ParsedCommand {
                namespace: CommandNamespace::Profile,
                action: "set".to_string(),
                target: None,
                args: vec![CommandArg::KeyValue {
                    key: "name".to_string(),
                    value: "alice".to_string(),
                }],
            },
            created_at: 100,
        };
        let output = execute_command(&store, set).await.expect("set");
        assert_eq!(output.code, "profile_updated");

        let visibility = CommandExecutionInput {
            event_id: vec![4u8; 32],
            actor_pubkey: actor.clone(),
            parsed: ParsedCommand {
                namespace: CommandNamespace::Profile,
                action: "visibility".to_string(),
                target: None,
                args: vec![CommandArg::Positional("public".to_string())],
            },
            created_at: 101,
        };
        let output = execute_command(&store, visibility).await.expect("visibility");
        assert_eq!(output.code, "profile_visibility_updated");

        let saved = store
            .profile_state(&actor)
            .await
            .expect("profile")
            .expect("record");
        assert_eq!(saved.display_name.as_deref(), Some("alice"));
        assert_eq!(saved.visibility, ProfileVisibilityV1::Public);
    }

    #[tokio::test]
    async fn execute_repo_create_update_archive() {
        let store = MemoryStore::default();
        let actor = vec![9u8; 32];

        let create = CommandExecutionInput {
            event_id: vec![5u8; 32],
            actor_pubkey: actor.clone(),
            parsed: ParsedCommand {
                namespace: CommandNamespace::Repo,
                action: "create".to_string(),
                target: Some("demo".to_string()),
                args: Vec::new(),
            },
            created_at: 100,
        };
        assert_eq!(execute_command(&store, create).await.expect("create").code, "repo_created");

        let update = CommandExecutionInput {
            event_id: vec![6u8; 32],
            actor_pubkey: actor.clone(),
            parsed: ParsedCommand {
                namespace: CommandNamespace::Repo,
                action: "update".to_string(),
                target: Some("demo".to_string()),
                args: vec![CommandArg::KeyValue {
                    key: "description".to_string(),
                    value: "hello".to_string(),
                }],
            },
            created_at: 101,
        };
        assert_eq!(execute_command(&store, update).await.expect("update").code, "repo_updated");

        let archive = CommandExecutionInput {
            event_id: vec![7u8; 32],
            actor_pubkey: actor.clone(),
            parsed: ParsedCommand {
                namespace: CommandNamespace::Repo,
                action: "archive".to_string(),
                target: Some("demo".to_string()),
                args: Vec::new(),
            },
            created_at: 102,
        };
        assert_eq!(
            execute_command(&store, archive).await.expect("archive").code,
            "repo_state_updated"
        );

        let state = store
            .repo_state(&actor, "demo")
            .await
            .expect("repo")
            .expect("state");
        assert_eq!(state.visibility, RepoVisibilityV1::Private);
        assert!(state.archived);
    }

    #[tokio::test]
    async fn execute_parsed_command_sequence_updates_projection_records() {
        let store = MemoryStore::default();
        let actor = vec![10u8; 32];
        let maintainer = vec![12u8; 32];
        let maintainer_npub = npub_from_bytes(&maintainer).expect("npub");

        let inputs = vec![
            ("gittree account create".to_string(), vec![10u8; 32], 100),
            (
                "gittree profile set name=alice website=https://gittr.ee".to_string(),
                vec![11u8; 32],
                101,
            ),
            (
                "gittree profile visibility public".to_string(),
                vec![12u8; 32],
                102,
            ),
            ("gittree repo create demo".to_string(), vec![13u8; 32], 103),
            (
                format!("gittree repo maintainers demo add {maintainer_npub}"),
                vec![14u8; 32],
                104,
            ),
        ];

        for (content, event_id, created_at) in inputs {
            let output = execute_command(
                &store,
                CommandExecutionInput {
                    event_id,
                    actor_pubkey: actor.clone(),
                    parsed: parse(&content),
                    created_at,
                },
            )
            .await
            .expect("execute");
            assert_eq!(output.status, CommandStatus::Ok);
        }

        let account = store
            .account_state(&actor)
            .await
            .expect("account lookup")
            .expect("account");
        assert_eq!(account.status, AccountLifecycle::Active);

        let profile = store
            .profile_state(&actor)
            .await
            .expect("profile lookup")
            .expect("profile");
        assert_eq!(profile.display_name.as_deref(), Some("alice"));
        assert_eq!(profile.website_url.as_deref(), Some("https://gittr.ee"));
        assert_eq!(profile.visibility, ProfileVisibilityV1::Public);

        let repo = store
            .repo_state(&actor, "demo")
            .await
            .expect("repo lookup")
            .expect("repo");
        assert_eq!(repo.repo_name, "demo");
        assert_eq!(repo.visibility, RepoVisibilityV1::Private);
        assert!(!repo.archived);

        let maintainers = store
            .list_active_repo_maintainers(&actor, "demo")
            .await
            .expect("maintainers");
        assert!(maintainers.contains(&actor));
        assert!(maintainers.contains(&maintainer));
    }

    #[tokio::test]
    async fn execute_command_dedupes_event_id() {
        let store = MemoryStore::default();
        let actor = vec![1u8; 32];
        let input = CommandExecutionInput {
            event_id: vec![9u8; 32],
            actor_pubkey: actor,
            parsed: command(CommandNamespace::Account, "status"),
            created_at: 1,
        };

        let first = execute_command(&store, input.clone()).await.expect("first");
        let second = execute_command(&store, input).await.expect("second");
        assert_eq!(first.code, "account_status");
        assert_eq!(second.code, "duplicate");
    }

    #[tokio::test]
    async fn execute_account_delete_and_invalid_action_paths() {
        let store = MemoryStore::default();
        let actor = vec![15u8; 32];

        let missing_status = execute_command(
            &store,
            CommandExecutionInput {
                event_id: vec![40u8; 32],
                actor_pubkey: actor.clone(),
                parsed: parse("gittree account status"),
                created_at: 100,
            },
        )
        .await
        .expect("status");
        assert_eq!(missing_status.code, "account_status");
        assert_eq!(missing_status.message, "account missing");

        let deleted = execute_command(
            &store,
            CommandExecutionInput {
                event_id: vec![41u8; 32],
                actor_pubkey: actor.clone(),
                parsed: parse("gittree account delete"),
                created_at: 101,
            },
        )
        .await
        .expect("delete");
        assert_eq!(deleted.code, "account_deleted");

        let state = store
            .account_state(&actor)
            .await
            .expect("lookup")
            .expect("state");
        assert_eq!(state.status, AccountLifecycle::Deleted);

        let invalid = execute_command(
            &store,
            CommandExecutionInput {
                event_id: vec![42u8; 32],
                actor_pubkey: actor,
                parsed: command(CommandNamespace::Account, "unknown"),
                created_at: 102,
            },
        )
        .await
        .expect("invalid");
        assert_eq!(invalid.status, CommandStatus::Error);
        assert_eq!(invalid.code, "invalid_command");
    }

    #[tokio::test]
    async fn execute_profile_invalid_and_full_set_paths() {
        let store = MemoryStore::default();
        let actor = vec![16u8; 32];

        let invalid_visibility = execute_command(
            &store,
            CommandExecutionInput {
                event_id: vec![50u8; 32],
                actor_pubkey: actor.clone(),
                parsed: ParsedCommand {
                    namespace: CommandNamespace::Profile,
                    action: "visibility".to_string(),
                    target: None,
                    args: Vec::new(),
                },
                created_at: 200,
            },
        )
        .await
        .expect("invalid visibility");
        assert_eq!(invalid_visibility.status, CommandStatus::Error);
        assert_eq!(invalid_visibility.code, "invalid_args");

        let set = execute_command(
            &store,
            CommandExecutionInput {
                event_id: vec![51u8; 32],
                actor_pubkey: actor.clone(),
                parsed: parse(
                    "gittree profile set name=alice bio=hi avatar=https://a website=https://w location=earth",
                ),
                created_at: 201,
            },
        )
        .await
        .expect("set");
        assert_eq!(set.code, "profile_updated");

        let private_visibility = execute_command(
            &store,
            CommandExecutionInput {
                event_id: vec![52u8; 32],
                actor_pubkey: actor.clone(),
                parsed: parse("gittree profile visibility private"),
                created_at: 202,
            },
        )
        .await
        .expect("visibility");
        assert_eq!(private_visibility.code, "profile_visibility_updated");

        let profile = store
            .profile_state(&actor)
            .await
            .expect("lookup")
            .expect("profile");
        assert_eq!(profile.bio.as_deref(), Some("hi"));
        assert_eq!(profile.avatar_url.as_deref(), Some("https://a"));
        assert_eq!(profile.website_url.as_deref(), Some("https://w"));
        assert_eq!(profile.location.as_deref(), Some("earth"));
        assert_eq!(profile.visibility, ProfileVisibilityV1::Private);

        let invalid = execute_command(
            &store,
            CommandExecutionInput {
                event_id: vec![53u8; 32],
                actor_pubkey: actor,
                parsed: command(CommandNamespace::Profile, "unknown"),
                created_at: 203,
            },
        )
        .await
        .expect("invalid");
        assert_eq!(invalid.status, CommandStatus::Error);
        assert_eq!(invalid.code, "invalid_command");
    }

    #[tokio::test]
    async fn execute_profile_set_ignores_unknown_and_positional_args() {
        let store = MemoryStore::default();
        let actor = vec![19u8; 32];
        let output = execute_command(
            &store,
            CommandExecutionInput {
                event_id: vec![54u8; 32],
                actor_pubkey: actor.clone(),
                parsed: ParsedCommand {
                    namespace: CommandNamespace::Profile,
                    action: "set".to_string(),
                    target: None,
                    args: vec![
                        CommandArg::Positional("ignored".to_string()),
                        CommandArg::KeyValue {
                            key: "unsupported".to_string(),
                            value: "noop".to_string(),
                        },
                        CommandArg::KeyValue {
                            key: "name".to_string(),
                            value: "alice".to_string(),
                        },
                    ],
                },
                created_at: 210,
            },
        )
        .await
        .expect("set");
        assert_eq!(output.code, "profile_updated");

        let profile = store
            .profile_state(&actor)
            .await
            .expect("lookup")
            .expect("profile");
        assert_eq!(profile.display_name.as_deref(), Some("alice"));
    }

    #[tokio::test]
    async fn execute_repo_branch_paths_cover_invalid_and_misc_actions() {
        let store = MemoryStore::default();
        let actor = vec![17u8; 32];
        let actor_npub = npub_from_bytes(&actor).expect("npub");

        let missing_target = execute_command(
            &store,
            CommandExecutionInput {
                event_id: vec![60u8; 32],
                actor_pubkey: actor.clone(),
                parsed: ParsedCommand {
                    namespace: CommandNamespace::Repo,
                    action: "create".to_string(),
                    target: None,
                    args: Vec::new(),
                },
                created_at: 300,
            },
        )
        .await
        .expect("missing target");
        assert_eq!(missing_target.status, CommandStatus::Error);
        assert_eq!(missing_target.code, "invalid_args");

        let created = execute_command(
            &store,
            CommandExecutionInput {
                event_id: vec![61u8; 32],
                actor_pubkey: actor.clone(),
                parsed: parse("gittree repo create demo"),
                created_at: 301,
            },
        )
        .await
        .expect("create");
        assert_eq!(created.code, "repo_created");

        let duplicate = execute_command(
            &store,
            CommandExecutionInput {
                event_id: vec![62u8; 32],
                actor_pubkey: actor.clone(),
                parsed: parse("gittree repo create demo"),
                created_at: 302,
            },
        )
        .await
        .expect("duplicate");
        assert_eq!(duplicate.code, "repo_exists");

        let not_found = execute_command(
            &store,
            CommandExecutionInput {
                event_id: vec![63u8; 32],
                actor_pubkey: actor.clone(),
                parsed: parse("gittree repo update missing description=hello"),
                created_at: 303,
            },
        )
        .await
        .expect("missing");
        assert_eq!(not_found.status, CommandStatus::Error);
        assert_eq!(not_found.code, "not_found");

        let invalid_maintainer = execute_command(
            &store,
            CommandExecutionInput {
                event_id: vec![64u8; 32],
                actor_pubkey: actor.clone(),
                parsed: ParsedCommand {
                    namespace: CommandNamespace::Repo,
                    action: "maintainers".to_string(),
                    target: Some("demo".to_string()),
                    args: vec![
                        CommandArg::Positional("add".to_string()),
                        CommandArg::Positional("not-an-npub".to_string()),
                    ],
                },
                created_at: 304,
            },
        )
        .await
        .expect("invalid maintainer");
        assert_eq!(invalid_maintainer.status, CommandStatus::Error);
        assert_eq!(invalid_maintainer.code, "invalid_args");

        let malformed_maintainer = execute_command(
            &store,
            CommandExecutionInput {
                event_id: vec![65u8; 32],
                actor_pubkey: actor.clone(),
                parsed: ParsedCommand {
                    namespace: CommandNamespace::Repo,
                    action: "maintainers".to_string(),
                    target: Some("demo".to_string()),
                    args: vec![CommandArg::Positional("add".to_string())],
                },
                created_at: 305,
            },
        )
        .await
        .expect("malformed maintainer");
        assert_eq!(malformed_maintainer.status, CommandStatus::Error);
        assert_eq!(malformed_maintainer.code, "invalid_args");

        let announce = execute_command(
            &store,
            CommandExecutionInput {
                event_id: vec![66u8; 32],
                actor_pubkey: actor.clone(),
                parsed: parse("gittree repo announce demo"),
                created_at: 306,
            },
        )
        .await
        .expect("announce");
        assert_eq!(announce.code, "repo_announce_accepted");

        let sync = execute_command(
            &store,
            CommandExecutionInput {
                event_id: vec![67u8; 32],
                actor_pubkey: actor.clone(),
                parsed: parse("gittree repo sync demo"),
                created_at: 307,
            },
        )
        .await
        .expect("sync");
        assert_eq!(sync.code, "repo_sync_accepted");

        let unarchive = execute_command(
            &store,
            CommandExecutionInput {
                event_id: vec![68u8; 32],
                actor_pubkey: actor.clone(),
                parsed: parse("gittree repo unarchive demo"),
                created_at: 308,
            },
        )
        .await
        .expect("unarchive");
        assert_eq!(unarchive.code, "repo_state_updated");

        let remove_actor = execute_command(
            &store,
            CommandExecutionInput {
                event_id: vec![69u8; 32],
                actor_pubkey: actor.clone(),
                parsed: parse(&format!("gittree repo maintainers demo remove {actor_npub}")),
                created_at: 309,
            },
        )
        .await
        .expect("remove actor");
        assert_eq!(remove_actor.code, "repo_maintainer_updated");

        let unauthorized_update = execute_command(
            &store,
            CommandExecutionInput {
                event_id: vec![70u8; 32],
                actor_pubkey: actor.clone(),
                parsed: parse("gittree repo update demo description=world"),
                created_at: 310,
            },
        )
        .await
        .expect("unauthorized");
        assert_eq!(unauthorized_update.status, CommandStatus::Error);
        assert_eq!(unauthorized_update.code, "unauthorized");

        let unauthorized_maintainer_update = execute_command(
            &store,
            CommandExecutionInput {
                event_id: vec![71u8; 32],
                actor_pubkey: actor.clone(),
                parsed: parse(&format!("gittree repo maintainers demo add {actor_npub}")),
                created_at: 311,
            },
        )
        .await
        .expect("unauthorized maintainers");
        assert_eq!(unauthorized_maintainer_update.status, CommandStatus::Error);
        assert_eq!(unauthorized_maintainer_update.code, "unauthorized");

        let archive_missing = execute_command(
            &store,
            CommandExecutionInput {
                event_id: vec![72u8; 32],
                actor_pubkey: actor.clone(),
                parsed: parse("gittree repo archive missing"),
                created_at: 312,
            },
        )
        .await
        .expect("archive missing");
        assert_eq!(archive_missing.status, CommandStatus::Error);
        assert_eq!(archive_missing.code, "not_found");

        let archive_unauthorized = execute_command(
            &store,
            CommandExecutionInput {
                event_id: vec![73u8; 32],
                actor_pubkey: actor.clone(),
                parsed: parse("gittree repo archive demo"),
                created_at: 313,
            },
        )
        .await
        .expect("archive unauthorized");
        assert_eq!(archive_unauthorized.status, CommandStatus::Error);
        assert_eq!(archive_unauthorized.code, "unauthorized");

        let invalid = execute_command(
            &store,
            CommandExecutionInput {
                event_id: vec![74u8; 32],
                actor_pubkey: actor,
                parsed: ParsedCommand {
                    namespace: CommandNamespace::Repo,
                    action: "unknown".to_string(),
                    target: Some("demo".to_string()),
                    args: Vec::new(),
                },
                created_at: 311,
            },
        )
        .await
        .expect("invalid");
        assert_eq!(invalid.status, CommandStatus::Error);
        assert_eq!(invalid.code, "invalid_command");
    }

    #[tokio::test]
    async fn execute_repo_update_covers_visibility_website_and_default_branch_variants() {
        let store = MemoryStore::default();
        let actor = vec![20u8; 32];
        assert_eq!(
            execute_command(
                &store,
                CommandExecutionInput {
                    event_id: vec![90u8; 32],
                    actor_pubkey: actor.clone(),
                    parsed: parse("gittree repo create demo"),
                    created_at: 400,
                },
            )
            .await
            .expect("create")
            .code,
            "repo_created"
        );

        assert_eq!(
            execute_command(
                &store,
                CommandExecutionInput {
                    event_id: vec![91u8; 32],
                    actor_pubkey: actor.clone(),
                    parsed: ParsedCommand {
                        namespace: CommandNamespace::Repo,
                        action: "update".to_string(),
                        target: Some("demo".to_string()),
                        args: vec![
                            CommandArg::Positional("ignored".to_string()),
                            CommandArg::KeyValue {
                                key: "website".to_string(),
                                value: "https://gittr.ee/demo".to_string(),
                            },
                            CommandArg::KeyValue {
                                key: "visibility".to_string(),
                                value: "public".to_string(),
                            },
                            CommandArg::KeyValue {
                                key: "default_branch".to_string(),
                                value: "dev".to_string(),
                            },
                            CommandArg::KeyValue {
                                key: "unsupported".to_string(),
                                value: "noop".to_string(),
                            },
                        ],
                    },
                    created_at: 401,
                },
            )
            .await
            .expect("update")
            .code,
            "repo_updated"
        );

        assert_eq!(
            execute_command(
                &store,
                CommandExecutionInput {
                    event_id: vec![92u8; 32],
                    actor_pubkey: actor.clone(),
                    parsed: parse("gittree repo update demo visibility=private"),
                    created_at: 402,
                },
            )
            .await
            .expect("update")
            .code,
            "repo_updated"
        );

        let state = store
            .repo_state(&actor, "demo")
            .await
            .expect("repo")
            .expect("state");
        assert_eq!(state.website_url.as_deref(), Some("https://gittr.ee/demo"));
        assert_eq!(state.default_branch, "dev");
        assert_eq!(state.visibility, RepoVisibilityV1::Private);
    }

    #[tokio::test]
    async fn execute_command_maps_apply_errors_to_internal_outcome() {
        let store = ApplyFailStore;
        let actor = vec![18u8; 32];
        let output = execute_command(
            &store,
            CommandExecutionInput {
                event_id: vec![80u8; 32],
                actor_pubkey: actor,
                parsed: parse("gittree account status"),
                created_at: 400,
            },
        )
        .await
        .expect("output");
        assert_eq!(output.status, CommandStatus::Error);
        assert_eq!(output.code, "internal");
        assert!(output.message.contains("dispatch storage error"));
    }

    #[tokio::test]
    async fn helper_builders_and_store_trait_methods_are_exercised() {
        let memory = MemoryStore::default();
        let actor = vec![21u8; 32];
        let repo_name = "demo".to_string();

        let log_record = CommandLogRecord {
            event_id: vec![1u8; 32],
            pubkey: actor.clone(),
            namespace: "account".to_string(),
            action: "status".to_string(),
            target: None,
            args_json: json!({}),
            status: CommandStatus::Ok,
            code: "ok".to_string(),
            message: "ok".to_string(),
            created_at: 1,
        };
        assert!(
            memory
                .insert_command_log(&log_record)
                .await
                .expect("insert first")
        );
        assert!(
            !memory
                .insert_command_log(&log_record)
                .await
                .expect("insert duplicate")
        );
        memory
            .update_command_log_outcome(&log_record.event_id, CommandStatus::Ok, "ok", "ok")
            .await
            .expect("update");

        let account = AccountStateRecord {
            pubkey: actor.clone(),
            status: AccountLifecycle::Active,
            created_at: 1,
            updated_at: 2,
            deleted_at: None,
        };
        assert!(memory.account_state(&actor).await.expect("account lookup").is_none());
        memory
            .upsert_account_state(&account)
            .await
            .expect("upsert account");
        assert!(memory.account_state(&actor).await.expect("account lookup").is_some());

        let profile = ProfileStateRecord {
            pubkey: actor.clone(),
            display_name: Some("alice".to_string()),
            bio: None,
            avatar_url: None,
            website_url: None,
            location: None,
            visibility: ProfileVisibilityV1::Private,
            updated_at: 3,
        };
        assert!(memory.profile_state(&actor).await.expect("profile lookup").is_none());
        memory
            .upsert_profile_state(&profile)
            .await
            .expect("upsert profile");
        assert!(memory.profile_state(&actor).await.expect("profile lookup").is_some());

        let repo = RepoStateV1Record {
            owner_pubkey: actor.clone(),
            repo_name: repo_name.clone(),
            description: None,
            website_url: None,
            visibility: RepoVisibilityV1::Private,
            default_branch: "main".to_string(),
            archived: false,
            updated_at: 4,
        };
        assert!(
            memory
                .repo_state(&actor, &repo_name)
                .await
                .expect("repo lookup")
                .is_none()
        );
        memory.upsert_repo_state(&repo).await.expect("upsert repo");
        assert!(
            memory
                .repo_state(&actor, &repo_name)
                .await
                .expect("repo lookup")
                .is_some()
        );

        let maintainer_record = RepoMaintainerV1Record {
            owner_pubkey: actor.clone(),
            repo_name: repo_name.clone(),
            maintainer_pubkey: actor.clone(),
            active: true,
            updated_at: 5,
        };
        memory
            .set_repo_maintainer(&maintainer_record)
            .await
            .expect("set maintainer");
        let active = memory
            .list_active_repo_maintainers(&actor, &repo_name)
            .await
            .expect("list maintainers");
        assert!(active.contains(&actor));

        let remove_record = RepoMaintainerV1Record {
            active: false,
            ..maintainer_record
        };
        memory
            .set_repo_maintainer(&remove_record)
            .await
            .expect("unset maintainer");
        let active = memory
            .list_active_repo_maintainers(&actor, &repo_name)
            .await
            .expect("list maintainers");
        assert!(!active.contains(&actor));

        let apply_fail = ApplyFailStore;
        assert!(
            apply_fail
                .insert_command_log(&log_record)
                .await
                .expect("insert")
        );
        apply_fail
            .update_command_log_outcome(&log_record.event_id, CommandStatus::Error, "err", "err")
            .await
            .expect("update");
        assert!(apply_fail.upsert_account_state(&account).await.is_ok());
        assert!(apply_fail.profile_state(&actor).await.expect("profile").is_none());
        assert!(apply_fail.upsert_profile_state(&profile).await.is_ok());
        assert!(
            apply_fail
                .repo_state(&actor, &repo_name)
                .await
                .expect("repo")
                .is_none()
        );
        assert!(apply_fail.upsert_repo_state(&repo).await.is_ok());
        assert!(apply_fail.set_repo_maintainer(&remove_record).await.is_ok());
        assert!(
            apply_fail
                .list_active_repo_maintainers(&actor, &repo_name)
                .await
                .expect("maintainers")
                .is_empty()
        );
        assert!(apply_fail.account_state(&actor).await.is_err());

        assert_eq!(namespace_name(CommandNamespace::Account), "account");
        assert_eq!(namespace_name(CommandNamespace::Profile), "profile");
        assert_eq!(namespace_name(CommandNamespace::Repo), "repo");

        let args = vec![
            CommandArg::Positional("public".to_string()),
            CommandArg::KeyValue {
                key: "description".to_string(),
                value: "hello".to_string(),
            },
        ];
        assert_eq!(
            args_to_json(&args),
            json!({"description": "hello", "_positional": ["public"]})
        );

        let ok_output = ok("ok_code", "ok message");
        assert_eq!(ok_output.status, CommandStatus::Ok);
        assert_eq!(ok_output.code, "ok_code");
        let err_output = err("err_code", "err message");
        assert_eq!(err_output.status, CommandStatus::Error);
        assert_eq!(err_output.code, "err_code");
    }

    #[tokio::test]
    async fn postgres_command_store_impl_maps_storage_errors() {
        let store = postgres_store_with_closed_pool().await;
        let actor = vec![31u8; 32];
        let repo_name = "demo".to_string();
        let log_record = CommandLogRecord {
            event_id: vec![1u8; 32],
            pubkey: actor.clone(),
            namespace: "repo".to_string(),
            action: "create".to_string(),
            target: Some(repo_name.clone()),
            args_json: json!({}),
            status: CommandStatus::Ok,
            code: "ok".to_string(),
            message: "ok".to_string(),
            created_at: 1,
        };
        let account = AccountStateRecord {
            pubkey: actor.clone(),
            status: AccountLifecycle::Active,
            created_at: 1,
            updated_at: 2,
            deleted_at: None,
        };
        let profile = ProfileStateRecord {
            pubkey: actor.clone(),
            display_name: Some("alice".to_string()),
            bio: None,
            avatar_url: None,
            website_url: None,
            location: None,
            visibility: ProfileVisibilityV1::Private,
            updated_at: 2,
        };
        let repo = RepoStateV1Record {
            owner_pubkey: actor.clone(),
            repo_name: repo_name.clone(),
            description: None,
            website_url: None,
            visibility: RepoVisibilityV1::Private,
            default_branch: "main".to_string(),
            archived: false,
            updated_at: 2,
        };
        let maintainer = RepoMaintainerV1Record {
            owner_pubkey: actor.clone(),
            repo_name: repo_name.clone(),
            maintainer_pubkey: actor.clone(),
            active: true,
            updated_at: 2,
        };

        let insert = run_with_timeout(
            Duration::from_secs(3),
            CommandStore::insert_command_log(&store, &log_record),
        )
        .await;
        assert!(matches!(insert, Err(DispatchError::Storage(_))));

        let update = run_with_timeout(
            Duration::from_secs(3),
            CommandStore::update_command_log_outcome(
                &store,
                &log_record.event_id,
                CommandStatus::Ok,
                "ok",
                "ok",
            ),
        )
        .await;
        assert!(matches!(update, Err(DispatchError::Storage(_))));

        let account_state = run_with_timeout(
            Duration::from_secs(3),
            CommandStore::account_state(&store, &actor),
        )
        .await;
        assert!(matches!(account_state, Err(DispatchError::Storage(_))));

        let upsert_account = run_with_timeout(
            Duration::from_secs(3),
            CommandStore::upsert_account_state(&store, &account),
        )
        .await;
        assert!(matches!(upsert_account, Err(DispatchError::Storage(_))));

        let profile_state = run_with_timeout(
            Duration::from_secs(3),
            CommandStore::profile_state(&store, &actor),
        )
        .await;
        assert!(matches!(profile_state, Err(DispatchError::Storage(_))));

        let upsert_profile = run_with_timeout(
            Duration::from_secs(3),
            CommandStore::upsert_profile_state(&store, &profile),
        )
        .await;
        assert!(matches!(upsert_profile, Err(DispatchError::Storage(_))));

        let repo_state = run_with_timeout(
            Duration::from_secs(3),
            CommandStore::repo_state(&store, &actor, &repo_name),
        )
        .await;
        assert!(matches!(repo_state, Err(DispatchError::Storage(_))));

        let upsert_repo = run_with_timeout(
            Duration::from_secs(3),
            CommandStore::upsert_repo_state(&store, &repo),
        )
        .await;
        assert!(matches!(upsert_repo, Err(DispatchError::Storage(_))));

        let set_maintainer = run_with_timeout(
            Duration::from_secs(3),
            CommandStore::set_repo_maintainer(&store, &maintainer),
        )
        .await;
        assert!(matches!(set_maintainer, Err(DispatchError::Storage(_))));

        let list_maintainers = run_with_timeout(
            Duration::from_secs(3),
            CommandStore::list_active_repo_maintainers(&store, &actor, &repo_name),
        )
        .await;
        assert!(matches!(list_maintainers, Err(DispatchError::Storage(_))));
    }

    #[tokio::test]
    async fn run_with_timeout_maps_elapsed_to_storage_error() {
        let result = run_with_timeout(
            Duration::from_millis(1),
            std::future::pending::<Result<(), DispatchError>>(),
        )
        .await;

        assert!(matches!(
            result,
            Err(DispatchError::Storage(StorageError::Internal { message }))
                if message == "timeout waiting for storage call"
        ));
    }

    #[tokio::test]
    async fn run_with_timeout_returns_inner_result() {
        let result = run_with_timeout(Duration::from_secs(1), async {
            Ok::<&'static str, DispatchError>("ok")
        })
        .await
        .expect("ok result");
        assert_eq!(result, "ok");
    }

    #[tokio::test]
    async fn execute_command_propagates_update_command_log_outcome_error() {
        let store = FaultStore::new(Some(FaultPoint::UpdateCommandLogOutcome));
        let actor = vec![44u8; 32];
        let input = CommandExecutionInput {
            event_id: vec![88u8; 32],
            actor_pubkey: actor,
            parsed: parse("gittree account status"),
            created_at: 500,
        };

        let output = execute_command(&store, input).await;
        assert!(matches!(output, Err(DispatchError::Storage(_))));
    }

    #[tokio::test]
    async fn apply_account_and_profile_propagate_store_errors() {
        let actor = vec![45u8; 32];

        let account_create = CommandExecutionInput {
            event_id: vec![90u8; 32],
            actor_pubkey: actor.clone(),
            parsed: parse("gittree account create"),
            created_at: 600,
        };
        let create_err = super::apply_account(
            &FaultStore::new(Some(FaultPoint::UpsertAccountState)),
            &account_create,
        )
        .await;
        assert!(matches!(create_err, Err(DispatchError::Storage(_))));

        let account_delete = CommandExecutionInput {
            event_id: vec![91u8; 32],
            actor_pubkey: actor.clone(),
            parsed: parse("gittree account delete"),
            created_at: 601,
        };
        let delete_err = super::apply_account(
            &FaultStore::new(Some(FaultPoint::UpsertAccountState)).with_account(
                AccountStateRecord {
                    pubkey: actor.clone(),
                    status: AccountLifecycle::Active,
                    created_at: 500,
                    updated_at: 500,
                    deleted_at: None,
                },
            ),
            &account_delete,
        )
        .await;
        assert!(matches!(delete_err, Err(DispatchError::Storage(_))));

        let profile_set = CommandExecutionInput {
            event_id: vec![92u8; 32],
            actor_pubkey: actor.clone(),
            parsed: parse("gittree profile set name=alice"),
            created_at: 602,
        };
        let profile_state_err =
            super::apply_profile(&FaultStore::new(Some(FaultPoint::ProfileState)), &profile_set).await;
        assert!(matches!(profile_state_err, Err(DispatchError::Storage(_))));

        let profile_set_err = super::apply_profile(
            &FaultStore::new(Some(FaultPoint::UpsertProfileState)),
            &profile_set,
        )
        .await;
        assert!(matches!(profile_set_err, Err(DispatchError::Storage(_))));

        let profile_visibility = CommandExecutionInput {
            event_id: vec![93u8; 32],
            actor_pubkey: actor,
            parsed: parse("gittree profile visibility public"),
            created_at: 603,
        };
        let profile_visibility_err = super::apply_profile(
            &FaultStore::new(Some(FaultPoint::UpsertProfileState)),
            &profile_visibility,
        )
        .await;
        assert!(matches!(profile_visibility_err, Err(DispatchError::Storage(_))));
    }

    #[tokio::test]
    async fn apply_repo_propagates_store_errors_for_each_mutation_step() {
        let actor = vec![46u8; 32];
        let actor_npub = npub_from_bytes(&actor).expect("npub");
        let repo_record = RepoStateV1Record {
            owner_pubkey: actor.clone(),
            repo_name: "demo".to_string(),
            description: None,
            website_url: None,
            visibility: RepoVisibilityV1::Private,
            default_branch: "main".to_string(),
            archived: false,
            updated_at: 700,
        };

        let repo_create = CommandExecutionInput {
            event_id: vec![94u8; 32],
            actor_pubkey: actor.clone(),
            parsed: parse("gittree repo create demo"),
            created_at: 701,
        };
        assert!(matches!(
            super::apply_repo(&FaultStore::new(Some(FaultPoint::RepoState)), &repo_create).await,
            Err(DispatchError::Storage(_))
        ));
        assert!(matches!(
            super::apply_repo(&FaultStore::new(Some(FaultPoint::UpsertRepoState)), &repo_create).await,
            Err(DispatchError::Storage(_))
        ));
        assert!(matches!(
            super::apply_repo(&FaultStore::new(Some(FaultPoint::SetRepoMaintainer)), &repo_create).await,
            Err(DispatchError::Storage(_))
        ));

        let repo_update = CommandExecutionInput {
            event_id: vec![95u8; 32],
            actor_pubkey: actor.clone(),
            parsed: parse("gittree repo update demo description=hello"),
            created_at: 702,
        };
        assert!(matches!(
            super::apply_repo(&FaultStore::new(Some(FaultPoint::RepoState)), &repo_update).await,
            Err(DispatchError::Storage(_))
        ));
        assert!(matches!(
            super::apply_repo(
                &FaultStore::new(Some(FaultPoint::ListActiveRepoMaintainers))
                    .with_repo(repo_record.clone()),
                &repo_update
            )
            .await,
            Err(DispatchError::Storage(_))
        ));
        assert!(matches!(
            super::apply_repo(
                &FaultStore::new(Some(FaultPoint::UpsertRepoState))
                    .with_repo(repo_record.clone())
                    .with_maintainer(actor.clone()),
                &repo_update
            )
            .await,
            Err(DispatchError::Storage(_))
        ));

        let repo_archive = CommandExecutionInput {
            event_id: vec![96u8; 32],
            actor_pubkey: actor.clone(),
            parsed: parse("gittree repo archive demo"),
            created_at: 703,
        };
        assert!(matches!(
            super::apply_repo(&FaultStore::new(Some(FaultPoint::RepoState)), &repo_archive).await,
            Err(DispatchError::Storage(_))
        ));
        assert!(matches!(
            super::apply_repo(
                &FaultStore::new(Some(FaultPoint::ListActiveRepoMaintainers))
                    .with_repo(repo_record.clone()),
                &repo_archive
            )
            .await,
            Err(DispatchError::Storage(_))
        ));
        assert!(matches!(
            super::apply_repo(
                &FaultStore::new(Some(FaultPoint::UpsertRepoState))
                    .with_repo(repo_record.clone())
                    .with_maintainer(actor.clone()),
                &repo_archive
            )
            .await,
            Err(DispatchError::Storage(_))
        ));

        let repo_maintainers = CommandExecutionInput {
            event_id: vec![97u8; 32],
            actor_pubkey: actor.clone(),
            parsed: parse(&format!("gittree repo maintainers demo add {actor_npub}")),
            created_at: 704,
        };
        assert!(matches!(
            super::apply_repo(
                &FaultStore::new(Some(FaultPoint::ListActiveRepoMaintainers))
                    .with_repo(repo_record.clone()),
                &repo_maintainers
            )
            .await,
            Err(DispatchError::Storage(_))
        ));
        assert!(matches!(
            super::apply_repo(
                &FaultStore::new(Some(FaultPoint::SetRepoMaintainer))
                    .with_repo(repo_record)
                    .with_maintainer(actor),
                &repo_maintainers
            )
            .await,
            Err(DispatchError::Storage(_))
        ));
    }
}
