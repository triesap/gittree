use futures_util::{Sink, SinkExt, Stream, StreamExt};
pub use gittree_core::{CommandParseError, ParsedCommand, parse_cli_command};
use gittree_observability::{ObservabilityConfigError, ObservabilityError, ObservabilityHandle};
use gittree_storage::{PostgresRepositories, StorageConfig, StorageError};
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::{Error as WsError, Message};
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
    serve_with_shutdown(config, tokio::signal::ctrl_c()).await }

async fn serve_with_shutdown(
    config: DispatchConfig,
    shutdown: impl Future<Output = Result<(), std::io::Error>>,
) -> Result<(), DispatchError> {
    let repositories: Arc<dyn CommandStore + Send + Sync> = Arc::new(build_repositories(&config)?);
    let filter = dispatch_filter_config(&config);
    tracing::info!(bind = %config.bind, relay_count = config.relay_urls.len(), storage = %config.storage.read_connection, "dispatch relay subscriber initialized");

    let mut tasks = tokio::task::JoinSet::new();
    for relay_url in &config.relay_urls {
        let store = Arc::clone(&repositories);
        let filter = filter.clone();
        let relay_url = relay_url.clone();
        tasks.spawn(run_relay_subscription(store, filter, relay_url));
    }

    if let Err(err) = shutdown.await {
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

pub async fn process_event_envelope(
    store: &dyn CommandStore,
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

async fn process_event_message(
    store: &dyn CommandStore,
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

async fn run_relay_subscription(
    store: Arc<dyn CommandStore + Send + Sync>,
    filter: DispatchFilterConfig,
    relay_url: String,
) {
    loop {
        match connect_async(relay_url.as_str()).await {
            Ok((stream, _response)) => {
                tracing::info!(relay = %relay_url, "dispatch relay connected");
                let (mut writer, mut reader) = stream.split();
                process_relay_connection(
                    store.as_ref(),
                    &filter,
                    &relay_url,
                    &mut writer,
                    &mut reader,
                )
                .await;
            }
            Err(err) => {
                tracing::warn!(relay = %relay_url, error = %err, "dispatch relay connect failed");
            }
        }
        tokio::time::sleep(Duration::from_secs(RELAY_RETRY_DELAY_SECS)).await;
    }
}

async fn process_relay_connection<W, R>(
    store: &dyn CommandStore,
    filter: &DispatchFilterConfig,
    relay_url: &str,
    writer: &mut W,
    reader: &mut R,
) where
    W: Sink<Message, Error = WsError> + Unpin,
    R: Stream<Item = Result<Message, WsError>> + Unpin,
{
    let req = build_relay_req_message(&filter.admin_pubkey);
    if writer.send(Message::Text(req)).await.is_err() {
        tracing::warn!(relay = %relay_url, "dispatch failed to send relay req");
        return;
    }

    while let Some(next) = reader.next().await {
        match next {
            Ok(Message::Text(text)) => match process_event_message(store, filter, relay_url, &text).await {
                Ok(Some(DispatchEventOutcome::Applied(output))) => {
                    tracing::info!(relay = %relay_url, code = %output.code, "dispatch applied command event");
                }
                Ok(Some(DispatchEventOutcome::Ignored(reason))) => {
                    tracing::debug!(relay = %relay_url, ?reason, "dispatch ignored relay event");
                }
                Ok(Some(DispatchEventOutcome::Rejected(message))) => {
                    tracing::warn!(relay = %relay_url, %message, "dispatch rejected relay event");
                }
                Ok(None) => {}
                Err(err) => {
                    tracing::error!(relay = %relay_url, error = %err, "dispatch event processing failed");
                }
            },
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

#[cfg(test)]
mod tests {
    use super::{
        DispatchConfig, DispatchError, DispatchEventOutcome, DispatchFilterConfig,
        RelayEventEnvelope, build_repositories, build_relay_req_message, dispatch_filter_config,
        parse_csv, parse_relay_event_message, process_event_envelope, process_event_message,
        process_relay_connection, run_relay_subscription, serve_with_shutdown,
    };
    use crate::handlers::CommandStore;
    use async_trait::async_trait;
    use futures_util::{SinkExt, StreamExt};
    use gittree_storage::{
        AccountStateRecord, CommandLogRecord, CommandStatus, ProfileStateRecord,
        RepoMaintainerV1Record, RepoStateV1Record,
    };
    use std::collections::{HashMap, HashSet};
    use std::io;
    use std::pin::Pin;
    use std::sync::{Arc, Mutex, OnceLock};
    use std::task::{Context, Poll};
    use std::time::Duration;
    use tokio::net::TcpListener;
    use tokio_tungstenite::accept_async;
    use tokio_tungstenite::tungstenite::{Error as WsError, Message};

    #[derive(Default)]
    struct ScriptedWriter {
        fail_send_on: Option<usize>,
        send_count: usize,
    }

    impl futures_util::Sink<Message> for ScriptedWriter {
        type Error = WsError;

        fn poll_ready(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
        ) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(Ok(()))
        }

        fn start_send(
            mut self: Pin<&mut Self>,
            _item: Message,
        ) -> Result<(), Self::Error> {
            self.send_count += 1;
            if self.fail_send_on == Some(self.send_count) {
                return Err(WsError::Io(io::Error::other("scripted send failure")));
            }
            Ok(())
        }

        fn poll_flush(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
        ) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(Ok(()))
        }

        fn poll_close(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
        ) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(Ok(()))
        }
    }

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

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    fn with_env_vars(vars: &[(&str, &str)], run: impl FnOnce()) {
        let _guard = env_lock().lock().expect("env lock");
        let mut previous = Vec::with_capacity(vars.len());
        for (key, value) in vars {
            let before = std::env::var(key).ok();
            previous.push((*key, before));
            unsafe { std::env::set_var(key, value) };
        }
        run();
        for (key, value) in previous {
            match value {
                Some(existing) => unsafe { std::env::set_var(key, existing) },
                None => unsafe { std::env::remove_var(key) },
            }
        }
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
        fail_insert: bool,
    }

    #[async_trait]
    impl CommandStore for EventStore {
        async fn insert_command_log(
            &self,
            record: &CommandLogRecord,
        ) -> Result<bool, DispatchError> {
            if self.fail_insert {
                return Err(DispatchError::Config("boom".to_string()));
            }
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

    #[test]
    fn from_env_rejects_invalid_storage_numeric_values() {
        let mut env = base_env();
        env.push(("GITTREE_STORAGE_MAX_CONNECTIONS", "abc"));
        let err = from_pairs(&env).expect_err("invalid max");
        assert!(matches!(
            err,
            DispatchError::Config(message)
            if message.contains("GITTREE_STORAGE_MAX_CONNECTIONS")
        ));
    }

    #[test]
    fn from_env_rejects_invalid_storage_timeout_values() {
        let mut env = base_env();
        env.push(("GITTREE_STORAGE_IDLE_TIMEOUT_SECS", "xyz"));
        let err = from_pairs(&env).expect_err("invalid idle timeout");
        assert!(matches!(
            err,
            DispatchError::Config(message)
            if message.contains("GITTREE_STORAGE_IDLE_TIMEOUT_SECS")
        ));
    }

    #[test]
    fn from_env_rejects_invalid_storage_pool_bounds() {
        let mut env = base_env();
        env.push(("GITTREE_STORAGE_MAX_CONNECTIONS", "1"));
        env.push(("GITTREE_STORAGE_MIN_CONNECTIONS", "2"));
        let err = from_pairs(&env).expect_err("invalid bounds");
        assert!(matches!(
            err,
            DispatchError::Config(message)
            if message.contains("max_connections") || message.contains("min_connections")
        ));
    }

    #[test]
    fn from_env_treats_blank_optional_storage_values_as_defaults() {
        let mut env = base_env();
        env.push(("GITTREE_STORAGE_MAX_CONNECTIONS", "   "));
        env.push(("GITTREE_STORAGE_MIN_CONNECTIONS", ""));
        let config = from_pairs(&env).expect("config");
        assert_eq!(config.storage.max_connections, 10);
        assert_eq!(config.storage.min_connections, 2);
    }

    #[test]
    fn from_env_reads_process_environment() {
        unsafe { std::env::set_var("GITTREE_DISPATCH_BIND", "127.0.0.1:19991") };
        with_env_vars(
            &[
                ("GITTREE_DISPATCH_BIND", "127.0.0.1:19091"),
                ("GITTREE_DISPATCH_ADMIN_PUBKEY", "npub1admin"),
                ("GITTREE_DISPATCH_RELAY_URLS", "wss://gittr.ee"),
                (
                    "GITTREE_STORAGE_READ_URL",
                    "postgres://gittree:gittree@127.0.0.1:5432/gittree",
                ),
            ],
            || {
                let config = DispatchConfig::from_env().expect("config");
                assert_eq!(config.bind, "127.0.0.1:19091");
                assert_eq!(config.admin_pubkey, "npub1admin");
                assert_eq!(config.relay_urls, vec!["wss://gittr.ee".to_string()]);
            },
        );
        assert_eq!(
            std::env::var("GITTREE_DISPATCH_BIND").expect("restored bind"),
            "127.0.0.1:19991"
        );
        unsafe { std::env::remove_var("GITTREE_DISPATCH_BIND") };
    }

    #[test]
    fn from_env_treats_blank_u64_values_as_none() {
        let mut env = base_env();
        env.push(("GITTREE_STORAGE_IDLE_TIMEOUT_SECS", " "));
        env.push(("GITTREE_STORAGE_MAX_LIFETIME_SECS", ""));
        let config = from_pairs(&env).expect("config");
        assert_eq!(config.storage.idle_timeout_secs, None);
        assert_eq!(config.storage.max_lifetime_secs, None);
    }

    #[test]
    fn from_env_loads_optional_storage_fields() {
        let mut env = base_env();
        env.push((
            "GITTREE_STORAGE_WRITE_URL",
            "postgres://gittree:gittree@127.0.0.1:5432/gittree",
        ));
        env.push(("GITTREE_STORAGE_IDLE_TIMEOUT_SECS", "60"));
        env.push(("GITTREE_STORAGE_MAX_LIFETIME_SECS", "120"));
        env.push(("GITTREE_STORAGE_APP_NAME", "dispatch-tests"));
        let config = from_pairs(&env).expect("config");
        assert!(config.storage.write_connection.is_some());
        assert_eq!(config.storage.idle_timeout_secs, Some(60));
        assert_eq!(config.storage.max_lifetime_secs, Some(120));
        assert_eq!(
            config.storage.application_name.as_deref(),
            Some("dispatch-tests")
        );
    }

    #[tokio::test]
    async fn build_repositories_constructs_lazy_pool_from_config() {
        let config = from_pairs(&base_env()).expect("config");
        let _repositories = build_repositories(&config).expect("repositories");
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

    #[test]
    fn parse_relay_event_message_rejects_invalid_payload_shapes() {
        let invalid_kind = serde_json::json!([
            "EVENT",
            "gittree-dispatch",
            {
                "id": "11".repeat(32),
                "pubkey": "22".repeat(32),
                "kind": "1",
                "created_at": 321,
                "content": "gittree account create",
                "tags": [["p", "npub1admin"]]
            }
        ])
        .to_string();
        assert!(parse_relay_event_message(&invalid_kind, "wss://gittr.ee").is_none());

        assert!(parse_relay_event_message("{not-json", "wss://gittr.ee").is_none());
        assert!(parse_relay_event_message("[]", "wss://gittr.ee").is_none());

        let notice = serde_json::json!(["NOTICE", "ok"]).to_string();
        assert!(parse_relay_event_message(&notice, "wss://gittr.ee").is_none());

        let invalid_tags = serde_json::json!([
            "EVENT",
            "gittree-dispatch",
            {
                "id": "11".repeat(32),
                "pubkey": "22".repeat(32),
                "kind": 1,
                "created_at": 321,
                "content": "gittree account create",
                "tags": [1, 2, 3]
            }
        ])
        .to_string();
        assert!(parse_relay_event_message(&invalid_tags, "wss://gittr.ee").is_none());

        let invalid_tag_column = serde_json::json!([
            "EVENT",
            "gittree-dispatch",
            {
                "id": "11".repeat(32),
                "pubkey": "22".repeat(32),
                "kind": 1,
                "created_at": 321,
                "content": "gittree account create",
                "tags": [["p", 1]]
            }
        ])
        .to_string();
        assert!(parse_relay_event_message(&invalid_tag_column, "wss://gittr.ee").is_none());
    }

    #[test]
    fn parse_relay_event_message_rejects_missing_required_fields() {
        let missing_id = serde_json::json!([
            "EVENT",
            "gittree-dispatch",
            {
                "pubkey": "22".repeat(32),
                "kind": 1,
                "created_at": 321,
                "content": "gittree account create",
                "tags": [["p", "npub1admin"]]
            }
        ])
        .to_string();
        assert!(parse_relay_event_message(&missing_id, "wss://gittr.ee").is_none());

        let missing_pubkey = serde_json::json!([
            "EVENT",
            "gittree-dispatch",
            {
                "id": "11".repeat(32),
                "kind": 1,
                "created_at": 321,
                "content": "gittree account create",
                "tags": [["p", "npub1admin"]]
            }
        ])
        .to_string();
        assert!(parse_relay_event_message(&missing_pubkey, "wss://gittr.ee").is_none());

        let missing_created_at = serde_json::json!([
            "EVENT",
            "gittree-dispatch",
            {
                "id": "11".repeat(32),
                "pubkey": "22".repeat(32),
                "kind": 1,
                "content": "gittree account create",
                "tags": [["p", "npub1admin"]]
            }
        ])
        .to_string();
        assert!(parse_relay_event_message(&missing_created_at, "wss://gittr.ee").is_none());
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

    #[tokio::test]
    async fn process_event_message_returns_rejected_for_invalid_command() {
        let store = EventStore::default();
        let message = serde_json::json!([
            "EVENT",
            "gittree-dispatch",
            {
                "id": "11".repeat(32),
                "pubkey": "22".repeat(32),
                "kind": 1,
                "created_at": 321,
                "content": "gittree account nope",
                "tags": [["p", "npub1admin"]]
            }
        ])
        .to_string();
        let outcome = process_event_message(&store, &filter(), "wss://gittr.ee", &message)
            .await
            .expect("result");
        assert!(matches!(
            outcome,
            Some(DispatchEventOutcome::Rejected(message))
            if message.contains("invalid command")
        ));

        let invalid_event_id = serde_json::json!([
            "EVENT",
            "gittree-dispatch",
            {
                "id": "11",
                "pubkey": "22".repeat(32),
                "kind": 1,
                "created_at": 321,
                "content": "gittree account create",
                "tags": [["p", "npub1admin"]]
            }
        ])
        .to_string();
        let outcome = process_event_message(&store, &filter(), "wss://gittr.ee", &invalid_event_id)
            .await
            .expect("result");
        assert!(matches!(
            outcome,
            Some(DispatchEventOutcome::Rejected(message))
            if message.contains("invalid event id")
        ));
    }

    #[tokio::test]
    async fn process_event_message_applies_valid_event() {
        let store = EventStore::default();
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
        let outcome = process_event_message(&store, &filter(), "wss://gittr.ee", &message)
            .await
            .expect("result");
        assert!(matches!(
            outcome,
            Some(DispatchEventOutcome::Applied(output))
            if output.code == "account_created"
        ));
    }

    #[tokio::test]
    async fn event_store_trait_methods_cover_all_paths() {
        let store = EventStore::default();
        let actor = vec![2u8; 32];
        let repo_name = "demo".to_string();

        let log_record = CommandLogRecord {
            event_id: vec![1u8; 32],
            pubkey: actor.clone(),
            namespace: "account".to_string(),
            action: "create".to_string(),
            target: None,
            args_json: serde_json::json!({}),
            status: CommandStatus::Ok,
            code: "ok".to_string(),
            message: "ok".to_string(),
            created_at: 1,
        };
        assert!(store.insert_command_log(&log_record).await.expect("insert first"));
        assert!(!store.insert_command_log(&log_record).await.expect("insert duplicate"));
        store
            .update_command_log_outcome(&log_record.event_id, CommandStatus::Ok, "ok", "ok")
            .await
            .expect("update");

        let account = AccountStateRecord {
            pubkey: actor.clone(),
            status: gittree_storage::AccountLifecycle::Active,
            created_at: 1,
            updated_at: 2,
            deleted_at: None,
        };
        assert!(store.account_state(&actor).await.expect("account lookup").is_none());
        store
            .upsert_account_state(&account)
            .await
            .expect("upsert account");
        assert!(store.account_state(&actor).await.expect("account lookup").is_some());

        let profile = ProfileStateRecord {
            pubkey: actor.clone(),
            display_name: Some("alice".to_string()),
            bio: None,
            avatar_url: None,
            website_url: None,
            location: None,
            visibility: gittree_storage::ProfileVisibilityV1::Private,
            updated_at: 3,
        };
        assert!(store.profile_state(&actor).await.expect("profile lookup").is_none());
        store
            .upsert_profile_state(&profile)
            .await
            .expect("upsert profile");
        assert!(store.profile_state(&actor).await.expect("profile lookup").is_none());

        let repo = RepoStateV1Record {
            owner_pubkey: actor.clone(),
            repo_name: repo_name.clone(),
            description: None,
            website_url: None,
            visibility: gittree_storage::RepoVisibilityV1::Private,
            default_branch: "main".to_string(),
            archived: false,
            updated_at: 4,
        };
        assert!(
            store
                .repo_state(&actor, &repo_name)
                .await
                .expect("repo lookup")
                .is_none()
        );
        store.upsert_repo_state(&repo).await.expect("upsert repo");
        assert!(
            store
                .repo_state(&actor, &repo_name)
                .await
                .expect("repo lookup")
                .is_none()
        );

        let maintainer = RepoMaintainerV1Record {
            owner_pubkey: actor.clone(),
            repo_name: repo_name.clone(),
            maintainer_pubkey: actor.clone(),
            active: true,
            updated_at: 5,
        };
        store
            .set_repo_maintainer(&maintainer)
            .await
            .expect("set maintainer");
        assert!(
            store
                .list_active_repo_maintainers(&actor, &repo_name)
                .await
                .expect("maintainers")
                .is_empty()
        );
    }

    #[tokio::test]
    async fn serve_with_shutdown_covers_ok_and_error_paths() {
        let config = from_pairs(&base_env()).expect("config");
        let ok = serve_with_shutdown(config.clone(), async {
            tokio::time::sleep(Duration::from_millis(100)).await;
            Ok(())
        })
        .await;
        assert!(ok.is_ok());

        let err = serve_with_shutdown(
            config,
            async { Err(std::io::Error::other("shutdown failure")) },
        )
        .await
        .expect_err("shutdown error");
        assert!(matches!(
            err,
            DispatchError::Config(message)
            if message.contains("dispatch shutdown signal failed")
        ));
    }

    #[tokio::test]
    async fn serve_starts_and_waits_for_shutdown_signal() {
        let config = from_pairs(&base_env()).expect("config");
        let task = tokio::spawn(super::serve(config));
        tokio::time::sleep(Duration::from_millis(150)).await;
        task.abort();
        let join_error = task.await.expect_err("serve should be aborted");
        assert!(join_error.is_cancelled());
    }

    #[tokio::test]
    async fn run_relay_subscription_processes_text_ping_and_close() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let relay_addr = listener.local_addr().expect("addr");
        let relay_url = format!("ws://{relay_addr}");

        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept");
            let mut socket = accept_async(stream).await.expect("handshake");
            let req = socket.next().await.expect("req frame").expect("req");
            assert!(matches!(req, Message::Text(_)));
            let req_text = req.into_text().expect("req text");
            assert!(req_text.contains("\"REQ\""));
            assert!(req_text.contains("npub1admin"));

            let event_message = serde_json::json!([
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
            socket
                .send(Message::Text(event_message))
                .await
                .expect("send event");
            let payload = vec![1u8, 2, 3];
            socket
                .send(Message::Ping(payload.clone().into()))
                .await
                .expect("send ping");

            let next = tokio::time::timeout(Duration::from_secs(2), socket.next())
                .await
                .expect("pong timeout")
                .expect("pong frame")
                .expect("pong");
            assert!(matches!(next, Message::Pong(_)));
            let returned = next.into_data();
            assert_eq!(returned, payload);

            socket.send(Message::Close(None)).await.expect("send close");
        });

        let concrete_store = Arc::new(EventStore::default());
        let store: Arc<dyn CommandStore + Send + Sync> = concrete_store.clone();
        let filter = DispatchFilterConfig {
            admin_pubkey: "npub1admin".to_string(),
            relay_allowlist: vec![relay_url.clone()],
        };
        let relay_task = tokio::spawn(run_relay_subscription(
            Arc::clone(&store),
            filter,
            relay_url,
        ));

        server.await.expect("server task");
        tokio::time::sleep(Duration::from_millis(200)).await;
        relay_task.abort();
        let _ = relay_task.await;

        let actor = hex::decode("22".repeat(32)).expect("actor");
        assert!(concrete_store.account_state(&actor).await.expect("account").is_some());
    }

    #[tokio::test]
    async fn run_relay_subscription_handles_req_send_fail_and_non_text_messages() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let relay_addr = listener.local_addr().expect("addr");
        let relay_url = format!("ws://{relay_addr}");

        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept");
            let mut socket = accept_async(stream).await.expect("handshake");
            let _ = socket.next().await.expect("req frame").expect("req");
            socket
                .send(Message::Text(
                    serde_json::json!(["EVENT", "sub", {"id":"11".repeat(32),"pubkey":"22".repeat(32),"kind":7,"created_at":1,"content":"gittree account create","tags":[["p","npub1admin"]]}]).to_string(),
                ))
                .await
                .expect("send ignored");
            socket
                .send(Message::Text(
                    serde_json::json!(["EVENT", "sub", {"id":"11".repeat(32),"pubkey":"22".repeat(32),"kind":1,"created_at":1,"content":"gittree account nope","tags":[["p","npub1admin"]]}]).to_string(),
                ))
                .await
                .expect("send rejected");
            socket
                .send(Message::Binary(vec![1u8, 2, 3].into()))
                .await
                .expect("send binary");
            socket.send(Message::Close(None)).await.expect("close");
        });

        let store: Arc<dyn CommandStore + Send + Sync> = Arc::new(EventStore::default());
        let filter = DispatchFilterConfig {
            admin_pubkey: "npub1admin".to_string(),
            relay_allowlist: vec![relay_url.clone()],
        };
        let relay_task = tokio::spawn(run_relay_subscription(store, filter, relay_url));

        server.await.expect("server task");
        tokio::time::sleep(Duration::from_millis(200)).await;
        relay_task.abort();
        let _ = relay_task.await;
    }

    #[tokio::test]
    async fn run_relay_subscription_covers_text_none_and_processing_error() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let relay_addr = listener.local_addr().expect("addr");
        let relay_url = format!("ws://{relay_addr}");

        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept");
            let mut socket = accept_async(stream).await.expect("handshake");
            let _ = socket.next().await.expect("req frame").expect("req");
            socket
                .send(Message::Text("not-json".to_string()))
                .await
                .expect("send non-event text");
            socket
                .send(Message::Text(
                    serde_json::json!(["EVENT", "sub", {"id":"11".repeat(32),"pubkey":"22".repeat(32),"kind":1,"created_at":1,"content":"gittree account create","tags":[["p","npub1admin"]]}]).to_string(),
                ))
                .await
                .expect("send event");
            socket.send(Message::Close(None)).await.expect("close");
        });

        let store: Arc<dyn CommandStore + Send + Sync> = Arc::new(EventStore {
            fail_insert: true,
            ..Default::default()
        });
        let filter = DispatchFilterConfig {
            admin_pubkey: "npub1admin".to_string(),
            relay_allowlist: vec![relay_url.clone()],
        };
        let relay_task = tokio::spawn(run_relay_subscription(store, filter, relay_url));

        server.await.expect("server task");
        tokio::time::sleep(Duration::from_millis(200)).await;
        relay_task.abort();
        let _ = relay_task.await;
    }

    #[tokio::test]
    async fn run_relay_subscription_handles_req_send_failure() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let relay_addr = listener.local_addr().expect("addr");
        let relay_url = format!("ws://{relay_addr}");

        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept");
            let mut socket = accept_async(stream).await.expect("handshake");
            socket.send(Message::Close(None)).await.expect("close");
        });

        let store: Arc<dyn CommandStore + Send + Sync> = Arc::new(EventStore::default());
        let filter = DispatchFilterConfig {
            admin_pubkey: "npub1admin".to_string(),
            relay_allowlist: vec![relay_url.clone()],
        };
        let relay_task = tokio::spawn(run_relay_subscription(store, filter, relay_url));

        server.await.expect("server task");
        tokio::time::sleep(Duration::from_millis(200)).await;
        relay_task.abort();
        let _ = relay_task.await;
    }

    #[tokio::test]
    async fn process_relay_connection_covers_send_fail_pong_fail_and_reader_error() {
        let filter = DispatchFilterConfig {
            admin_pubkey: "npub1admin".to_string(),
            relay_allowlist: vec!["wss://gittr.ee".to_string()],
        };
        let relay_url = "wss://gittr.ee";

        let mut fail_first_send = ScriptedWriter {
            fail_send_on: Some(1),
            send_count: 0,
        };
        let mut empty_reader = futures_util::stream::iter(Vec::<Result<Message, WsError>>::new());
        process_relay_connection(
            &EventStore::default(),
            &filter,
            relay_url,
            &mut fail_first_send,
            &mut empty_reader,
        )
        .await;
        assert_eq!(fail_first_send.send_count, 1);

        let mut fail_second_send = ScriptedWriter {
            fail_send_on: Some(2),
            send_count: 0,
        };
        let mut ping_reader = futures_util::stream::iter(vec![Ok(Message::Ping(vec![1u8].into()))]);
        process_relay_connection(
            &EventStore::default(),
            &filter,
            relay_url,
            &mut fail_second_send,
            &mut ping_reader,
        )
        .await;
        assert_eq!(fail_second_send.send_count, 2);

        let mut ok_writer = ScriptedWriter::default();
        let mut err_reader = futures_util::stream::iter(vec![Err(WsError::Io(io::Error::other(
            "scripted read failure",
        )))]);
        process_relay_connection(
            &EventStore::default(),
            &filter,
            relay_url,
            &mut ok_writer,
            &mut err_reader,
        )
        .await;
        assert_eq!(ok_writer.send_count, 1);
        futures_util::SinkExt::close(&mut ok_writer)
            .await
            .expect("close writer");

        let mut scripted_writer = ScriptedWriter::default();
        let valid_event = serde_json::json!([
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
        let invalid_event = serde_json::json!([
            "EVENT",
            "gittree-dispatch",
            {
                "id": "11".repeat(32),
                "pubkey": "22".repeat(32),
                "kind": 1,
                "created_at": 321,
                "content": "gittree account nope",
                "tags": [["p", "npub1admin"]]
            }
        ])
        .to_string();
        let mut text_and_non_text_reader = futures_util::stream::iter(vec![
            Ok(Message::Text("not-json".to_string())),
            Ok(Message::Text(valid_event)),
            Ok(Message::Text(invalid_event)),
            Ok(Message::Binary(vec![0x01].into())),
            Ok(Message::Ping(vec![0x02].into())),
            Ok(Message::Close(None)),
        ]);
        process_relay_connection(
            &EventStore::default(),
            &filter,
            relay_url,
            &mut scripted_writer,
            &mut text_and_non_text_reader,
        )
        .await;
        assert!(scripted_writer.send_count >= 2);
    }

    #[tokio::test]
    async fn run_relay_subscription_connect_error_loop_is_abortable() {
        let store: Arc<dyn CommandStore + Send + Sync> = Arc::new(EventStore::default());
        let filter = DispatchFilterConfig {
            admin_pubkey: "npub1admin".to_string(),
            relay_allowlist: vec!["ws://127.0.0.1:1".to_string()],
        };
        let relay_task = tokio::spawn(run_relay_subscription(
            store,
            filter,
            "ws://127.0.0.1:1".to_string(),
        ));

        tokio::time::sleep(Duration::from_millis(200)).await;
        relay_task.abort();
        let join_error = relay_task.await.expect_err("relay should be aborted");
        assert!(join_error.is_cancelled());
    }

    #[test]
    fn dispatch_error_display_and_source_cover_variants() {
        let config = DispatchError::Config("bad config".to_string());
        assert_eq!(config.to_string(), "dispatch config error: bad config");
        assert!(std::error::Error::source(&config).is_none());

        let storage = DispatchError::Storage(gittree_storage::StorageError::Internal {
            message: "boom".to_string(),
        });
        assert_eq!(
            storage.to_string(),
            "dispatch storage error: internal error: boom"
        );
        assert!(std::error::Error::source(&storage).is_some());

        let observability_config = DispatchError::ObservabilityConfig(
            gittree_observability::ObservabilityConfigError::InvalidEnv {
                key: "GITTREE_LOG_JSON",
                value: "nope".to_string(),
            },
        );
        assert!(
            observability_config
                .to_string()
                .contains("dispatch observability config error:")
        );
        assert!(std::error::Error::source(&observability_config).is_some());

        let observability = DispatchError::Observability(
            gittree_observability::ObservabilityError::LogInit("cannot open".to_string()),
        );
        assert!(
            observability
                .to_string()
                .contains("dispatch observability error:")
        );
        assert!(std::error::Error::source(&observability).is_some());
    }
}
