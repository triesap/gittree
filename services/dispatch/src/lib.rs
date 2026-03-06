use futures_util::{SinkExt, StreamExt};
pub use gittree_core::{CommandParseError, ParsedCommand, parse_cli_command};
use gittree_observability::{ObservabilityConfigError, ObservabilityError, ObservabilityHandle};
use gittree_storage::{PostgresRepositories, StorageConfig, StorageError};
use std::sync::Arc;
use std::time::Duration;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
pub mod handlers;
pub mod ingest;
use handlers::{CommandExecutionInput, CommandExecutionOutput, CommandStore, execute_command};
pub use ingest::{
    DispatchFilterConfig, IngestRejectReason, RelayEventEnvelope, is_dispatch_command_event,
};

const ENV_BIND: &str = "GITTREE_DISPATCH_BIND";
const ENV_ADMIN_PUBKEY: &str = "GITTREE_DISPATCH_ADMIN_PUBKEY";
const ENV_RELAY_URLS: &str = "GITTREE_DISPATCH_RELAY_URLS";
const ENV_STORAGE_READ_URL: &str = "GITTREE_STORAGE_READ_URL";
const ENV_STORAGE_WRITE_URL: &str = "GITTREE_STORAGE_WRITE_URL";
const ENV_STORAGE_MAX_CONNECTIONS: &str = "GITTREE_STORAGE_MAX_CONNECTIONS";
const ENV_STORAGE_MIN_CONNECTIONS: &str = "GITTREE_STORAGE_MIN_CONNECTIONS";
const ENV_STORAGE_IDLE_TIMEOUT_SECS: &str = "GITTREE_STORAGE_IDLE_TIMEOUT_SECS";
const ENV_STORAGE_MAX_LIFETIME_SECS: &str = "GITTREE_STORAGE_MAX_LIFETIME_SECS";
const ENV_STORAGE_APP_NAME: &str = "GITTREE_STORAGE_APP_NAME";
const DISPATCH_SUB_ID: &str = "gittree-dispatch";
const RELAY_RETRY_DELAY_SECS: u64 = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DispatchConfig {
    pub bind: String,
    pub admin_pubkey: String,
    pub relay_urls: Vec<String>,
    pub storage: StorageConfig,
}

impl DispatchConfig {
    pub fn from_env() -> Result<Self, DispatchError> {
        let mut get_var = |key| std::env::var(key).ok();
        Self::from_env_with(&mut get_var)
    }

