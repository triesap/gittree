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

pub async fn execute_command<S: CommandStore>(
    store: &S,
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

async fn apply_command<S: CommandStore>(
    store: &S,
    input: &CommandExecutionInput,
) -> Result<CommandExecutionOutput, DispatchError> {
    match input.parsed.namespace {
        CommandNamespace::Account => apply_account(store, input).await,
        CommandNamespace::Profile => apply_profile(store, input).await,
        CommandNamespace::Repo => apply_repo(store, input).await,
    }
}

async fn apply_account<S: CommandStore>(
    store: &S,
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

async fn apply_profile<S: CommandStore>(
    store: &S,
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

async fn apply_repo<S: CommandStore>(
    store: &S,
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

async fn actor_is_maintainer<S: CommandStore>(
    store: &S,
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
    use super::{CommandExecutionInput, CommandStore, execute_command};
    use crate::DispatchError;
    use async_trait::async_trait;
    use gittree_app_core::npub_from_bytes;
    use gittree_core::{CommandArg, CommandNamespace, ParsedCommand};
    use gittree_storage::{
        AccountLifecycle, AccountStateRecord, CommandLogRecord, CommandStatus,
        ProfileStateRecord, ProfileVisibilityV1, RepoMaintainerV1Record, RepoStateV1Record,
        RepoVisibilityV1,
    };
    use std::collections::{HashMap, HashSet};
    use std::sync::Mutex;

    #[derive(Default)]
    struct MemoryStore {
        command_log: Mutex<HashSet<Vec<u8>>>,
        accounts: Mutex<HashMap<Vec<u8>, AccountStateRecord>>,
        profiles: Mutex<HashMap<Vec<u8>, ProfileStateRecord>>,
        repos: Mutex<HashMap<(Vec<u8>, String), RepoStateV1Record>>,
        maintainers: Mutex<HashMap<(Vec<u8>, String), HashSet<Vec<u8>>>>,
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
}