    pub fn from_env_with(
        get_var: &mut dyn FnMut(&'static str) -> Option<String>,
    ) -> Result<Self, DispatchError> {
        let bind = get_var(ENV_BIND).unwrap_or_else(|| "127.0.0.1:8091".to_string());
        let admin_pubkey = get_var(ENV_ADMIN_PUBKEY)
            .ok_or_else(|| DispatchError::Config(format!("missing env {ENV_ADMIN_PUBKEY}")))?;
        let relay_urls = parse_csv(&get_var(ENV_RELAY_URLS).unwrap_or_default());
        if relay_urls.is_empty() {
            return Err(DispatchError::Config(format!(
                "missing relay urls in {ENV_RELAY_URLS}"
            )));
        }
        let storage = storage_from_env(get_var)?;
        Ok(Self {
            bind,
            admin_pubkey,
            relay_urls,
            storage,
        })
    }
}

fn parse_csv(input: &str) -> Vec<String> {
    input
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn storage_from_env(
    get_var: &mut dyn FnMut(&'static str) -> Option<String>,
) -> Result<StorageConfig, DispatchError> {
    let read_connection = get_var(ENV_STORAGE_READ_URL)
        .ok_or_else(|| DispatchError::Config(format!("missing env {ENV_STORAGE_READ_URL}")))?;
    let write_connection = get_var(ENV_STORAGE_WRITE_URL);
    let max_connections = env_u32(get_var, ENV_STORAGE_MAX_CONNECTIONS)?.unwrap_or(10);
    let min_connections = env_u32(get_var, ENV_STORAGE_MIN_CONNECTIONS)?.unwrap_or(2);
    let idle_timeout_secs = env_u64(get_var, ENV_STORAGE_IDLE_TIMEOUT_SECS)?;
    let max_lifetime_secs = env_u64(get_var, ENV_STORAGE_MAX_LIFETIME_SECS)?;
    let application_name = get_var(ENV_STORAGE_APP_NAME);

    let storage = StorageConfig {
        read_connection,
        write_connection,
        max_connections,
        min_connections,
        idle_timeout_secs,
        max_lifetime_secs,
        application_name,
    };

    storage
        .validate()
        .map_err(|err| DispatchError::Config(err.to_string()))?;
    Ok(storage)
}

fn env_u32(
    get_var: &mut dyn FnMut(&'static str) -> Option<String>,
    key: &'static str,
) -> Result<Option<u32>, DispatchError> {
    let Some(value) = get_var(key) else {
        return Ok(None);
    };
    if value.trim().is_empty() {
        return Ok(None);
    }
    value
        .parse::<u32>()
        .map(Some)
        .map_err(|_| DispatchError::Config(format!("invalid env {key}: {value}")))
}

fn env_u64(
    get_var: &mut dyn FnMut(&'static str) -> Option<String>,
    key: &'static str,
) -> Result<Option<u64>, DispatchError> {
    let Some(value) = get_var(key) else {
        return Ok(None);
    };
    if value.trim().is_empty() {
        return Ok(None);
    }
    value
        .parse::<u64>()
        .map(Some)
        .map_err(|_| DispatchError::Config(format!("invalid env {key}: {value}")))
}

#[derive(Debug)]
pub enum DispatchError {
    Config(String),
    Storage(StorageError),
    ObservabilityConfig(ObservabilityConfigError),
    Observability(ObservabilityError),
}

impl std::fmt::Display for DispatchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DispatchError::Config(message) => write!(f, "dispatch config error: {message}"),
            DispatchError::Storage(err) => write!(f, "dispatch storage error: {err}"),
            DispatchError::ObservabilityConfig(err) => {
                write!(f, "dispatch observability config error: {err}")
            }
            DispatchError::Observability(err) => write!(f, "dispatch observability error: {err}"),
        }
    }
}

impl std::error::Error for DispatchError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            DispatchError::Config(_) => None,
            DispatchError::Storage(err) => Some(err),
            DispatchError::ObservabilityConfig(err) => Some(err),
            DispatchError::Observability(err) => Some(err),
        }
    }
}

pub fn init_observability() -> Result<ObservabilityHandle, DispatchError> {
    let config = gittree_observability::ObservabilityConfig::from_env("gittree-dispatch")
        .map_err(DispatchError::ObservabilityConfig)?;
    let handle = gittree_observability::init(&config).map_err(DispatchError::Observability)?;
    Ok(handle)
}

pub async fn serve(config: DispatchConfig) -> Result<(), DispatchError> {
    let _guard = init_observability()?;
    let repositories = Arc::new(build_repositories(&config)?);
    let filter = dispatch_filter_config(&config);
    tracing::info!(
        bind = %config.bind,
        relay_count = config.relay_urls.len(),
        storage = %config.storage.read_connection,
        "dispatch relay subscriber initialized"
    );

    let mut tasks = tokio::task::JoinSet::new();
    for relay_url in &config.relay_urls {
        let store = Arc::clone(&repositories);
        let filter = filter.clone();
        let relay_url = relay_url.clone();
        tasks.spawn(async move {
            run_relay_subscription(store, filter, relay_url).await;
        });
    }

    if let Err(err) = tokio::signal::ctrl_c().await {
        return Err(DispatchError::Config(format!(
            "dispatch shutdown signal failed: {err}"
        )));
    }

    tasks.abort_all();
    while tasks.join_next().await.is_some() {}
    Ok(())
}

pub fn build_repositories(config: &DispatchConfig) -> Result<PostgresRepositories, DispatchError> {
    let pool_options = config
        .storage
        .pool_options()
        .map_err(DispatchError::Storage)?;
    let connect_options = config
        .storage
        .read_connect_options()
        .map_err(DispatchError::Storage)?;
    let pool = pool_options.connect_lazy_with(connect_options);
    Ok(PostgresRepositories::new(pool))
}

pub fn parse_command_content(content: &str) -> Result<ParsedCommand, CommandParseError> {
    parse_cli_command(content)
}

pub fn dispatch_filter_config(config: &DispatchConfig) -> DispatchFilterConfig {
    DispatchFilterConfig {
        admin_pubkey: config.admin_pubkey.clone(),
        relay_allowlist: config.relay_urls.clone(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DispatchEventOutcome {
    Ignored(IngestRejectReason),
    Rejected(String),
    Applied(CommandExecutionOutput),
}

pub async fn process_event_envelope<S: CommandStore>(
    store: &S,
    filter: &DispatchFilterConfig,
    envelope: RelayEventEnvelope,
) -> Result<DispatchEventOutcome, DispatchError> {
    if let Err(reason) = is_dispatch_command_event(filter, &envelope) {
        return Ok(DispatchEventOutcome::Ignored(reason));
    }

    let parsed = match parse_command_content(&envelope.content) {
        Ok(parsed) => parsed,
        Err(err) => return Ok(DispatchEventOutcome::Rejected(err.to_string())),
    };

    let event_id = match decode_fixed_hex(&envelope.id, "event id") {
        Ok(bytes) => bytes,
        Err(message) => return Ok(DispatchEventOutcome::Rejected(message)),
    };
    let actor_pubkey = match decode_fixed_hex(&envelope.pubkey, "pubkey") {
        Ok(bytes) => bytes,
        Err(message) => return Ok(DispatchEventOutcome::Rejected(message)),
    };

    let output = execute_command(
        store,
        CommandExecutionInput {
            event_id,
            actor_pubkey,
            parsed,
            created_at: envelope.created_at,
        },
    )
    .await?;

    Ok(DispatchEventOutcome::Applied(output))
}

fn decode_fixed_hex(value: &str, field: &str) -> Result<Vec<u8>, String> {
    match hex::decode(value) {
        Ok(bytes) if bytes.len() == 32 => Ok(bytes),
        Ok(_) => Err(format!("invalid {field}: expected 32-byte hex")),
        Err(_) => Err(format!("invalid {field}: expected hex")),
    }
}

fn build_relay_req_message(admin_pubkey: &str) -> String {
    serde_json::json!(["REQ", DISPATCH_SUB_ID, {"kinds":[1], "#p":[admin_pubkey]}]).to_string()
}

fn parse_relay_event_message(message: &str, relay_url: &str) -> Option<RelayEventEnvelope> {
    let value = serde_json::from_str::<serde_json::Value>(message).ok()?;
    let parts = value.as_array()?;
    if parts.len() < 3 || parts.first()?.as_str()? != "EVENT" {
        return None;
    }

    let event = parts.get(2)?.as_object()?;
    let id = event.get("id")?.as_str()?.to_string();
    let pubkey = event.get("pubkey")?.as_str()?.to_string();
    let kind = event.get("kind")?.as_u64()? as u32;
    let created_at = event.get("created_at")?.as_i64()?;
    let content = event.get("content")?.as_str()?.to_string();
    let tags = parse_event_tags(event.get("tags")?)?;

    Some(RelayEventEnvelope {
        id,
        pubkey,
        kind,
        created_at,
        content,
        tags,
        relay_url: relay_url.to_string(),
    })
}

fn parse_event_tags(value: &serde_json::Value) -> Option<Vec<Vec<String>>> {
    let rows = value.as_array()?;
    let mut tags = Vec::with_capacity(rows.len());
    for row in rows {
        let row = row.as_array()?;
        let mut columns = Vec::with_capacity(row.len());
        for col in row {
            columns.push(col.as_str()?.to_string());
        }
        tags.push(columns);
    }
    Some(tags)
}

async fn process_event_message<S: CommandStore>(
    store: &S,
    filter: &DispatchFilterConfig,
    relay_url: &str,
    message: &str,
) -> Result<Option<DispatchEventOutcome>, DispatchError> {
    let Some(envelope) = parse_relay_event_message(message, relay_url) else {
        return Ok(None);
    };
    let outcome = process_event_envelope(store, filter, envelope).await?;
    Ok(Some(outcome))
}

async fn run_relay_subscription<S: CommandStore + Send + Sync + 'static>(
    store: Arc<S>,
    filter: DispatchFilterConfig,
    relay_url: String,
) {
    loop {
        match connect_async(relay_url.as_str()).await {
            Ok((stream, _response)) => {
                tracing::info!(relay = %relay_url, "dispatch relay connected");
                let (mut writer, mut reader) = stream.split();
                let req = build_relay_req_message(&filter.admin_pubkey);
                if writer.send(Message::Text(req)).await.is_err() {
                    tracing::warn!(relay = %relay_url, "dispatch failed to send relay req");
                    tokio::time::sleep(Duration::from_secs(RELAY_RETRY_DELAY_SECS)).await;
                    continue;
                }

                while let Some(next) = reader.next().await {
                    match next {
                        Ok(Message::Text(text)) => {
                            match process_event_message(store.as_ref(), &filter, &relay_url, &text)
                                .await
                            {
                                Ok(Some(DispatchEventOutcome::Applied(output))) => {
                                    tracing::info!(
                                        relay = %relay_url,
                                        code = %output.code,
                                        "dispatch applied command event"
                                    );
                                }
                                Ok(Some(DispatchEventOutcome::Ignored(reason))) => {
                                    tracing::debug!(relay = %relay_url, ?reason, "dispatch ignored relay event");
                                }
                                Ok(Some(DispatchEventOutcome::Rejected(message))) => {
                                    tracing::warn!(
                                        relay = %relay_url,
                                        %message,
                                        "dispatch rejected relay event"
                                    );
                                }
                                Ok(None) => {}
                                Err(err) => {
                                    tracing::error!(relay = %relay_url, error = %err, "dispatch event processing failed");
                                }
                            }
                        }
                        Ok(Message::Ping(payload)) => {
                            if writer.send(Message::Pong(payload)).await.is_err() {
                                break;
                            }
                        }
                        Ok(Message::Close(_)) => break,
                        Ok(_) => {}
                        Err(err) => {
                            tracing::warn!(relay = %relay_url, error = %err, "dispatch relay read failed");
                            break;
                        }
                    }
                }
            }
            Err(err) => {
                tracing::warn!(relay = %relay_url, error = %err, "dispatch relay connect failed");
            }
        }
        tokio::time::sleep(Duration::from_secs(RELAY_RETRY_DELAY_SECS)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DispatchConfig, DispatchError, DispatchEventOutcome, DispatchFilterConfig,
        RelayEventEnvelope, build_relay_req_message, dispatch_filter_config, parse_csv,
        parse_relay_event_message, process_event_envelope, process_event_message,
    };
    use crate::handlers::CommandStore;
    use async_trait::async_trait;
    use gittree_storage::{
        AccountStateRecord, CommandLogRecord, CommandStatus, ProfileStateRecord,
        RepoMaintainerV1Record, RepoStateV1Record,
    };
    use std::collections::{HashMap, HashSet};
    use std::sync::Mutex;

    fn base_env() -> Vec<(&'static str, &'static str)> {
        vec![
            ("GITTREE_DISPATCH_ADMIN_PUBKEY", "npub1admin"),
            ("GITTREE_DISPATCH_RELAY_URLS", "wss://gittr.ee"),
            (
                "GITTREE_STORAGE_READ_URL",
                "postgres://gittree:gittree@127.0.0.1:5432/gittree",
            ),
        ]
    }

    fn from_pairs(
        values: &[(&'static str, &'static str)],
    ) -> Result<DispatchConfig, DispatchError> {
        let map: HashMap<&'static str, &'static str> = values.iter().copied().collect();
        let mut get_var = |key: &'static str| map.get(key).map(|value| value.to_string());
        DispatchConfig::from_env_with(&mut get_var)
    }

    fn event(content: &str) -> RelayEventEnvelope {
        RelayEventEnvelope {
            id: "11".repeat(32),
            pubkey: "22".repeat(32),
            kind: 1,
            created_at: 123,
            content: content.to_string(),
            tags: vec![vec!["p".to_string(), "npub1admin".to_string()]],
            relay_url: "wss://gittr.ee".to_string(),
        }
    }

    fn filter() -> DispatchFilterConfig {
        DispatchFilterConfig {
            admin_pubkey: "npub1admin".to_string(),
            relay_allowlist: vec!["wss://gittr.ee".to_string()],
        }
    }

    #[derive(Default)]
    struct EventStore {
        command_log: Mutex<HashSet<Vec<u8>>>,
        accounts: Mutex<HashMap<Vec<u8>, AccountStateRecord>>,
    }

    #[async_trait]
    impl CommandStore for EventStore {
        async fn insert_command_log(
            &self,
            record: &CommandLogRecord,
        ) -> Result<bool, DispatchError> {
            let mut log = self.command_log.lock().expect("command log");
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

        async fn account_state(
            &self,
            pubkey: &[u8],
        ) -> Result<Option<AccountStateRecord>, DispatchError> {
            let map = self.accounts.lock().expect("accounts");
            Ok(map.get(pubkey).cloned())
        }

        async fn upsert_account_state(
            &self,
            record: &AccountStateRecord,
        ) -> Result<(), DispatchError> {
            self.accounts
                .lock()
                .expect("accounts")
                .insert(record.pubkey.clone(), record.clone());
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

    #[test]
    fn parse_csv_handles_empty_segments() {
        let values = parse_csv("a, ,b,, c ");
        assert_eq!(values, vec!["a", "b", "c"]);
    }

    #[test]
    fn parse_command_content_delegates_to_core_parser() {
        let command = super::parse_command_content("gittree account create").expect("command");
        assert_eq!(command.action, "create");
    }

    #[test]
    fn dispatch_filter_config_uses_dispatch_settings() {
        let mut env = base_env();
        env.push((
            "GITTREE_DISPATCH_RELAY_URLS",
            "wss://gittr.ee,wss://relay.example",
        ));
        let config = from_pairs(&env).expect("config");
        let filter = dispatch_filter_config(&config);
        assert_eq!(filter.admin_pubkey, "npub1admin");
        assert_eq!(
            filter.relay_allowlist,
            vec![
                "wss://gittr.ee".to_string(),
                "wss://relay.example".to_string()
            ]
        );
    }

    #[test]
    fn from_env_requires_admin_pubkey() {
        let env = [
            ("GITTREE_DISPATCH_RELAY_URLS", "wss://gittr.ee"),
            (
                "GITTREE_STORAGE_READ_URL",
                "postgres://gittree:gittree@127.0.0.1:5432/gittree",
            ),
        ];
        let err = from_pairs(&env).expect_err("missing admin key");
        assert!(
            matches!(err, DispatchError::Config(message) if message.contains("GITTREE_DISPATCH_ADMIN_PUBKEY"))
        );
    }

    #[test]
    fn from_env_requires_relay_urls() {
        let env = [
            ("GITTREE_DISPATCH_ADMIN_PUBKEY", "npub1admin"),
            (
                "GITTREE_STORAGE_READ_URL",
                "postgres://gittree:gittree@127.0.0.1:5432/gittree",
            ),
        ];
        let err = from_pairs(&env).expect_err("missing relay urls");
        assert!(
            matches!(err, DispatchError::Config(message) if message.contains("GITTREE_DISPATCH_RELAY_URLS"))
        );
    }

    #[test]
    fn from_env_requires_storage_url() {
        let env = [
            ("GITTREE_DISPATCH_ADMIN_PUBKEY", "npub1admin"),
            ("GITTREE_DISPATCH_RELAY_URLS", "wss://gittr.ee"),
        ];
        let err = from_pairs(&env).expect_err("missing storage");
        assert!(
            matches!(err, DispatchError::Config(message) if message.contains("GITTREE_STORAGE_READ_URL"))
        );
    }

    #[test]
    fn from_env_loads_expected_values() {
        let mut env = base_env();
        env.push(("GITTREE_DISPATCH_BIND", "127.0.0.1:19091"));
        env.push(("GITTREE_STORAGE_MAX_CONNECTIONS", "16"));
        env.push(("GITTREE_STORAGE_MIN_CONNECTIONS", "3"));
        let config = from_pairs(&env).expect("config");
        assert_eq!(config.bind, "127.0.0.1:19091");
        assert_eq!(config.admin_pubkey, "npub1admin");
        assert_eq!(config.storage.max_connections, 16);
        assert_eq!(config.storage.min_connections, 3);
        assert_eq!(config.relay_urls, vec!["wss://gittr.ee".to_string()]);
    }

    #[tokio::test]
    async fn process_event_ignores_non_dispatch_messages() {
        let store = EventStore::default();
        let mut envelope = event("gittree account create");
        envelope.kind = 7;
        let outcome = process_event_envelope(&store, &filter(), envelope)
            .await
            .expect("outcome");
        assert_eq!(
            outcome,
            DispatchEventOutcome::Ignored(super::IngestRejectReason::WrongKind)
        );
    }

    #[tokio::test]
    async fn process_event_rejects_invalid_payload_shapes() {
        let store = EventStore::default();
        let mut bad_pubkey = event("gittree account create");
        bad_pubkey.pubkey = "zz".to_string();
        let outcome = process_event_envelope(&store, &filter(), bad_pubkey)
            .await
            .expect("outcome");
        assert!(
            matches!(outcome, DispatchEventOutcome::Rejected(message) if message.contains("invalid pubkey"))
        );

        let bad_command = event("gittree account nope");
        let outcome = process_event_envelope(&store, &filter(), bad_command)
            .await
            .expect("outcome");
        assert!(
            matches!(outcome, DispatchEventOutcome::Rejected(message) if message.contains("invalid command"))
        );
    }

    #[tokio::test]
    async fn process_event_applies_command_to_store() {
        let store = EventStore::default();
        let envelope = event("gittree account create");
        let outcome = process_event_envelope(&store, &filter(), envelope)
            .await
            .expect("outcome");
        assert!(
            matches!(outcome, DispatchEventOutcome::Applied(output) if output.code == "account_created")
        );
        let account = store
            .account_state(&hex::decode("22".repeat(32)).expect("actor"))
            .await
            .expect("account lookup");
        assert!(account.is_some());
    }

    #[test]
    fn build_relay_req_message_uses_nostr_req_shape() {
        let encoded = build_relay_req_message("npub1admin");
        let parsed = serde_json::from_str::<serde_json::Value>(&encoded).expect("json");
        let parts = parsed.as_array().expect("array");
        assert_eq!(parts[0], "REQ");
        assert_eq!(parts[1], "gittree-dispatch");
        assert_eq!(parts[2]["kinds"][0], 1);
        assert_eq!(parts[2]["#p"][0], "npub1admin");
    }

    #[test]
    fn parse_relay_event_message_extracts_envelope() {
        let message = serde_json::json!([
            "EVENT",
            "gittree-dispatch",
            {
                "id": "11".repeat(32),
                "pubkey": "22".repeat(32),
                "kind": 1,
                "created_at": 321,
                "content": "gittree account create",
                "tags": [["p", "npub1admin"]]
            }
        ])
        .to_string();
        let envelope = parse_relay_event_message(&message, "wss://gittr.ee").expect("envelope");
        assert_eq!(envelope.kind, 1);
        assert_eq!(envelope.created_at, 321);
        assert_eq!(envelope.relay_url, "wss://gittr.ee");
    }

    #[tokio::test]
    async fn process_event_message_ignores_non_event_payloads() {
        let store = EventStore::default();
        let message = serde_json::json!(["NOTICE", "ok"]).to_string();
        let outcome = process_event_message(&store, &filter(), "wss://gittr.ee", &message)
            .await
            .expect("result");
        assert!(outcome.is_none());
    }
}
