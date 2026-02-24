use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use gittree_config::{ConfigError, ForgejoConfig, RelayTargetsConfig, ServicesConfig};
use gittree_core::{
    CoreError, Nip34Event, RepoAnnouncement, RepoMapping, extract_npub, parse_repo_path,
};
use gittree_forgejo::{ForgejoClient, ForgejoError, ForgejoTransport};
use gittree_observability::{ObservabilityConfigError, ObservabilityError, ObservabilityHandle};
use gittree_relay_adapter::{
    RelayAdapter, RelayAdapterConfig, SignedNostrEvent, WebsocketRelayAdapter,
};
use gittree_storage::{
    AnnouncementRepository, PostgresRepositories, RelayPublishJob, RelayPublishRepository,
    RepoAnnouncementRecord, RepoMappingRecord, RepoMappingRepository, StorageConfig, StorageError,
};
use serde::{Deserialize, Serialize};
use std::future::{Future, pending};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::Duration as StdDuration;
use time::{Duration as TimeDuration, OffsetDateTime};

const ENV_STORAGE_READ_URL: &str = "GITTREE_STORAGE_READ_URL";
const ENV_STORAGE_WRITE_URL: &str = "GITTREE_STORAGE_WRITE_URL";
const ENV_STORAGE_MAX_CONNECTIONS: &str = "GITTREE_STORAGE_MAX_CONNECTIONS";
const ENV_STORAGE_MIN_CONNECTIONS: &str = "GITTREE_STORAGE_MIN_CONNECTIONS";
const ENV_STORAGE_IDLE_TIMEOUT_SECS: &str = "GITTREE_STORAGE_IDLE_TIMEOUT_SECS";
const ENV_STORAGE_MAX_LIFETIME_SECS: &str = "GITTREE_STORAGE_MAX_LIFETIME_SECS";
const ENV_STORAGE_APP_NAME: &str = "GITTREE_STORAGE_APP_NAME";
const ENV_COORDINATOR_REPO_ROOT: &str = "GITTREE_COORDINATOR_REPO_ROOT";
const ENV_COORDINATOR_PRE_RECEIVE_HOOK: &str = "GITTREE_COORDINATOR_PRE_RECEIVE_HOOK";
const ENV_COORDINATOR_POST_RECEIVE_HOOK: &str = "GITTREE_COORDINATOR_POST_RECEIVE_HOOK";
const OUTBOX_POLL_SECS: u64 = 2;
const OUTBOX_RETRY_BASE_SECS: i64 = 30;
const OUTBOX_RETRY_MAX_SECS: i64 = 30 * 60;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoordinatorConfig {
    pub bind: String,
    pub storage: StorageConfig,
    pub relay_urls: Vec<String>,
    pub repo_root: PathBuf,
    pub hooks: HookInstallConfig,
    pub forgejo: ForgejoConfig,
}

impl CoordinatorConfig {
    pub fn from_env() -> Result<Self, CoordinatorConfigError> {
        let services =
            ServicesConfig::from_env_validated().map_err(CoordinatorConfigError::Config)?;
        let storage = storage_from_env()?;
        let relay_targets =
            RelayTargetsConfig::from_env_validated().map_err(CoordinatorConfigError::Config)?;
        let repo_root = env_path(ENV_COORDINATOR_REPO_ROOT)?;
        let hooks = HookInstallConfig {
            pre_receive_source: env_path(ENV_COORDINATOR_PRE_RECEIVE_HOOK)?,
            post_receive_source: env_path(ENV_COORDINATOR_POST_RECEIVE_HOOK)?,
        };
        let forgejo = ForgejoConfig::from_env().map_err(CoordinatorConfigError::Config)?;
        Ok(Self {
            bind: services.coordinator.bind,
            storage,
            relay_urls: relay_targets.relay_urls,
            repo_root,
            hooks,
            forgejo,
        })
    }
}

#[derive(Debug)]
pub enum CoordinatorConfigError {
    Config(ConfigError),
    Storage(StorageConfigError),
    MissingEnv(&'static str),
    InvalidEnv { key: &'static str, value: String },
}

impl std::fmt::Display for CoordinatorConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CoordinatorConfigError::Config(err) => write!(f, "coordinator config error: {err}"),
            CoordinatorConfigError::Storage(err) => {
                write!(f, "coordinator storage config error: {err}")
            }
            CoordinatorConfigError::MissingEnv(key) => write!(f, "missing env {key}"),
            CoordinatorConfigError::InvalidEnv { key, value } => {
                write!(f, "invalid env {key}: {value}")
            }
        }
    }
}

impl std::error::Error for CoordinatorConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            CoordinatorConfigError::Config(err) => Some(err),
            CoordinatorConfigError::Storage(err) => Some(err),
            CoordinatorConfigError::MissingEnv(_) => None,
            CoordinatorConfigError::InvalidEnv { .. } => None,
        }
    }
}

#[derive(Debug)]
pub enum StorageConfigError {
    MissingEnv(&'static str),
    InvalidEnv { key: &'static str, value: String },
    InvalidConfig(String),
}

impl std::fmt::Display for StorageConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StorageConfigError::MissingEnv(key) => write!(f, "missing env {key}"),
            StorageConfigError::InvalidEnv { key, value } => {
                write!(f, "invalid env {key}: {value}")
            }
            StorageConfigError::InvalidConfig(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for StorageConfigError {}

fn storage_from_env() -> Result<StorageConfig, CoordinatorConfigError> {
    let read_connection = std::env::var(ENV_STORAGE_READ_URL).map_err(|_| {
        CoordinatorConfigError::Storage(StorageConfigError::MissingEnv(ENV_STORAGE_READ_URL))
    })?;
    let write_connection = std::env::var(ENV_STORAGE_WRITE_URL).ok();
    let max_connections = env_u32(ENV_STORAGE_MAX_CONNECTIONS)?.unwrap_or(10);
    let min_connections = env_u32(ENV_STORAGE_MIN_CONNECTIONS)?.unwrap_or(2);
    let idle_timeout_secs = env_u64(ENV_STORAGE_IDLE_TIMEOUT_SECS)?;
    let max_lifetime_secs = env_u64(ENV_STORAGE_MAX_LIFETIME_SECS)?;
    let application_name = std::env::var(ENV_STORAGE_APP_NAME).ok();

    let config = StorageConfig {
        read_connection,
        write_connection,
        max_connections,
        min_connections,
        idle_timeout_secs,
        max_lifetime_secs,
        application_name,
    };

    config.validate().map_err(|err| {
        CoordinatorConfigError::Storage(StorageConfigError::InvalidConfig(err.to_string()))
    })?;

    Ok(config)
}

fn env_u32(key: &'static str) -> Result<Option<u32>, CoordinatorConfigError> {
    match std::env::var(key) {
        Ok(value) => {
            if value.trim().is_empty() {
                return Ok(None);
            }
            value.parse::<u32>().map(Some).map_err(|_| {
                CoordinatorConfigError::Storage(StorageConfigError::InvalidEnv { key, value })
            })
        }
        Err(_) => Ok(None),
    }
}

fn env_u64(key: &'static str) -> Result<Option<u64>, CoordinatorConfigError> {
    match std::env::var(key) {
        Ok(value) => {
            if value.trim().is_empty() {
                return Ok(None);
            }
            value.parse::<u64>().map(Some).map_err(|_| {
                CoordinatorConfigError::Storage(StorageConfigError::InvalidEnv { key, value })
            })
        }
        Err(_) => Ok(None),
    }
}

fn env_path(key: &'static str) -> Result<PathBuf, CoordinatorConfigError> {
    let value = std::env::var(key).map_err(|_| CoordinatorConfigError::MissingEnv(key))?;
    if value.trim().is_empty() {
        return Err(CoordinatorConfigError::InvalidEnv { key, value });
    }
    Ok(PathBuf::from(value))
}

#[derive(Debug)]
pub enum CoordinatorError {
    Config(CoordinatorConfigError),
    ObservabilityConfig(ObservabilityConfigError),
    Observability(ObservabilityError),
    Storage(StorageError),
    Forgejo(ForgejoError),
    Serve(String),
}

impl std::fmt::Display for CoordinatorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CoordinatorError::Config(err) => write!(f, "coordinator error: {err}"),
            CoordinatorError::ObservabilityConfig(err) => {
                write!(f, "coordinator observability config error: {err}")
            }
            CoordinatorError::Observability(err) => {
                write!(f, "coordinator observability error: {err}")
            }
            CoordinatorError::Storage(err) => write!(f, "coordinator storage error: {err}"),
            CoordinatorError::Forgejo(err) => write!(f, "coordinator forgejo error: {err}"),
            CoordinatorError::Serve(err) => write!(f, "coordinator serve error: {err}"),
        }
    }
}

impl std::error::Error for CoordinatorError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            CoordinatorError::Config(err) => Some(err),
            CoordinatorError::ObservabilityConfig(err) => Some(err),
            CoordinatorError::Observability(err) => Some(err),
            CoordinatorError::Storage(err) => Some(err),
            CoordinatorError::Forgejo(err) => Some(err),
            CoordinatorError::Serve(_) => None,
        }
    }
}

pub fn init_observability() -> Result<ObservabilityHandle, CoordinatorError> {
    let config = gittree_observability::ObservabilityConfig::from_env("gittree-coordinator")
        .map_err(CoordinatorError::ObservabilityConfig)?;
    let handle = gittree_observability::init(&config).map_err(CoordinatorError::Observability)?;
    Ok(handle)
}

pub fn build_repositories(
    config: &CoordinatorConfig,
) -> Result<PostgresRepositories, CoordinatorError> {
    let pool_options = config
        .storage
        .pool_options()
        .map_err(CoordinatorError::Storage)?;
    let connect_options = config
        .storage
        .read_connect_options()
        .map_err(CoordinatorError::Storage)?;
    let pool = pool_options.connect_lazy_with(connect_options);
    Ok(PostgresRepositories::new(pool))
}

struct CoordinatorAppState<R, T> {
    repositories: Arc<R>,
    repo_root: PathBuf,
    hooks: HookInstallConfig,
    forgejo: ForgejoClient<T>,
}

impl<R, T> Clone for CoordinatorAppState<R, T>
where
    T: Clone,
{
    fn clone(&self) -> Self {
        Self {
            repositories: Arc::clone(&self.repositories),
            repo_root: self.repo_root.clone(),
            hooks: self.hooks.clone(),
            forgejo: self.forgejo.clone(),
        }
    }
}

pub async fn serve(config: CoordinatorConfig) -> Result<(), CoordinatorError> {
    serve_with_init(config, init_observability).await
}

async fn serve_with_init<I, O>(config: CoordinatorConfig, init: I) -> Result<(), CoordinatorError>
where
    I: FnOnce() -> Result<O, CoordinatorError>,
{
    let _observability = init()?;
    let repositories = build_repositories(&config)?;
    let forgejo = ForgejoClient::new(config.forgejo).map_err(CoordinatorError::Forgejo)?;
    let state = CoordinatorAppState {
        repositories: Arc::new(repositories),
        repo_root: config.repo_root,
        hooks: config.hooks,
        forgejo,
    };
    let publisher_state = state.clone();
    spawn_publish_outbox(publisher_state);
    let router = build_router(state);
    let listener = tokio::net::TcpListener::bind(&config.bind)
        .await
        .map_err(|err| CoordinatorError::Serve(err.to_string()))?;
    run_http_server_with_shutdown(listener, router, pending()).await
}

fn spawn_publish_outbox<R, T>(state: CoordinatorAppState<R, T>) -> tokio::task::JoinHandle<()>
where
    R: RelayPublishRepository
        + AnnouncementRepository
        + RepoMappingRepository
        + Send
        + Sync
        + 'static,
    T: ForgejoTransport + Clone + Send + Sync + 'static,
{
    tokio::spawn(publish_outbox_loop_with_delay_and_publish(
        state,
        StdDuration::from_secs(OUTBOX_POLL_SECS),
        publish_to_relay,
    ))
}

async fn run_http_server_with_shutdown<Shutdown>(
    listener: tokio::net::TcpListener,
    router: Router,
    shutdown: Shutdown,
) -> Result<(), CoordinatorError>
where
    Shutdown: Future<Output = ()> + Send + 'static,
{
    if let Err(err) = axum::serve(listener, router)
        .with_graceful_shutdown(shutdown)
        .await
    {
        return Err(CoordinatorError::Serve(err.to_string()));
    }
    Ok(())
}

fn build_router<R, T>(state: CoordinatorAppState<R, T>) -> Router
where
    R: AnnouncementRepository + RepoMappingRepository + Send + Sync + 'static,
    T: ForgejoTransport + Clone + Send + Sync + 'static,
{
    Router::new()
        .route("/health", get(health_handler))
        .route("/announcement", post(announcement_handler))
        .with_state(state)
}

async fn health_handler() -> &'static str {
    "ok"
}

async fn publish_to_relay(relay_url: String, event: SignedNostrEvent) -> Result<(), String> {
    let adapter = WebsocketRelayAdapter::new(RelayAdapterConfig::new(relay_url));
    adapter
        .publish_event(&event)
        .await
        .map_err(|err| err.to_string())
}

async fn publish_outbox_loop_with_delay_and_publish<R, T, Publish, PublishFut>(
    state: CoordinatorAppState<R, T>,
    poll_delay: StdDuration,
    publish: Publish,
) where
    R: RelayPublishRepository
        + AnnouncementRepository
        + RepoMappingRepository
        + Send
        + Sync
        + 'static,
    T: ForgejoTransport + Clone + Send + Sync + 'static,
    Publish: Fn(String, SignedNostrEvent) -> PublishFut + Send + Sync + 'static,
    PublishFut: Future<Output = Result<(), String>> + Send,
{
    loop {
        let now = OffsetDateTime::now_utc();
        let job = match state.repositories.claim_relay_publish(now).await {
            Ok(job) => job,
            Err(err) => {
                tracing::error!(error = %err, "outbox claim failed");
                tokio::time::sleep(poll_delay).await;
                continue;
            }
        };

        let Some(job) = job else {
            tokio::time::sleep(poll_delay).await;
            continue;
        };

        let event = signed_event_from_job(&job);
        match publish(job.relay_url.clone(), event).await {
            Ok(()) => {
                if let Err(err) = state
                    .repositories
                    .mark_relay_publish_succeeded(job.id)
                    .await
                {
                    tracing::error!(error = %err, "outbox mark succeeded failed");
                    continue;
                }
                match state
                    .repositories
                    .pending_relay_publishes(&job.pubkey, &job.identifier, job.kind)
                    .await
                {
                    Ok(0) => {
                        let _ = finalize_outbox_job(&state, &job)
                            .await
                            .map_err(|err| tracing::error!(error = %err, "outbox finalize failed"));
                    }
                    Ok(_) => {}
                    Err(err) => {
                        tracing::error!(error = %err, "outbox pending count failed");
                    }
                }
            }
            Err(err) => {
                let retry_at = retry_after(now, job.attempt_count);
                if let Err(storage_err) = state
                    .repositories
                    .mark_relay_publish_failed(job.id, &err, retry_at)
                    .await
                {
                    tracing::error!(error = %storage_err, "outbox mark failed failed");
                }
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoordinatorEventPayload {
    pub kind: u64,
    pub event_id: String,
    pub pubkey: String,
    pub created_at: i64,
    pub tags: Vec<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum CoordinatorActionPayload {
    Provisioned { repo_path: String },
    SkippedExisting { repo_path: String },
    Ignored,
}

impl From<CoordinatorAction> for CoordinatorActionPayload {
    fn from(action: CoordinatorAction) -> Self {
        match action {
            CoordinatorAction::Provisioned { repo_path } => CoordinatorActionPayload::Provisioned {
                repo_path: repo_path.display().to_string(),
            },
            CoordinatorAction::SkippedExisting { repo_path } => {
                CoordinatorActionPayload::SkippedExisting {
                    repo_path: repo_path.display().to_string(),
                }
            }
            CoordinatorAction::Ignored => CoordinatorActionPayload::Ignored,
        }
    }
}

#[derive(Debug)]
enum CoordinatorHttpError {
    BadRequest(String),
    Internal(String),
}

impl From<CoordinatorEventError> for CoordinatorHttpError {
    fn from(err: CoordinatorEventError) -> Self {
        match err {
            CoordinatorEventError::Parse(message) => CoordinatorHttpError::BadRequest(message),
            CoordinatorEventError::MissingNpub => {
                CoordinatorHttpError::BadRequest("missing npub".to_string())
            }
            CoordinatorEventError::Plan(err) => CoordinatorHttpError::Internal(err.to_string()),
            CoordinatorEventError::Init(err) => CoordinatorHttpError::Internal(err.to_string()),
            CoordinatorEventError::Hooks(err) => CoordinatorHttpError::Internal(err.to_string()),
            CoordinatorEventError::Storage(err) => CoordinatorHttpError::Internal(err.to_string()),
            CoordinatorEventError::Forgejo(err) => CoordinatorHttpError::Internal(err.to_string()),
            CoordinatorEventError::Mapping(err) => CoordinatorHttpError::Internal(err.to_string()),
        }
    }
}

impl IntoResponse for CoordinatorHttpError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            CoordinatorHttpError::BadRequest(message) => (StatusCode::BAD_REQUEST, message),
            CoordinatorHttpError::Internal(message) => (StatusCode::INTERNAL_SERVER_ERROR, message),
        };
        (status, message).into_response()
    }
}

async fn announcement_handler<R, T>(
    State(state): State<CoordinatorAppState<R, T>>,
    Json(payload): Json<CoordinatorEventPayload>,
) -> Result<Json<CoordinatorActionPayload>, CoordinatorHttpError>
where
    R: AnnouncementRepository + RepoMappingRepository + Send + Sync,
    T: ForgejoTransport + Clone + Send + Sync,
{
    let kind = u32::try_from(payload.kind)
        .map_err(|_| CoordinatorHttpError::BadRequest("invalid kind".to_string()))?;
    let event = RelayEvent {
        kind,
        event_id: payload.event_id,
        pubkey: payload.pubkey,
        created_at: payload.created_at,
        tags: payload.tags,
    };
    let action = handle_announcement_event_with_storage(
        &state.repo_root,
        &state.hooks,
        state.repositories.as_ref(),
        &state.forgejo,
        &event,
    )
    .await
    .map_err(CoordinatorHttpError::from)?;
    Ok(Json(action.into()))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoProvisionPlan {
    pub npub: String,
    pub identifier: String,
    pub repo_path: PathBuf,
    pub hooks_dir: PathBuf,
    pub pre_receive_hook: PathBuf,
    pub post_receive_hook: PathBuf,
    pub git_config: Vec<GitConfigEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitConfigEntry {
    pub key: String,
    pub value: String,
}

impl GitConfigEntry {
    pub fn new(key: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            value: value.into(),
        }
    }
}

#[derive(Debug)]
pub enum ProvisionPlanError {
    InvalidRepo(String),
}

impl std::fmt::Display for ProvisionPlanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProvisionPlanError::InvalidRepo(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for ProvisionPlanError {}

pub fn build_provision_plan(
    root: impl AsRef<Path>,
    npub: &str,
    announcement: &RepoAnnouncement,
) -> Result<RepoProvisionPlan, ProvisionPlanError> {
    let repo_path = root
        .as_ref()
        .join(npub)
        .join(format!("{}.git", announcement.identifier));
    if let Err(err) = parse_repo_path(&repo_path) {
        return Err(ProvisionPlanError::InvalidRepo(err.to_string()));
    }
    let hooks_dir = repo_path.join("hooks");
    let pre_receive_hook = hooks_dir.join("pre-receive");
    let post_receive_hook = hooks_dir.join("post-receive");
    Ok(RepoProvisionPlan {
        npub: npub.to_string(),
        identifier: announcement.identifier.clone(),
        repo_path,
        hooks_dir,
        pre_receive_hook,
        post_receive_hook,
        git_config: default_repo_config(),
    })
}

fn default_repo_config() -> Vec<GitConfigEntry> {
    vec![
        GitConfigEntry::new("core.bare", "true"),
        GitConfigEntry::new("receive.advertisePushOptions", "true"),
    ]
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoInitReport {
    pub created: bool,
    pub configured: usize,
}

#[derive(Debug)]
pub enum RepoInitError {
    Io(std::io::Error),
    InvalidRepo(String),
    InvalidPath(String),
    Git(String),
}

impl std::fmt::Display for RepoInitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RepoInitError::Io(err) => write!(f, "repo init io error: {err}"),
            RepoInitError::InvalidRepo(message) => write!(f, "{message}"),
            RepoInitError::InvalidPath(message) => write!(f, "{message}"),
            RepoInitError::Git(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for RepoInitError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            RepoInitError::Io(err) => Some(err),
            RepoInitError::InvalidRepo(_) => None,
            RepoInitError::InvalidPath(_) => None,
            RepoInitError::Git(_) => None,
        }
    }
}

pub fn init_repo(plan: &RepoProvisionPlan) -> Result<RepoInitReport, RepoInitError> {
    let mut created = false;
    if plan.repo_path.exists() {
        if !plan.repo_path.is_dir() {
            return Err(RepoInitError::InvalidRepo(format!(
                "repo path is not a directory: {}",
                plan.repo_path.display()
            )));
        }
        let head = plan.repo_path.join("HEAD");
        if !head.exists() {
            return Err(RepoInitError::InvalidRepo(format!(
                "repo path missing HEAD: {}",
                plan.repo_path.display()
            )));
        }
    } else {
        create_bare_repo(&plan.repo_path)?;
        created = true;
    }

    for entry in &plan.git_config {
        apply_git_config(&plan.repo_path, entry)?;
    }

    Ok(RepoInitReport {
        created,
        configured: plan.git_config.len(),
    })
}

fn create_bare_repo(path: &Path) -> Result<(), RepoInitError> {
    let path_str = path
        .to_str()
        .ok_or_else(|| RepoInitError::InvalidPath("repo path is not utf-8".to_string()))?;
    let output = Command::new("git")
        .arg("init")
        .arg("--bare")
        .arg(path_str)
        .output()
        .map_err(RepoInitError::Io)?;
    if !output.status.success() {
        return Err(RepoInitError::Git(format!(
            "git init failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    Ok(())
}

fn apply_git_config(path: &Path, entry: &GitConfigEntry) -> Result<(), RepoInitError> {
    let path_str = path
        .to_str()
        .ok_or_else(|| RepoInitError::InvalidPath("repo path is not utf-8".to_string()))?;
    let output = Command::new("git")
        .arg("-C")
        .arg(path_str)
        .arg("config")
        .arg(&entry.key)
        .arg(&entry.value)
        .output()
        .map_err(RepoInitError::Io)?;
    if !output.status.success() {
        return Err(RepoInitError::Git(format!(
            "git config failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookInstallConfig {
    pub pre_receive_source: PathBuf,
    pub post_receive_source: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookInstallReport {
    pub installed: usize,
}

#[derive(Debug)]
pub enum HookInstallError {
    Io(std::io::Error),
    MissingSource(String),
}

impl std::fmt::Display for HookInstallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HookInstallError::Io(err) => write!(f, "hook install io error: {err}"),
            HookInstallError::MissingSource(path) => write!(f, "missing hook source: {path}"),
        }
    }
}

impl std::error::Error for HookInstallError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            HookInstallError::Io(err) => Some(err),
            HookInstallError::MissingSource(_) => None,
        }
    }
}

pub fn install_hooks(
    plan: &RepoProvisionPlan,
    config: &HookInstallConfig,
) -> Result<HookInstallReport, HookInstallError> {
    ensure_source_exists(&config.pre_receive_source)?;
    ensure_source_exists(&config.post_receive_source)?;

    std::fs::create_dir_all(&plan.hooks_dir).map_err(HookInstallError::Io)?;
    install_hook(&config.pre_receive_source, &plan.pre_receive_hook)?;
    install_hook(&config.post_receive_source, &plan.post_receive_hook)?;

    Ok(HookInstallReport { installed: 2 })
}

fn ensure_source_exists(path: &Path) -> Result<(), HookInstallError> {
    if path.exists() {
        Ok(())
    } else {
        Err(HookInstallError::MissingSource(path.display().to_string()))
    }
}

fn install_hook(source: &Path, destination: &Path) -> Result<(), HookInstallError> {
    std::fs::copy(source, destination).map_err(HookInstallError::Io)?;
    ensure_executable(destination)?;
    Ok(())
}

fn ensure_executable(path: &Path) -> Result<(), HookInstallError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = std::fs::metadata(path)
            .map_err(HookInstallError::Io)?
            .permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(path, permissions).map_err(HookInstallError::Io)?;
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayEvent {
    pub kind: u32,
    pub event_id: String,
    pub pubkey: String,
    pub created_at: i64,
    pub tags: Vec<Vec<String>>,
}

fn signed_event_from_job(job: &RelayPublishJob) -> SignedNostrEvent {
    SignedNostrEvent {
        id: hex::encode(&job.event_id),
        pubkey: hex::encode(&job.pubkey),
        created_at: job.created_at,
        kind: job.kind,
        tags: job.tags.clone(),
        content: job.content.clone(),
        sig: hex::encode(&job.sig),
    }
}

fn relay_event_from_job(job: &RelayPublishJob) -> RelayEvent {
    RelayEvent {
        kind: job.kind,
        event_id: hex::encode(&job.event_id),
        pubkey: hex::encode(&job.pubkey),
        created_at: job.created_at,
        tags: job.tags.clone(),
    }
}

fn retry_after(now: OffsetDateTime, attempt_count: i32) -> OffsetDateTime {
    let attempt = i64::from(attempt_count.max(1));
    let delay = OUTBOX_RETRY_BASE_SECS
        .saturating_mul(attempt)
        .min(OUTBOX_RETRY_MAX_SECS);
    now + TimeDuration::seconds(delay)
}

async fn finalize_outbox_job<R, T>(
    state: &CoordinatorAppState<R, T>,
    job: &RelayPublishJob,
) -> Result<(), CoordinatorEventError>
where
    R: AnnouncementRepository + RepoMappingRepository,
    T: ForgejoTransport,
{
    let event = relay_event_from_job(job);
    let parsed = Nip34Event::parse_validated(event.kind, &event.tags)
        .map_err(|err| CoordinatorEventError::Parse(err.to_string()))?;
    let Nip34Event::RepoAnnouncement(announcement) = parsed else {
        return Ok(());
    };
    let record = RepoAnnouncementRecord::new(
        &event.event_id,
        &event.pubkey,
        event.created_at,
        &announcement,
    )
    .map_err(CoordinatorEventError::Storage)?;
    state
        .repositories
        .insert_announcement(record)
        .await
        .map_err(CoordinatorEventError::Storage)?;
    let repo = state
        .forgejo
        .ensure_repo_for_owner(
            &job.forgejo_owner,
            &job.forgejo_repo,
            announcement.description.as_deref(),
        )
        .await
        .map_err(CoordinatorEventError::Forgejo)?;
    state
        .forgejo
        .ensure_webhook_for_owner(&job.forgejo_owner, &repo.name)
        .await
        .map_err(CoordinatorEventError::Forgejo)?;
    let mapping = RepoMapping::new(
        repo.owner,
        repo.name,
        event.pubkey.clone(),
        announcement.identifier.clone(),
    )
    .map_err(CoordinatorEventError::Mapping)?;
    let record = RepoMappingRecord::new(&mapping).map_err(CoordinatorEventError::Storage)?;
    state
        .repositories
        .upsert_mapping(record)
        .await
        .map_err(CoordinatorEventError::Storage)?;
    let _ = handle_announcement_event(&state.repo_root, &state.hooks, &event)?;
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoordinatorAction {
    Provisioned { repo_path: PathBuf },
    SkippedExisting { repo_path: PathBuf },
    Ignored,
}

#[derive(Debug)]
pub enum CoordinatorEventError {
    Parse(String),
    MissingNpub,
    Plan(ProvisionPlanError),
    Init(RepoInitError),
    Hooks(HookInstallError),
    Storage(StorageError),
    Forgejo(ForgejoError),
    Mapping(CoreError),
}

impl std::fmt::Display for CoordinatorEventError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CoordinatorEventError::Parse(message) => write!(f, "{message}"),
            CoordinatorEventError::MissingNpub => write!(f, "missing npub in clone urls"),
            CoordinatorEventError::Plan(err) => write!(f, "{err}"),
            CoordinatorEventError::Init(err) => write!(f, "{err}"),
            CoordinatorEventError::Hooks(err) => write!(f, "{err}"),
            CoordinatorEventError::Storage(err) => write!(f, "storage error: {err}"),
            CoordinatorEventError::Forgejo(err) => write!(f, "{err}"),
            CoordinatorEventError::Mapping(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for CoordinatorEventError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            CoordinatorEventError::Parse(_) => None,
            CoordinatorEventError::MissingNpub => None,
            CoordinatorEventError::Plan(err) => Some(err),
            CoordinatorEventError::Init(err) => Some(err),
            CoordinatorEventError::Hooks(err) => Some(err),
            CoordinatorEventError::Storage(err) => Some(err),
            CoordinatorEventError::Forgejo(err) => Some(err),
            CoordinatorEventError::Mapping(err) => Some(err),
        }
    }
}

pub fn handle_announcement_event(
    root: impl AsRef<Path>,
    hooks: &HookInstallConfig,
    event: &RelayEvent,
) -> Result<CoordinatorAction, CoordinatorEventError> {
    let parsed = Nip34Event::parse_validated(event.kind, &event.tags)
        .map_err(|err| CoordinatorEventError::Parse(err.to_string()))?;
    let Nip34Event::RepoAnnouncement(announcement) = parsed else {
        return Ok(CoordinatorAction::Ignored);
    };

    let npub = announcement
        .clone
        .iter()
        .find_map(|url| extract_npub(url).ok().map(|value| value.to_string()))
        .ok_or(CoordinatorEventError::MissingNpub)?;
    let plan =
        build_provision_plan(root, &npub, &announcement).map_err(CoordinatorEventError::Plan)?;

    if plan.repo_path.exists() {
        return Ok(CoordinatorAction::SkippedExisting {
            repo_path: plan.repo_path,
        });
    }

    init_repo(&plan).map_err(CoordinatorEventError::Init)?;
    install_hooks(&plan, hooks).map_err(CoordinatorEventError::Hooks)?;
    Ok(CoordinatorAction::Provisioned {
        repo_path: plan.repo_path,
    })
}

fn forgejo_repo_name(identifier: &str, pubkey: &str) -> String {
    let suffix = pubkey.get(0..8).unwrap_or(pubkey);
    format!("{identifier}--{suffix}")
}

pub async fn handle_announcement_event_with_storage<S, T>(
    root: impl AsRef<Path>,
    hooks: &HookInstallConfig,
    storage: &S,
    forgejo: &ForgejoClient<T>,
    event: &RelayEvent,
) -> Result<CoordinatorAction, CoordinatorEventError>
where
    S: AnnouncementRepository + RepoMappingRepository,
    T: ForgejoTransport,
{
    let parsed = Nip34Event::parse_validated(event.kind, &event.tags)
        .map_err(|err| CoordinatorEventError::Parse(err.to_string()))?;
    let Nip34Event::RepoAnnouncement(announcement) = parsed else {
        return Ok(CoordinatorAction::Ignored);
    };
    let record = RepoAnnouncementRecord::new(
        &event.event_id,
        &event.pubkey,
        event.created_at,
        &announcement,
    )
    .map_err(CoordinatorEventError::Storage)?;
    storage
        .insert_announcement(record)
        .await
        .map_err(CoordinatorEventError::Storage)?;
    let forgejo_name = forgejo_repo_name(&announcement.identifier, &event.pubkey);
    let repo = forgejo
        .ensure_repo(&forgejo_name, announcement.description.as_deref())
        .await
        .map_err(CoordinatorEventError::Forgejo)?;
    forgejo
        .ensure_webhook(&repo.name)
        .await
        .map_err(CoordinatorEventError::Forgejo)?;
    let mapping = RepoMapping::new(
        repo.owner,
        repo.name,
        event.pubkey.clone(),
        announcement.identifier.clone(),
    )
    .map_err(CoordinatorEventError::Mapping)?;
    let record = RepoMappingRecord::new(&mapping).map_err(CoordinatorEventError::Storage)?;
    storage
        .upsert_mapping(record)
        .await
        .map_err(CoordinatorEventError::Storage)?;
    handle_announcement_event(root, hooks, event)
}

#[cfg(test)]
mod tests {
    use super::CoordinatorAction;
    use super::CoordinatorActionPayload;
    use super::CoordinatorConfig;
    use super::CoordinatorEventPayload;
    use super::ENV_STORAGE_READ_URL;
    use super::HookInstallConfig;
    use super::ObservabilityHandle;
    use super::RelayEvent;
    use super::RepoAnnouncement;
    use super::build_provision_plan;
    use super::handle_announcement_event;
    use super::handle_announcement_event_with_storage;
    use super::init_observability;
    use super::init_repo;
    use super::install_hooks;
    use async_trait::async_trait;
    use axum::Router;
    use axum::body::{Body, to_bytes};
    use axum::http::Request;
    use axum::response::IntoResponse;
    use gittree_config::ConfigError;
    use gittree_config::ForgejoConfig;
    use gittree_core::UserGraspList;
    use gittree_core::kinds::{KIND_GIT_REPO_ANNOUNCEMENT, KIND_USER_GRASP_LIST};
    use gittree_forgejo::{
        ForgejoClient, ForgejoMethod, ForgejoRequest, ForgejoResponse, ForgejoTransport,
    };
    use gittree_observability::{ObservabilityConfigError, ObservabilityError};
    use gittree_storage::{
        AnnouncementRepository, InMemoryRepositories, RelayPublishJob, RelayPublishRepository,
        RelayPublishRequest, RepoAnnouncementRecord, RepoMappingRecord, RepoMappingRepository,
        StorageConfig, StorageError,
    };
    use std::collections::VecDeque;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex, OnceLock};
    use std::time::Duration as StdDuration;
    use time::{Duration as TimeDuration, OffsetDateTime};
    use tower::ServiceExt;

    static ENV_LOCK: Mutex<()> = Mutex::new(());
    static OBSERVABILITY: OnceLock<ObservabilityHandle> = OnceLock::new();

    #[derive(Clone, Default)]
    struct MockTransport {
        requests: Arc<Mutex<Vec<ForgejoRequest>>>,
        responses: Arc<Mutex<VecDeque<ForgejoResponse>>>,
    }

    impl MockTransport {
        fn new(responses: Vec<ForgejoResponse>) -> Self {
            Self {
                requests: Arc::new(Mutex::new(Vec::new())),
                responses: Arc::new(Mutex::new(VecDeque::from(responses))),
            }
        }

        fn requests(&self) -> Vec<ForgejoRequest> {
            self.requests.lock().expect("requests").clone()
        }
    }

    #[derive(Clone, Default)]
    struct ScriptedOutboxRepositories {
        inner: Arc<InMemoryRepositories>,
        fail_claim: bool,
        fail_mark_succeeded: bool,
        fail_pending: bool,
        fail_mark_failed: bool,
    }

    impl ScriptedOutboxRepositories {
        fn with_flags(
            fail_claim: bool,
            fail_mark_succeeded: bool,
            fail_pending: bool,
            fail_mark_failed: bool,
        ) -> Self {
            Self {
                inner: Arc::new(InMemoryRepositories::new()),
                fail_claim,
                fail_mark_succeeded,
                fail_pending,
                fail_mark_failed,
            }
        }
    }

    #[async_trait]
    impl AnnouncementRepository for ScriptedOutboxRepositories {
        async fn insert_announcement(
            &self,
            record: RepoAnnouncementRecord,
        ) -> Result<(), StorageError> {
            self.inner.insert_announcement(record).await
        }

        async fn list_announcements(
            &self,
            pubkey: &[u8],
            identifier: &str,
        ) -> Result<Vec<RepoAnnouncementRecord>, StorageError> {
            self.inner.list_announcements(pubkey, identifier).await
        }

        async fn latest_announcement(
            &self,
            pubkey: &[u8],
            identifier: &str,
        ) -> Result<Option<RepoAnnouncementRecord>, StorageError> {
            self.inner.latest_announcement(pubkey, identifier).await
        }
    }

    #[async_trait]
    impl RepoMappingRepository for ScriptedOutboxRepositories {
        async fn upsert_mapping(&self, record: RepoMappingRecord) -> Result<(), StorageError> {
            self.inner.upsert_mapping(record).await
        }

        async fn mapping_by_forgejo(
            &self,
            owner: &str,
            repo: &str,
        ) -> Result<Option<RepoMappingRecord>, StorageError> {
            self.inner.mapping_by_forgejo(owner, repo).await
        }

        async fn mapping_by_repo(
            &self,
            pubkey: &[u8],
            identifier: &str,
        ) -> Result<Option<RepoMappingRecord>, StorageError> {
            self.inner.mapping_by_repo(pubkey, identifier).await
        }

        async fn list_mappings(&self) -> Result<Vec<RepoMappingRecord>, StorageError> {
            self.inner.list_mappings().await
        }
    }

    #[async_trait]
    impl RelayPublishRepository for ScriptedOutboxRepositories {
        async fn enqueue_relay_publish(
            &self,
            request: RelayPublishRequest,
        ) -> Result<(), StorageError> {
            self.inner.enqueue_relay_publish(request).await
        }

        async fn claim_relay_publish(
            &self,
            now: OffsetDateTime,
        ) -> Result<Option<RelayPublishJob>, StorageError> {
            if self.fail_claim {
                return Err(StorageError::Internal {
                    message: "claim failure".to_string(),
                });
            }
            self.inner.claim_relay_publish(now).await
        }

        async fn mark_relay_publish_succeeded(&self, id: i64) -> Result<(), StorageError> {
            if self.fail_mark_succeeded {
                return Err(StorageError::Internal {
                    message: "mark succeeded failure".to_string(),
                });
            }
            self.inner.mark_relay_publish_succeeded(id).await
        }

        async fn mark_relay_publish_failed(
            &self,
            id: i64,
            error: &str,
            retry_at: OffsetDateTime,
        ) -> Result<(), StorageError> {
            if self.fail_mark_failed {
                return Err(StorageError::Internal {
                    message: "mark failed failure".to_string(),
                });
            }
            self.inner
                .mark_relay_publish_failed(id, error, retry_at)
                .await
        }

        async fn pending_relay_publishes(
            &self,
            pubkey: &[u8],
            identifier: &str,
            kind: u32,
        ) -> Result<i64, StorageError> {
            if self.fail_pending {
                return Err(StorageError::Internal {
                    message: "pending failure".to_string(),
                });
            }
            self.inner
                .pending_relay_publishes(pubkey, identifier, kind)
                .await
        }
    }

    async fn publish_ok(_: String, _: super::SignedNostrEvent) -> Result<(), String> {
        Ok(())
    }

    async fn publish_err(_: String, _: super::SignedNostrEvent) -> Result<(), String> {
        Err("publish failed".to_string())
    }

    #[async_trait]
    impl ForgejoTransport for MockTransport {
        async fn send(
            &self,
            request: ForgejoRequest,
        ) -> Result<ForgejoResponse, gittree_forgejo::ForgejoError> {
            self.requests.lock().expect("requests").push(request);
            self.responses
                .lock()
                .expect("responses")
                .pop_front()
                .ok_or_else(|| {
                    gittree_forgejo::ForgejoError::Request("missing mock response".to_string())
                })
        }
    }

    fn test_forgejo_config() -> ForgejoConfig {
        ForgejoConfig {
            base_url: "http://localhost:3000".to_string(),
            api_token: "token".to_string(),
            owner: "gittree".to_string(),
            webhook_url: "http://localhost:8090/".to_string(),
            webhook_secret: "secret".to_string(),
            repo_private: true,
        }
    }

    fn forgejo_client_with_responses(
        responses: Vec<ForgejoResponse>,
    ) -> (ForgejoClient<MockTransport>, MockTransport) {
        let transport = MockTransport::new(responses);
        let client = ForgejoClient::with_transport(test_forgejo_config(), transport.clone());
        (client, transport)
    }

    fn repo_json(owner: &str, name: &str) -> String {
        format!(
            r#"{{"full_name":"{owner}/{name}","name":"{name}","owner":{{"username":"{owner}"}},"html_url":"http://localhost/{owner}/{name}"}}"#
        )
    }

    fn sample_storage_config(read_connection: &str) -> StorageConfig {
        StorageConfig {
            read_connection: read_connection.to_string(),
            write_connection: None,
            max_connections: 10,
            min_connections: 2,
            idle_timeout_secs: None,
            max_lifetime_secs: None,
            application_name: None,
        }
    }

    fn sample_coordinator_config(storage: StorageConfig) -> CoordinatorConfig {
        CoordinatorConfig {
            bind: "127.0.0.1:9091".to_string(),
            storage,
            relay_urls: vec!["wss://relay.example".to_string()],
            repo_root: PathBuf::from("/tmp/gittree-repos"),
            hooks: HookInstallConfig {
                pre_receive_source: PathBuf::from("/tmp/gittree-pre-receive"),
                post_receive_source: PathBuf::from("/tmp/gittree-post-receive"),
            },
            forgejo: test_forgejo_config(),
        }
    }

    fn sample_plan(repo_path: PathBuf) -> super::RepoProvisionPlan {
        let hooks_dir = repo_path.join("hooks");
        super::RepoProvisionPlan {
            npub: "npub1test".to_string(),
            identifier: "repo".to_string(),
            pre_receive_hook: hooks_dir.join("pre-receive"),
            post_receive_hook: hooks_dir.join("post-receive"),
            hooks_dir,
            repo_path,
            git_config: Vec::new(),
        }
    }

    fn sample_publish_job(kind: u32, tags: Vec<Vec<String>>, identifier: &str) -> RelayPublishJob {
        RelayPublishJob {
            id: 7,
            relay_url: "wss://relay.example".to_string(),
            event_id: vec![0x11; 32],
            pubkey: vec![0x22; 32],
            created_at: 42,
            kind,
            tags,
            content: "content".to_string(),
            sig: vec![0x33; 64],
            forgejo_owner: "owner".to_string(),
            forgejo_repo: "repo".to_string(),
            identifier: identifier.to_string(),
            attempt_count: 1,
            publish_after: OffsetDateTime::from_unix_timestamp(0).expect("ts"),
        }
    }

    #[tokio::test]
    async fn scripted_outbox_repositories_delegate_announcement_and_mapping_methods() {
        let repositories = ScriptedOutboxRepositories::default();
        let mapping =
            super::RepoMapping::new("owner", "repo", "11".repeat(32), "repo").expect("mapping");
        let mapping_record = RepoMappingRecord::new(&mapping).expect("mapping record");
        repositories
            .upsert_mapping(mapping_record)
            .await
            .expect("upsert mapping");
        let pubkey = hex::decode("11".repeat(32)).expect("pubkey");
        assert!(
            repositories
                .mapping_by_forgejo("owner", "repo")
                .await
                .expect("mapping by forgejo")
                .is_some()
        );
        assert!(
            repositories
                .mapping_by_repo(&pubkey, "repo")
                .await
                .expect("mapping by repo")
                .is_some()
        );
        assert_eq!(
            repositories
                .list_mappings()
                .await
                .expect("list mappings")
                .len(),
            1
        );

        let announcement = RepoAnnouncement {
            identifier: "repo".to_string(),
            name: None,
            description: None,
            root_commit: None,
            clone: vec!["https://example.com/repo.git".to_string()],
            web: Vec::new(),
            relays: vec!["wss://relay.example".to_string()],
            blossoms: Vec::new(),
            hashtags: Vec::new(),
            maintainers: Vec::new(),
        };
        let announcement_record =
            RepoAnnouncementRecord::new(&"aa".repeat(32), &"11".repeat(32), 42, &announcement)
                .expect("announcement record");
        repositories
            .insert_announcement(announcement_record)
            .await
            .expect("insert announcement");
        assert_eq!(
            repositories
                .list_announcements(&pubkey, "repo")
                .await
                .expect("list announcements")
                .len(),
            1
        );
        assert!(
            repositories
                .latest_announcement(&pubkey, "repo")
                .await
                .expect("latest announcement")
                .is_some()
        );
    }

    fn sample_publish_request(
        relay_url: &str,
        kind: u32,
        tags: Vec<Vec<String>>,
    ) -> RelayPublishRequest {
        RelayPublishRequest {
            relay_url: relay_url.to_string(),
            event_id: "11".repeat(32),
            pubkey: "22".repeat(32),
            created_at: 42,
            kind,
            tags,
            content: "content".to_string(),
            sig: "33".repeat(64),
            forgejo_owner: "owner".to_string(),
            forgejo_repo: "repo".to_string(),
            identifier: "repo".to_string(),
        }
    }

    fn with_env_var<F: FnOnce()>(key: &str, value: &str, f: F) {
        let previous = std::env::var_os(key);
        // SAFETY: tests run single-threaded in this crate; we restore the previous value after.
        unsafe {
            std::env::set_var(key, value);
        }
        f();
        match previous {
            Some(old) => unsafe {
                std::env::set_var(key, old);
            },
            None => unsafe {
                std::env::remove_var(key);
            },
        }
    }

    fn with_unset_env_var<F: FnOnce()>(key: &str, f: F) {
        let previous = std::env::var_os(key);
        // SAFETY: tests run single-threaded in this crate; we restore the previous value after.
        unsafe {
            std::env::remove_var(key);
        }
        f();
        if let Some(old) = previous {
            // SAFETY: tests run single-threaded in this crate; we restore the previous value after.
            unsafe {
                std::env::set_var(key, old);
            }
        }
    }

    fn with_forgejo_envs<F: FnOnce()>(f: F) {
        with_env_var("GITTREE_FORGEJO_BASE_URL", "http://localhost:3000", || {
            with_env_var("GITTREE_FORGEJO_API_TOKEN", "token", || {
                with_env_var("GITTREE_FORGEJO_OWNER", "gittree", || {
                    with_env_var(
                        "GITTREE_FORGEJO_WEBHOOK_URL",
                        "http://localhost:8090/",
                        || {
                            with_env_var("GITTREE_FORGEJO_WEBHOOK_SECRET", "secret", f);
                        },
                    );
                });
            });
        });
    }

    fn with_required_coordinator_envs<F: FnOnce()>(f: F) {
        with_env_var(
            ENV_STORAGE_READ_URL,
            "postgres://user:pass@localhost:5432/gittree",
            || {
                with_env_var("GITTREE_COORDINATOR_BIND", "127.0.0.1:9091", || {
                    with_env_var("GITTREE_RELAY_URLS", "wss://relay.example", || {
                        with_env_var(
                            super::ENV_COORDINATOR_REPO_ROOT,
                            "/tmp/gittree-repos",
                            || {
                                with_env_var(
                                    super::ENV_COORDINATOR_PRE_RECEIVE_HOOK,
                                    "/tmp/gittree-pre-receive",
                                    || {
                                        with_env_var(
                                            super::ENV_COORDINATOR_POST_RECEIVE_HOOK,
                                            "/tmp/gittree-post-receive",
                                            || {
                                                with_forgejo_envs(f);
                                            },
                                        );
                                    },
                                );
                            },
                        );
                    });
                });
            },
        );
    }

    #[test]
    fn config_loads_from_env() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var(
            ENV_STORAGE_READ_URL,
            "postgres://user:pass@localhost:5432/gittree",
            || {
                with_env_var("GITTREE_COORDINATOR_BIND", "127.0.0.1:9091", || {
                    with_env_var("GITTREE_RELAY_URLS", "wss://relay.example", || {
                        with_env_var(
                            super::ENV_COORDINATOR_REPO_ROOT,
                            "/tmp/gittree-repos",
                            || {
                                with_env_var(
                                    super::ENV_COORDINATOR_PRE_RECEIVE_HOOK,
                                    "/tmp/gittree-pre-receive",
                                    || {
                                        with_env_var(
                                            super::ENV_COORDINATOR_POST_RECEIVE_HOOK,
                                            "/tmp/gittree-post-receive",
                                            || {
                                                with_forgejo_envs(|| {
                                                    let config = CoordinatorConfig::from_env()
                                                        .expect("config");
                                                    assert_eq!(config.bind, "127.0.0.1:9091");
                                                    assert_eq!(
                                                        config.storage.read_connection,
                                                        "postgres://user:pass@localhost:5432/gittree"
                                                    );
                                                    assert_eq!(
                                                        config.relay_urls,
                                                        vec!["wss://relay.example".to_string()]
                                                    );
                                                    assert_eq!(
                                                        config.repo_root,
                                                        PathBuf::from("/tmp/gittree-repos")
                                                    );
                                                    assert_eq!(
                                                        config.hooks.pre_receive_source,
                                                        PathBuf::from("/tmp/gittree-pre-receive")
                                                    );
                                                    assert_eq!(
                                                        config.hooks.post_receive_source,
                                                        PathBuf::from("/tmp/gittree-post-receive")
                                                    );
                                                    assert_eq!(
                                                        config.forgejo.base_url,
                                                        "http://localhost:3000"
                                                    );
                                                    assert_eq!(config.forgejo.owner, "gittree");
                                                });
                                            },
                                        );
                                    },
                                );
                            },
                        );
                    });
                });
            },
        );
    }

    #[test]
    fn config_ignores_empty_pool_timeouts() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var(
            ENV_STORAGE_READ_URL,
            "postgres://user:pass@localhost:5432/gittree",
            || {
                with_env_var(
                    super::ENV_COORDINATOR_REPO_ROOT,
                    "/tmp/gittree-repos",
                    || {
                        with_env_var(super::ENV_COORDINATOR_PRE_RECEIVE_HOOK, "/tmp/pre", || {
                            with_env_var(
                                super::ENV_COORDINATOR_POST_RECEIVE_HOOK,
                                "/tmp/post",
                                || {
                                    with_env_var(super::ENV_STORAGE_IDLE_TIMEOUT_SECS, "", || {
                                        with_env_var(
                                            super::ENV_STORAGE_MAX_LIFETIME_SECS,
                                            "",
                                            || {
                                                with_forgejo_envs(|| {
                                                    let config = CoordinatorConfig::from_env()
                                                        .expect("config");
                                                    assert_eq!(
                                                        config.storage.idle_timeout_secs,
                                                        None
                                                    );
                                                    assert_eq!(
                                                        config.storage.max_lifetime_secs,
                                                        None
                                                    );
                                                });
                                            },
                                        );
                                    });
                                },
                            );
                        });
                    },
                );
            },
        );
    }

    #[test]
    fn config_rejects_invalid_storage_integer_envs() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_required_coordinator_envs(|| {
            with_env_var(super::ENV_STORAGE_MAX_CONNECTIONS, "nope", || {
                let err = CoordinatorConfig::from_env().expect_err("invalid max connections");
                assert!(matches!(
                    err,
                    super::CoordinatorConfigError::Storage(super::StorageConfigError::InvalidEnv {
                        key: super::ENV_STORAGE_MAX_CONNECTIONS,
                        ..
                    })
                ));
            });
            with_env_var(super::ENV_STORAGE_IDLE_TIMEOUT_SECS, "bad", || {
                let err = CoordinatorConfig::from_env().expect_err("invalid idle timeout");
                assert!(matches!(
                    err,
                    super::CoordinatorConfigError::Storage(super::StorageConfigError::InvalidEnv {
                        key: super::ENV_STORAGE_IDLE_TIMEOUT_SECS,
                        ..
                    })
                ));
            });
            with_env_var(super::ENV_STORAGE_MIN_CONNECTIONS, "bad", || {
                let err = CoordinatorConfig::from_env().expect_err("invalid min connections");
                assert!(matches!(
                    err,
                    super::CoordinatorConfigError::Storage(super::StorageConfigError::InvalidEnv {
                        key: super::ENV_STORAGE_MIN_CONNECTIONS,
                        ..
                    })
                ));
            });
            with_env_var(super::ENV_STORAGE_MAX_LIFETIME_SECS, "bad", || {
                let err = CoordinatorConfig::from_env().expect_err("invalid max lifetime");
                assert!(matches!(
                    err,
                    super::CoordinatorConfigError::Storage(super::StorageConfigError::InvalidEnv {
                        key: super::ENV_STORAGE_MAX_LIFETIME_SECS,
                        ..
                    })
                ));
            });
        });
    }

    #[test]
    fn config_rejects_missing_read_url_and_invalid_pool_ranges() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_required_coordinator_envs(|| {
            with_unset_env_var(ENV_STORAGE_READ_URL, || {
                let err = CoordinatorConfig::from_env().expect_err("missing read url");
                assert!(matches!(
                    err,
                    super::CoordinatorConfigError::Storage(super::StorageConfigError::MissingEnv(
                        ENV_STORAGE_READ_URL
                    ))
                ));
            });

            with_env_var(super::ENV_STORAGE_MAX_CONNECTIONS, "1", || {
                with_env_var(super::ENV_STORAGE_MIN_CONNECTIONS, "2", || {
                    let err = CoordinatorConfig::from_env().expect_err("invalid pool range");
                    assert!(matches!(
                        err,
                        super::CoordinatorConfigError::Storage(
                            super::StorageConfigError::InvalidConfig(_)
                        )
                    ));
                });
            });

            with_env_var(super::ENV_STORAGE_MAX_CONNECTIONS, "", || {
                let config =
                    CoordinatorConfig::from_env().expect("empty max connections uses default");
                assert_eq!(config.storage.max_connections, 10);
            });
        });
    }

    #[test]
    fn config_rejects_invalid_relay_and_missing_forgejo_env() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_required_coordinator_envs(|| {
            with_env_var("GITTREE_RELAY_URLS", "not-a-url", || {
                let err = CoordinatorConfig::from_env().expect_err("invalid relay target");
                assert!(matches!(err, super::CoordinatorConfigError::Config(_)));
            });

            with_unset_env_var("GITTREE_FORGEJO_OWNER", || {
                let err = CoordinatorConfig::from_env().expect_err("missing forgejo owner");
                assert!(matches!(err, super::CoordinatorConfigError::Config(_)));
            });
        });
    }

    #[test]
    fn config_rejects_missing_and_empty_paths() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_unset_env_var(super::ENV_COORDINATOR_REPO_ROOT, || {
            let err = super::env_path(super::ENV_COORDINATOR_REPO_ROOT).expect_err("missing path");
            assert!(matches!(
                err,
                super::CoordinatorConfigError::MissingEnv(super::ENV_COORDINATOR_REPO_ROOT)
            ));
        });
        with_env_var(super::ENV_COORDINATOR_REPO_ROOT, "  ", || {
            let err = super::env_path(super::ENV_COORDINATOR_REPO_ROOT).expect_err("empty path");
            assert!(matches!(
                err,
                super::CoordinatorConfigError::InvalidEnv {
                    key: super::ENV_COORDINATOR_REPO_ROOT,
                    ..
                }
            ));
        });
    }

    #[test]
    fn config_rejects_missing_paths_during_from_env() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_required_coordinator_envs(|| {
            with_unset_env_var(super::ENV_COORDINATOR_REPO_ROOT, || {
                let err = CoordinatorConfig::from_env().expect_err("missing repo root");
                assert!(matches!(
                    err,
                    super::CoordinatorConfigError::MissingEnv(super::ENV_COORDINATOR_REPO_ROOT)
                ));
            });
            with_unset_env_var(super::ENV_COORDINATOR_PRE_RECEIVE_HOOK, || {
                let err = CoordinatorConfig::from_env().expect_err("missing pre-receive hook");
                assert!(matches!(
                    err,
                    super::CoordinatorConfigError::MissingEnv(
                        super::ENV_COORDINATOR_PRE_RECEIVE_HOOK
                    )
                ));
            });
            with_unset_env_var(super::ENV_COORDINATOR_POST_RECEIVE_HOOK, || {
                let err = CoordinatorConfig::from_env().expect_err("missing post-receive hook");
                assert!(matches!(
                    err,
                    super::CoordinatorConfigError::MissingEnv(
                        super::ENV_COORDINATOR_POST_RECEIVE_HOOK
                    )
                ));
            });
        });
    }

    #[test]
    fn coordinator_and_storage_error_display_and_source() {
        let config = super::CoordinatorConfigError::Config(ConfigError::MissingEnv("MISSING_ENV"));
        assert_eq!(
            format!("{config}"),
            "coordinator config error: missing env MISSING_ENV"
        );
        assert!(std::error::Error::source(&config).is_some());

        let storage = super::CoordinatorConfigError::Storage(
            super::StorageConfigError::InvalidConfig("invalid storage config".to_string()),
        );
        assert_eq!(
            format!("{storage}"),
            "coordinator storage config error: invalid storage config"
        );
        assert!(std::error::Error::source(&storage).is_some());

        let missing = super::CoordinatorConfigError::MissingEnv("REQUIRED");
        assert_eq!(format!("{missing}"), "missing env REQUIRED");
        assert!(std::error::Error::source(&missing).is_none());

        let invalid = super::CoordinatorConfigError::InvalidEnv {
            key: "BAD_KEY",
            value: "value".to_string(),
        };
        assert_eq!(format!("{invalid}"), "invalid env BAD_KEY: value");
        assert!(std::error::Error::source(&invalid).is_none());

        let storage_missing = super::StorageConfigError::MissingEnv("STORAGE_KEY");
        assert_eq!(format!("{storage_missing}"), "missing env STORAGE_KEY");
        let storage_invalid = super::StorageConfigError::InvalidEnv {
            key: "STORAGE_KEY",
            value: "NaN".to_string(),
        };
        assert_eq!(format!("{storage_invalid}"), "invalid env STORAGE_KEY: NaN");
    }

    #[test]
    fn coordinator_error_display_and_source() {
        let config = super::CoordinatorError::Config(super::CoordinatorConfigError::MissingEnv(
            "CFG_MISSING",
        ));
        assert_eq!(
            format!("{config}"),
            "coordinator error: missing env CFG_MISSING"
        );
        assert!(std::error::Error::source(&config).is_some());

        let observability_config =
            super::CoordinatorError::ObservabilityConfig(ObservabilityConfigError::InvalidEnv {
                key: "OTEL_METRICS_ENABLED",
                value: "wat".to_string(),
            });
        assert!(
            format!("{observability_config}").contains("coordinator observability config error")
        );
        assert!(std::error::Error::source(&observability_config).is_some());

        let observability =
            super::CoordinatorError::Observability(ObservabilityError::LogInit("boom".to_string()));
        assert!(format!("{observability}").contains("coordinator observability error"));
        assert!(std::error::Error::source(&observability).is_some());

        let storage = super::CoordinatorError::Storage(StorageError::Internal {
            message: "storage down".to_string(),
        });
        assert!(format!("{storage}").contains("coordinator storage error"));
        assert!(std::error::Error::source(&storage).is_some());

        let forgejo = super::CoordinatorError::Forgejo(gittree_forgejo::ForgejoError::Request(
            "request failed".to_string(),
        ));
        assert!(format!("{forgejo}").contains("coordinator forgejo error"));
        assert!(std::error::Error::source(&forgejo).is_some());

        let serve = super::CoordinatorError::Serve("bind failed".to_string());
        assert_eq!(format!("{serve}"), "coordinator serve error: bind failed");
        assert!(std::error::Error::source(&serve).is_none());
    }

    #[tokio::test]
    async fn build_repositories_maps_storage_errors_and_accepts_valid_config() {
        let invalid_storage = StorageConfig {
            max_connections: 0,
            ..sample_storage_config("postgres://user:pass@localhost:5432/gittree")
        };
        let invalid_config = sample_coordinator_config(invalid_storage);
        let invalid = super::build_repositories(&invalid_config).expect_err("invalid config");
        assert!(matches!(
            invalid,
            super::CoordinatorError::Storage(StorageError::InvalidPoolConfig {
                field: "max_connections",
                value: 0
            })
        ));

        let valid_config = sample_coordinator_config(sample_storage_config(
            "postgres://user:pass@localhost:5432/gittree",
        ));
        super::build_repositories(&valid_config).expect("valid config");
    }

    #[test]
    fn plan_builds_repo_paths() {
        let announcement = RepoAnnouncement {
            identifier: "repo".to_string(),
            name: None,
            description: None,
            root_commit: None,
            clone: Vec::new(),
            web: Vec::new(),
            relays: Vec::new(),
            blossoms: Vec::new(),
            hashtags: Vec::new(),
            maintainers: Vec::new(),
        };
        let root = std::path::Path::new("/var/lib/gittree");
        let npub = "npub1gjttreegkzys8jlhdnfm3qe39h2gka79cpndd0jsms5fk7tuhcnsdw56jq";
        let plan = build_provision_plan(root, npub, &announcement).expect("plan");
        assert_eq!(plan.repo_path, root.join(npub).join("repo.git"));
        assert_eq!(plan.hooks_dir, plan.repo_path.join("hooks"));
        assert_eq!(plan.pre_receive_hook, plan.hooks_dir.join("pre-receive"));
        assert_eq!(plan.post_receive_hook, plan.hooks_dir.join("post-receive"));
        assert!(plan.git_config.iter().any(|entry| entry.key == "core.bare"));
    }

    #[test]
    fn plan_builds_repo_paths_from_owned_pathbuf() {
        let announcement = RepoAnnouncement {
            identifier: "repo".to_string(),
            name: None,
            description: None,
            root_commit: None,
            clone: Vec::new(),
            web: Vec::new(),
            relays: Vec::new(),
            blossoms: Vec::new(),
            hashtags: Vec::new(),
            maintainers: Vec::new(),
        };
        let root = PathBuf::from("/var/lib/gittree");
        let npub = "npub1gjttreegkzys8jlhdnfm3qe39h2gka79cpndd0jsms5fk7tuhcnsdw56jq";
        let plan = build_provision_plan(root.clone(), npub, &announcement).expect("plan");
        assert_eq!(plan.repo_path, root.join(npub).join("repo.git"));
    }

    #[test]
    fn plan_rejects_invalid_npub_repo_path() {
        let announcement = RepoAnnouncement {
            identifier: "repo".to_string(),
            name: None,
            description: None,
            root_commit: None,
            clone: Vec::new(),
            web: Vec::new(),
            relays: Vec::new(),
            blossoms: Vec::new(),
            hashtags: Vec::new(),
            maintainers: Vec::new(),
        };
        let root = std::path::Path::new("/var/lib/gittree");
        let err = build_provision_plan(root, "npub-invalid", &announcement).expect_err("invalid npub");
        assert!(matches!(err, super::ProvisionPlanError::InvalidRepo(_)));
    }

    #[test]
    fn init_repo_creates_bare_repo() {
        let announcement = RepoAnnouncement {
            identifier: "repo".to_string(),
            name: None,
            description: None,
            root_commit: None,
            clone: Vec::new(),
            web: Vec::new(),
            relays: Vec::new(),
            blossoms: Vec::new(),
            hashtags: Vec::new(),
            maintainers: Vec::new(),
        };
        let temp_dir = temp_dir("gittree-init-repo");
        let npub = "npub1gjttreegkzys8jlhdnfm3qe39h2gka79cpndd0jsms5fk7tuhcnsdw56jq";
        let plan = build_provision_plan(&temp_dir, npub, &announcement).expect("plan");
        let report = init_repo(&plan).expect("init");
        assert!(report.created);
        assert!(plan.repo_path.join("HEAD").exists());
        let second = init_repo(&plan).expect("init again");
        assert!(!second.created);
        let _ = fs::remove_dir_all(temp_dir);
    }

    #[cfg(unix)]
    #[test]
    fn repo_init_helpers_reject_non_utf8_paths() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;

        let non_utf8 = PathBuf::from(OsString::from_vec(vec![0xff, b'a']));
        let init_err = super::create_bare_repo(&non_utf8).expect_err("invalid path");
        assert!(matches!(init_err, super::RepoInitError::InvalidPath(_)));

        let entry = super::GitConfigEntry::new("core.bare", "true");
        let config_err = super::apply_git_config(&non_utf8, &entry).expect_err("invalid path");
        assert!(matches!(config_err, super::RepoInitError::InvalidPath(_)));
    }

    #[test]
    fn install_hooks_is_idempotent() {
        let announcement = RepoAnnouncement {
            identifier: "repo".to_string(),
            name: None,
            description: None,
            root_commit: None,
            clone: Vec::new(),
            web: Vec::new(),
            relays: Vec::new(),
            blossoms: Vec::new(),
            hashtags: Vec::new(),
            maintainers: Vec::new(),
        };
        let temp_dir = temp_dir("gittree-hooks");
        let bin_dir = temp_dir.join("bin");
        fs::create_dir_all(&bin_dir).expect("bin dir");
        let pre_source = bin_dir.join("pre-receive");
        let post_source = bin_dir.join("post-receive");
        fs::write(&pre_source, "#!/bin/sh\necho pre\n").expect("pre hook");
        fs::write(&post_source, "#!/bin/sh\necho post\n").expect("post hook");

        let repo_root = temp_dir.join("repos");
        let npub = "npub1gjttreegkzys8jlhdnfm3qe39h2gka79cpndd0jsms5fk7tuhcnsdw56jq";
        let plan = build_provision_plan(&repo_root, npub, &announcement).expect("plan");
        let config = HookInstallConfig {
            pre_receive_source: pre_source,
            post_receive_source: post_source,
        };
        let report = install_hooks(&plan, &config).expect("install hooks");
        assert_eq!(report.installed, 2);
        assert!(plan.pre_receive_hook.exists());
        assert!(plan.post_receive_hook.exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let pre_mode = fs::metadata(&plan.pre_receive_hook)
                .expect("pre metadata")
                .permissions()
                .mode();
            let post_mode = fs::metadata(&plan.post_receive_hook)
                .expect("post metadata")
                .permissions()
                .mode();
            assert!(pre_mode & 0o111 != 0);
            assert!(post_mode & 0o111 != 0);
        }
        let second = install_hooks(&plan, &config).expect("install again");
        assert_eq!(second.installed, 2);
        let _ = fs::remove_dir_all(temp_dir);
    }

    #[test]
    fn handle_event_provisions_repo() {
        let npub = "npub1gjttreegkzys8jlhdnfm3qe39h2gka79cpndd0jsms5fk7tuhcnsdw56jq";
        let announcement = RepoAnnouncement {
            identifier: "repo".to_string(),
            name: None,
            description: None,
            root_commit: None,
            clone: vec![format!("https://gittr.ee/{npub}/repo.git")],
            web: Vec::new(),
            relays: vec!["wss://gittr.ee".to_string()],
            blossoms: Vec::new(),
            hashtags: Vec::new(),
            maintainers: Vec::new(),
        };
        let event = RelayEvent {
            kind: KIND_GIT_REPO_ANNOUNCEMENT.0,
            event_id: "22".repeat(32),
            pubkey: "11".repeat(32),
            created_at: 10,
            tags: announcement.to_tags(),
        };
        let temp_dir = temp_dir("gittree-event");
        let bin_dir = temp_dir.join("bin");
        fs::create_dir_all(&bin_dir).expect("bin dir");
        let pre_source = bin_dir.join("pre-receive");
        let post_source = bin_dir.join("post-receive");
        fs::write(&pre_source, "#!/bin/sh\necho pre\n").expect("pre hook");
        fs::write(&post_source, "#!/bin/sh\necho post\n").expect("post hook");
        let hooks = HookInstallConfig {
            pre_receive_source: pre_source,
            post_receive_source: post_source,
        };
        let repo_root = temp_dir.join("repos");
        let action = handle_announcement_event(&repo_root, &hooks, &event).expect("handle");
        assert!(matches!(action, CoordinatorAction::Provisioned { .. }));
        let again = handle_announcement_event(&repo_root, &hooks, &event).expect("handle");
        assert!(matches!(again, CoordinatorAction::SkippedExisting { .. }));
        let _ = fs::remove_dir_all(temp_dir);
    }

    #[test]
    fn action_payload_maps_skipped_and_ignored_variants() {
        let skipped = CoordinatorAction::SkippedExisting {
            repo_path: PathBuf::from("/tmp/repo.git"),
        };
        let skipped_payload = CoordinatorActionPayload::from(skipped);
        assert!(matches!(
            skipped_payload,
            CoordinatorActionPayload::SkippedExisting { ref repo_path }
            if repo_path == "/tmp/repo.git"
        ));

        let ignored_payload = CoordinatorActionPayload::from(CoordinatorAction::Ignored);
        assert!(matches!(ignored_payload, CoordinatorActionPayload::Ignored));
    }

    #[tokio::test]
    async fn http_error_maps_event_errors_to_expected_status() {
        let cases = vec![
            (
                super::CoordinatorEventError::Parse("bad parse".to_string()),
                axum::http::StatusCode::BAD_REQUEST,
                "bad parse",
            ),
            (
                super::CoordinatorEventError::MissingNpub,
                axum::http::StatusCode::BAD_REQUEST,
                "missing npub",
            ),
            (
                super::CoordinatorEventError::Plan(super::ProvisionPlanError::InvalidRepo(
                    "bad plan".to_string(),
                )),
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "bad plan",
            ),
            (
                super::CoordinatorEventError::Init(super::RepoInitError::InvalidRepo(
                    "bad init".to_string(),
                )),
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "bad init",
            ),
            (
                super::CoordinatorEventError::Hooks(super::HookInstallError::MissingSource(
                    "/tmp/missing-hook".to_string(),
                )),
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "missing hook source",
            ),
            (
                super::CoordinatorEventError::Storage(StorageError::Internal {
                    message: "storage failed".to_string(),
                }),
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "storage failed",
            ),
            (
                super::CoordinatorEventError::Forgejo(gittree_forgejo::ForgejoError::Request(
                    "forgejo failed".to_string(),
                )),
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "forgejo failed",
            ),
            (
                super::CoordinatorEventError::Mapping(gittree_core::CoreError::InvalidField {
                    field: "repo",
                    value: "bad".to_string(),
                }),
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "invalid field repo: bad",
            ),
        ];

        for (error, expected_status, expected_body_fragment) in cases {
            let response = super::CoordinatorHttpError::from(error).into_response();
            assert_eq!(response.status(), expected_status);
            let body = to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("body");
            let text = String::from_utf8(body.to_vec()).expect("utf8");
            assert!(
                text.contains(expected_body_fragment),
                "body `{text}` did not contain `{expected_body_fragment}`"
            );
        }
    }

    #[tokio::test]
    async fn announcement_endpoint_rejects_invalid_kind() {
        let repositories = Arc::new(InMemoryRepositories::new());
        let temp_dir = temp_dir("gittree-coordinator-http-invalid-kind");
        let hooks = HookInstallConfig {
            pre_receive_source: temp_dir.join("pre-receive"),
            post_receive_source: temp_dir.join("post-receive"),
        };
        let (forgejo, _transport) = forgejo_client_with_responses(Vec::new());
        let app = super::build_router(super::CoordinatorAppState {
            repositories,
            repo_root: temp_dir.join("repos"),
            hooks,
            forgejo,
        });
        let payload = CoordinatorEventPayload {
            kind: u64::from(u32::MAX) + 1,
            event_id: "44".repeat(32),
            pubkey: "11".repeat(32),
            created_at: 10,
            tags: Vec::new(),
        };
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/announcement")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&payload).expect("body")))
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), axum::http::StatusCode::BAD_REQUEST);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let text = String::from_utf8(body.to_vec()).expect("utf8");
        assert!(text.contains("invalid kind"));
        let _ = fs::remove_dir_all(temp_dir);
    }

    #[test]
    fn handle_event_ignores_non_repo_nip34_events() {
        let list = UserGraspList {
            urls: vec!["wss://relay.example".to_string()],
        };
        let event = RelayEvent {
            kind: KIND_USER_GRASP_LIST.0,
            event_id: "55".repeat(32),
            pubkey: "11".repeat(32),
            created_at: 10,
            tags: list.to_tags(),
        };
        let temp_dir = temp_dir("gittree-ignored-event");
        let hooks = HookInstallConfig {
            pre_receive_source: temp_dir.join("pre-receive"),
            post_receive_source: temp_dir.join("post-receive"),
        };
        let action = handle_announcement_event(&temp_dir, &hooks, &event).expect("ignored");
        assert!(matches!(action, CoordinatorAction::Ignored));
        let _ = fs::remove_dir_all(temp_dir);
    }

    #[tokio::test]
    async fn handle_event_with_storage_ignores_non_repo_nip34_events() {
        let list = UserGraspList {
            urls: vec!["wss://relay.example".to_string()],
        };
        let event = RelayEvent {
            kind: KIND_USER_GRASP_LIST.0,
            event_id: "66".repeat(32),
            pubkey: "11".repeat(32),
            created_at: 10,
            tags: list.to_tags(),
        };
        let temp_dir = temp_dir("gittree-ignored-storage-event");
        let hooks = HookInstallConfig {
            pre_receive_source: temp_dir.join("pre-receive"),
            post_receive_source: temp_dir.join("post-receive"),
        };
        let (forgejo, transport) = forgejo_client_with_responses(Vec::new());
        let storage = InMemoryRepositories::new();
        let action =
            handle_announcement_event_with_storage(&temp_dir, &hooks, &storage, &forgejo, &event)
                .await
                .expect("ignored");
        assert!(matches!(action, CoordinatorAction::Ignored));
        assert!(transport.requests().is_empty());
        let _ = fs::remove_dir_all(temp_dir);
    }

    #[tokio::test]
    async fn handle_event_with_storage_surfaces_parse_error() {
        let invalid = RelayEvent {
            kind: KIND_GIT_REPO_ANNOUNCEMENT.0,
            event_id: "77".repeat(32),
            pubkey: "11".repeat(32),
            created_at: 10,
            tags: Vec::new(),
        };
        let temp_dir = temp_dir("gittree-invalid-storage-event");
        let hooks = HookInstallConfig {
            pre_receive_source: temp_dir.join("pre-receive"),
            post_receive_source: temp_dir.join("post-receive"),
        };
        let (forgejo, transport) = forgejo_client_with_responses(Vec::new());
        let storage = InMemoryRepositories::new();
        let err =
            handle_announcement_event_with_storage(&temp_dir, &hooks, &storage, &forgejo, &invalid)
                .await
                .expect_err("parse error expected");
        assert!(matches!(err, super::CoordinatorEventError::Parse(_)));
        assert!(transport.requests().is_empty());
        let _ = fs::remove_dir_all(temp_dir);
    }

    #[test]
    fn handle_event_surfaces_parse_and_missing_npub_errors() {
        let invalid = RelayEvent {
            kind: KIND_GIT_REPO_ANNOUNCEMENT.0,
            event_id: "77".repeat(32),
            pubkey: "11".repeat(32),
            created_at: 10,
            tags: Vec::new(),
        };
        let temp_dir = temp_dir("gittree-invalid-event");
        let hooks = HookInstallConfig {
            pre_receive_source: temp_dir.join("pre-receive"),
            post_receive_source: temp_dir.join("post-receive"),
        };
        let parse_error = handle_announcement_event(&temp_dir, &hooks, &invalid)
            .expect_err("parse error expected");
        assert!(matches!(
            parse_error,
            super::CoordinatorEventError::Parse(_)
        ));

        let announcement = RepoAnnouncement {
            identifier: "repo".to_string(),
            name: None,
            description: None,
            root_commit: None,
            clone: vec!["https://relay.example/repo.git".to_string()],
            web: Vec::new(),
            relays: vec!["wss://relay.example".to_string()],
            blossoms: Vec::new(),
            hashtags: Vec::new(),
            maintainers: Vec::new(),
        };
        let missing_npub = RelayEvent {
            kind: KIND_GIT_REPO_ANNOUNCEMENT.0,
            event_id: "88".repeat(32),
            pubkey: "11".repeat(32),
            created_at: 10,
            tags: announcement.to_tags(),
        };
        let missing_npub_error = handle_announcement_event(&temp_dir, &hooks, &missing_npub)
            .expect_err("missing npub expected");
        assert!(matches!(
            missing_npub_error,
            super::CoordinatorEventError::MissingNpub
        ));
        let _ = fs::remove_dir_all(temp_dir);
    }

    #[test]
    fn repo_init_rejects_non_directory_and_missing_head() {
        let temp_dir = temp_dir("gittree-repo-init-errors");
        let repo_path = temp_dir.join("repo.git");
        fs::write(&repo_path, "not a dir").expect("repo file");
        let plan = sample_plan(repo_path.clone());
        let non_dir_error = super::init_repo(&plan).expect_err("non-directory should fail");
        assert!(matches!(
            non_dir_error,
            super::RepoInitError::InvalidRepo(_)
        ));

        fs::remove_file(&repo_path).expect("remove file");
        fs::create_dir_all(&repo_path).expect("repo dir");
        let missing_head_error = super::init_repo(&plan).expect_err("missing head should fail");
        assert!(matches!(
            missing_head_error,
            super::RepoInitError::InvalidRepo(_)
        ));
        let _ = fs::remove_dir_all(temp_dir);
    }

    #[test]
    fn repo_init_surfaces_git_init_and_config_failures() {
        let temp_dir = temp_dir("gittree-repo-init-git-errors");

        let parent_file = temp_dir.join("parent-file");
        fs::write(&parent_file, "not a directory").expect("parent file");
        let git_init_plan = sample_plan(parent_file.join("repo.git"));
        let git_init_error = super::init_repo(&git_init_plan).expect_err("git init should fail");
        assert!(matches!(git_init_error, super::RepoInitError::Git(_)));
        assert!(format!("{git_init_error}").contains("git init failed"));

        let mut git_config_plan = sample_plan(temp_dir.join("config-repo.git"));
        git_config_plan.git_config = vec![super::GitConfigEntry::new("bad key", "value")];
        let git_config_error =
            super::init_repo(&git_config_plan).expect_err("git config should fail");
        assert!(matches!(git_config_error, super::RepoInitError::Git(_)));
        assert!(format!("{git_config_error}").contains("git config failed"));

        let _ = fs::remove_dir_all(temp_dir);
    }

    #[test]
    fn error_display_and_source_cover_repo_hook_and_event_variants() {
        let io_error = std::io::Error::other("io failure");
        let repo_io = super::RepoInitError::Io(io_error);
        assert!(format!("{repo_io}").contains("repo init io error"));
        assert!(std::error::Error::source(&repo_io).is_some());

        let repo_invalid = super::RepoInitError::InvalidRepo("invalid repo".to_string());
        assert_eq!(format!("{repo_invalid}"), "invalid repo");
        assert!(std::error::Error::source(&repo_invalid).is_none());

        let repo_path = super::RepoInitError::InvalidPath("invalid path".to_string());
        assert_eq!(format!("{repo_path}"), "invalid path");
        assert!(std::error::Error::source(&repo_path).is_none());

        let repo_git = super::RepoInitError::Git("git failed".to_string());
        assert_eq!(format!("{repo_git}"), "git failed");
        assert!(std::error::Error::source(&repo_git).is_none());

        let hook_io = super::HookInstallError::Io(std::io::Error::other("hook io"));
        assert!(format!("{hook_io}").contains("hook install io error"));
        assert!(std::error::Error::source(&hook_io).is_some());

        let hook_missing = super::HookInstallError::MissingSource("/tmp/missing".to_string());
        assert_eq!(
            format!("{hook_missing}"),
            "missing hook source: /tmp/missing"
        );
        assert!(std::error::Error::source(&hook_missing).is_none());

        let plan_error = super::ProvisionPlanError::InvalidRepo("invalid plan".to_string());
        assert_eq!(format!("{plan_error}"), "invalid plan");
        assert!(std::error::Error::source(&plan_error).is_none());

        let parse = super::CoordinatorEventError::Parse("parse".to_string());
        assert_eq!(format!("{parse}"), "parse");
        assert!(std::error::Error::source(&parse).is_none());

        let missing_npub = super::CoordinatorEventError::MissingNpub;
        assert_eq!(format!("{missing_npub}"), "missing npub in clone urls");
        assert!(std::error::Error::source(&missing_npub).is_none());

        let plan = super::CoordinatorEventError::Plan(super::ProvisionPlanError::InvalidRepo(
            "plan".to_string(),
        ));
        assert_eq!(format!("{plan}"), "plan");
        assert!(std::error::Error::source(&plan).is_some());

        let init = super::CoordinatorEventError::Init(super::RepoInitError::InvalidRepo(
            "init".to_string(),
        ));
        assert_eq!(format!("{init}"), "init");
        assert!(std::error::Error::source(&init).is_some());

        let hooks = super::CoordinatorEventError::Hooks(super::HookInstallError::MissingSource(
            "/tmp/hook".to_string(),
        ));
        assert_eq!(format!("{hooks}"), "missing hook source: /tmp/hook");
        assert!(std::error::Error::source(&hooks).is_some());

        let storage = super::CoordinatorEventError::Storage(StorageError::Internal {
            message: "storage".to_string(),
        });
        assert_eq!(
            format!("{storage}"),
            "storage error: internal error: storage"
        );
        assert!(std::error::Error::source(&storage).is_some());

        let forgejo = super::CoordinatorEventError::Forgejo(
            gittree_forgejo::ForgejoError::Request("forgejo".to_string()),
        );
        assert_eq!(format!("{forgejo}"), "forgejo request error: forgejo");
        assert!(std::error::Error::source(&forgejo).is_some());

        let mapping = super::CoordinatorEventError::Mapping(gittree_core::CoreError::MissingField(
            "identifier",
        ));
        assert_eq!(format!("{mapping}"), "missing required field: identifier");
        assert!(std::error::Error::source(&mapping).is_some());
    }

    #[tokio::test]
    async fn install_hooks_and_mock_transport_cover_missing_source_and_response() {
        let temp_dir = temp_dir("gittree-hook-errors");
        let plan = sample_plan(temp_dir.join("repo.git"));
        let config = HookInstallConfig {
            pre_receive_source: temp_dir.join("missing-pre"),
            post_receive_source: temp_dir.join("missing-post"),
        };
        let hook_error = super::install_hooks(&plan, &config).expect_err("missing source");
        assert!(matches!(
            hook_error,
            super::HookInstallError::MissingSource(_)
        ));

        let transport = MockTransport::default();
        let request = ForgejoRequest {
            method: ForgejoMethod::Get,
            url: "http://localhost:3000/api/v1/repos/gittree/repo".to_string(),
            body: None,
        };
        let response_error = transport.send(request).await.expect_err("missing response");
        assert!(matches!(
            response_error,
            gittree_forgejo::ForgejoError::Request(message)
            if message.contains("missing mock response")
        ));
        let _ = fs::remove_dir_all(temp_dir);
    }

    #[tokio::test]
    async fn finalize_outbox_job_ignores_non_announcement_event_kinds() {
        let repositories = Arc::new(InMemoryRepositories::new());
        let temp_dir = temp_dir("gittree-finalize-ignored");
        let hooks = HookInstallConfig {
            pre_receive_source: temp_dir.join("pre-receive"),
            post_receive_source: temp_dir.join("post-receive"),
        };
        let (forgejo, transport) = forgejo_client_with_responses(Vec::new());
        let state = super::CoordinatorAppState {
            repositories,
            repo_root: temp_dir.join("repos"),
            hooks,
            forgejo,
        };
        let list = UserGraspList {
            urls: vec!["wss://relay.example".to_string()],
        };
        let job = sample_publish_job(KIND_USER_GRASP_LIST.0, list.to_tags(), "repo");
        super::finalize_outbox_job(&state, &job)
            .await
            .expect("ignored event should succeed");
        assert!(transport.requests().is_empty());
        let _ = fs::remove_dir_all(temp_dir);
    }

    #[tokio::test]
    async fn finalize_outbox_job_persists_state_and_provisions_repo() {
        let repositories = Arc::new(InMemoryRepositories::new());
        let temp_dir = temp_dir("gittree-finalize-success");
        let bin_dir = temp_dir.join("bin");
        fs::create_dir_all(&bin_dir).expect("bin dir");
        let pre_source = bin_dir.join("pre-receive");
        let post_source = bin_dir.join("post-receive");
        fs::write(&pre_source, "#!/bin/sh\necho pre\n").expect("pre hook");
        fs::write(&post_source, "#!/bin/sh\necho post\n").expect("post hook");
        let hooks = HookInstallConfig {
            pre_receive_source: pre_source,
            post_receive_source: post_source,
        };

        let forgejo_responses = vec![
            ForgejoResponse {
                status: 404,
                body: "not found".to_string(),
            },
            ForgejoResponse {
                status: 201,
                body: repo_json("owner", "repo"),
            },
            ForgejoResponse {
                status: 200,
                body: "[]".to_string(),
            },
            ForgejoResponse {
                status: 201,
                body: "created".to_string(),
            },
        ];
        let (forgejo, transport) = forgejo_client_with_responses(forgejo_responses);
        let repo_root = temp_dir.join("repos");
        let state = super::CoordinatorAppState {
            repositories: repositories.clone(),
            repo_root: repo_root.clone(),
            hooks,
            forgejo,
        };

        let npub = "npub1gjttreegkzys8jlhdnfm3qe39h2gka79cpndd0jsms5fk7tuhcnsdw56jq";
        let announcement = RepoAnnouncement {
            identifier: "repo".to_string(),
            name: None,
            description: Some("repo description".to_string()),
            root_commit: None,
            clone: vec![format!("https://gittr.ee/{npub}/repo.git")],
            web: Vec::new(),
            relays: vec!["wss://gittr.ee".to_string()],
            blossoms: Vec::new(),
            hashtags: Vec::new(),
            maintainers: Vec::new(),
        };
        let job = sample_publish_job(
            KIND_GIT_REPO_ANNOUNCEMENT.0,
            announcement.to_tags(),
            &announcement.identifier,
        );

        super::finalize_outbox_job(&state, &job)
            .await
            .expect("finalize succeeds");
        assert_eq!(transport.requests().len(), 4);

        let stored = repositories
            .latest_announcement(&job.pubkey, &announcement.identifier)
            .await
            .expect("latest");
        assert!(stored.is_some());

        let mapping = repositories
            .mapping_by_repo(&job.pubkey, &announcement.identifier)
            .await
            .expect("mapping");
        let mapping = mapping.expect("mapping stored");
        assert_eq!(mapping.forgejo_owner, "owner");
        assert_eq!(mapping.forgejo_repo, "repo");
        assert!(repo_root.join(npub).join("repo.git").exists());
        let _ = fs::remove_dir_all(temp_dir);
    }

    #[tokio::test]
    async fn serve_with_init_returns_bind_error_after_state_setup() {
        let mut config = sample_coordinator_config(sample_storage_config(
            "postgres://user:pass@localhost:5432/gittree",
        ));
        config.bind = "invalid-bind".to_string();
        let err = super::serve_with_init(config, || Ok(()))
            .await
            .expect_err("bind error");
        assert!(matches!(err, super::CoordinatorError::Serve(_)));
    }

    #[tokio::test]
    async fn serve_with_init_runs_server_until_cancelled() {
        let mut config = sample_coordinator_config(sample_storage_config(
            "postgres://user:pass@localhost:5432/gittree",
        ));
        config.bind = "127.0.0.1:0".to_string();
        let task = tokio::spawn(super::serve_with_init(config, || Ok(())));
        tokio::time::sleep(StdDuration::from_millis(20)).await;
        task.abort();
        let _ = task.await;
    }

    #[test]
    fn serve_maps_observability_config_errors() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        with_env_var("GITTREE_METRICS_ENABLED", "invalid-bool", || {
            let config = sample_coordinator_config(sample_storage_config(
                "postgres://user:pass@localhost:5432/gittree",
            ));
            let err = runtime
                .block_on(super::serve(config))
                .expect_err("observability config");
            assert!(matches!(
                err,
                super::CoordinatorError::ObservabilityConfig(_)
            ));
        });
    }

    #[tokio::test]
    async fn run_http_server_with_shutdown_returns_ok() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind listener");
        let result = super::run_http_server_with_shutdown(listener, Router::new(), async {}).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn run_http_server_with_pending_shutdown_can_be_aborted() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind listener");
        let task = tokio::spawn(super::run_http_server_with_shutdown(
            listener,
            Router::new(),
            std::future::pending(),
        ));
        tokio::time::sleep(StdDuration::from_millis(20)).await;
        task.abort();
        let join_err = task.await.expect_err("aborted");
        assert!(join_err.is_cancelled());
    }

    #[tokio::test]
    async fn spawn_publish_outbox_starts_loop() {
        let repositories = Arc::new(InMemoryRepositories::new());
        let temp_dir = temp_dir("gittree-publish-loop-starts");
        let hooks = HookInstallConfig {
            pre_receive_source: temp_dir.join("pre-receive"),
            post_receive_source: temp_dir.join("post-receive"),
        };
        let (forgejo, _transport) = forgejo_client_with_responses(Vec::new());
        let state = super::CoordinatorAppState {
            repositories,
            repo_root: temp_dir.join("repos"),
            hooks,
            forgejo,
        };
        let task = super::spawn_publish_outbox(state);
        tokio::time::sleep(StdDuration::from_millis(20)).await;
        task.abort();
        let _ = task.await;
        let _ = fs::remove_dir_all(temp_dir);
    }

    #[tokio::test]
    async fn publish_outbox_loop_with_delay_and_publish_handles_claim_errors() {
        let repositories = Arc::new(ScriptedOutboxRepositories::with_flags(
            true, false, false, false,
        ));
        let temp_dir = temp_dir("gittree-publish-loop-claim-error");
        let hooks = HookInstallConfig {
            pre_receive_source: temp_dir.join("pre-receive"),
            post_receive_source: temp_dir.join("post-receive"),
        };
        let (forgejo, _transport) = forgejo_client_with_responses(Vec::new());
        let state = super::CoordinatorAppState {
            repositories,
            repo_root: temp_dir.join("repos"),
            hooks,
            forgejo,
        };
        let task = tokio::spawn(super::publish_outbox_loop_with_delay_and_publish(
            state,
            StdDuration::from_millis(1),
            publish_ok,
        ));
        tokio::time::sleep(StdDuration::from_millis(20)).await;
        task.abort();
        let _ = task.await;
        let _ = fs::remove_dir_all(temp_dir);
    }

    #[tokio::test]
    async fn publish_outbox_loop_with_delay_and_publish_handles_mark_succeeded_errors() {
        let repositories = Arc::new(ScriptedOutboxRepositories::with_flags(
            false, true, false, false,
        ));
        repositories
            .enqueue_relay_publish(sample_publish_request(
                "wss://relay.example",
                KIND_GIT_REPO_ANNOUNCEMENT.0,
                vec![vec!["d".to_string(), "repo".to_string()]],
            ))
            .await
            .expect("enqueue");
        let temp_dir = temp_dir("gittree-publish-loop-mark-succeeded-error");
        let hooks = HookInstallConfig {
            pre_receive_source: temp_dir.join("pre-receive"),
            post_receive_source: temp_dir.join("post-receive"),
        };
        let (forgejo, _transport) = forgejo_client_with_responses(Vec::new());
        let state = super::CoordinatorAppState {
            repositories,
            repo_root: temp_dir.join("repos"),
            hooks,
            forgejo,
        };
        let task = tokio::spawn(super::publish_outbox_loop_with_delay_and_publish(
            state,
            StdDuration::from_millis(1),
            publish_ok,
        ));
        tokio::time::sleep(StdDuration::from_millis(20)).await;
        task.abort();
        let _ = task.await;
        let _ = fs::remove_dir_all(temp_dir);
    }

    #[tokio::test]
    async fn publish_outbox_loop_with_delay_and_publish_handles_pending_count_errors() {
        let repositories = Arc::new(ScriptedOutboxRepositories::with_flags(
            false, false, true, false,
        ));
        repositories
            .enqueue_relay_publish(sample_publish_request(
                "wss://relay.example",
                KIND_GIT_REPO_ANNOUNCEMENT.0,
                vec![vec!["d".to_string(), "repo".to_string()]],
            ))
            .await
            .expect("enqueue");
        let temp_dir = temp_dir("gittree-publish-loop-pending-error");
        let hooks = HookInstallConfig {
            pre_receive_source: temp_dir.join("pre-receive"),
            post_receive_source: temp_dir.join("post-receive"),
        };
        let (forgejo, _transport) = forgejo_client_with_responses(Vec::new());
        let state = super::CoordinatorAppState {
            repositories,
            repo_root: temp_dir.join("repos"),
            hooks,
            forgejo,
        };
        let task = tokio::spawn(super::publish_outbox_loop_with_delay_and_publish(
            state,
            StdDuration::from_millis(1),
            publish_ok,
        ));
        tokio::time::sleep(StdDuration::from_millis(20)).await;
        task.abort();
        let _ = task.await;
        let _ = fs::remove_dir_all(temp_dir);
    }

    #[tokio::test]
    async fn publish_outbox_loop_with_delay_and_publish_handles_mark_failed_errors() {
        let repositories = Arc::new(ScriptedOutboxRepositories::with_flags(
            false, false, false, true,
        ));
        repositories
            .enqueue_relay_publish(sample_publish_request(
                "wss://relay.example",
                KIND_GIT_REPO_ANNOUNCEMENT.0,
                vec![vec!["d".to_string(), "repo".to_string()]],
            ))
            .await
            .expect("enqueue");
        let temp_dir = temp_dir("gittree-publish-loop-mark-failed-error");
        let hooks = HookInstallConfig {
            pre_receive_source: temp_dir.join("pre-receive"),
            post_receive_source: temp_dir.join("post-receive"),
        };
        let (forgejo, _transport) = forgejo_client_with_responses(Vec::new());
        let state = super::CoordinatorAppState {
            repositories,
            repo_root: temp_dir.join("repos"),
            hooks,
            forgejo,
        };
        let task = tokio::spawn(super::publish_outbox_loop_with_delay_and_publish(
            state,
            StdDuration::from_millis(1),
            publish_err,
        ));
        tokio::time::sleep(StdDuration::from_millis(20)).await;
        task.abort();
        let _ = task.await;
        let _ = fs::remove_dir_all(temp_dir);
    }

    #[tokio::test]
    async fn publish_outbox_loop_with_delay_and_publish_handles_pending_jobs() {
        let repositories = Arc::new(ScriptedOutboxRepositories::default());
        let request = sample_publish_request(
            "wss://relay.example",
            KIND_GIT_REPO_ANNOUNCEMENT.0,
            vec![vec!["d".to_string(), "repo".to_string()]],
        );
        repositories
            .enqueue_relay_publish(request.clone())
            .await
            .expect("enqueue first");
        repositories
            .enqueue_relay_publish(request)
            .await
            .expect("enqueue second");
        let temp_dir = temp_dir("gittree-publish-loop-pending-jobs");
        let hooks = HookInstallConfig {
            pre_receive_source: temp_dir.join("pre-receive"),
            post_receive_source: temp_dir.join("post-receive"),
        };
        let (forgejo, _transport) = forgejo_client_with_responses(Vec::new());
        let state = super::CoordinatorAppState {
            repositories: repositories.clone(),
            repo_root: temp_dir.join("repos"),
            hooks,
            forgejo,
        };
        let task = tokio::spawn(super::publish_outbox_loop_with_delay_and_publish(
            state,
            StdDuration::from_millis(1),
            publish_ok,
        ));
        tokio::time::sleep(StdDuration::from_millis(20)).await;
        task.abort();
        let _ = task.await;
        let _ = fs::remove_dir_all(temp_dir);
    }

    #[tokio::test]
    async fn publish_outbox_loop_with_delay_and_publish_handles_finalize_errors() {
        let repositories = Arc::new(ScriptedOutboxRepositories::default());
        repositories
            .enqueue_relay_publish(sample_publish_request(
                "wss://relay.example",
                KIND_GIT_REPO_ANNOUNCEMENT.0,
                vec![vec!["invalid".to_string()]],
            ))
            .await
            .expect("enqueue");
        let temp_dir = temp_dir("gittree-publish-loop-finalize-error");
        let hooks = HookInstallConfig {
            pre_receive_source: temp_dir.join("pre-receive"),
            post_receive_source: temp_dir.join("post-receive"),
        };
        let (forgejo, _transport) = forgejo_client_with_responses(Vec::new());
        let state = super::CoordinatorAppState {
            repositories,
            repo_root: temp_dir.join("repos"),
            hooks,
            forgejo,
        };
        let task = tokio::spawn(super::publish_outbox_loop_with_delay_and_publish(
            state,
            StdDuration::from_millis(1),
            publish_ok,
        ));
        tokio::time::sleep(StdDuration::from_millis(20)).await;
        task.abort();
        let _ = task.await;
        let _ = fs::remove_dir_all(temp_dir);
    }

    #[tokio::test]
    async fn publish_outbox_loop_with_delay_and_publish_handles_finalize_success() {
        let repositories = Arc::new(ScriptedOutboxRepositories::default());
        repositories
            .enqueue_relay_publish(sample_publish_request(
                "wss://relay.example",
                1,
                vec![vec!["d".to_string(), "repo".to_string()]],
            ))
            .await
            .expect("enqueue");
        let temp_dir = temp_dir("gittree-publish-loop-finalize-ok");
        let hooks = HookInstallConfig {
            pre_receive_source: temp_dir.join("pre-receive"),
            post_receive_source: temp_dir.join("post-receive"),
        };
        let (forgejo, _transport) = forgejo_client_with_responses(Vec::new());
        let state = super::CoordinatorAppState {
            repositories,
            repo_root: temp_dir.join("repos"),
            hooks,
            forgejo,
        };
        let task = tokio::spawn(super::publish_outbox_loop_with_delay_and_publish(
            state,
            StdDuration::from_millis(1),
            publish_ok,
        ));
        tokio::time::sleep(StdDuration::from_millis(20)).await;
        task.abort();
        let _ = task.await;
        let _ = fs::remove_dir_all(temp_dir);
    }

    #[tokio::test]
    async fn publish_outbox_loop_runs_until_cancelled() {
        let repositories = Arc::new(InMemoryRepositories::new());
        let temp_dir = temp_dir("gittree-publish-loop-wrapper");
        let hooks = HookInstallConfig {
            pre_receive_source: temp_dir.join("pre-receive"),
            post_receive_source: temp_dir.join("post-receive"),
        };
        let (forgejo, _transport) = forgejo_client_with_responses(Vec::new());
        let state = super::CoordinatorAppState {
            repositories,
            repo_root: temp_dir.join("repos"),
            hooks,
            forgejo,
        };
        let result = tokio::time::timeout(
            StdDuration::from_millis(20),
            super::publish_outbox_loop_with_delay_and_publish(
                state,
                StdDuration::from_secs(super::OUTBOX_POLL_SECS),
                super::publish_to_relay,
            ),
        )
        .await;
        assert!(result.is_err());
        let _ = fs::remove_dir_all(temp_dir);
    }

    #[tokio::test]
    async fn publish_outbox_loop_with_delay_handles_empty_queue() {
        let repositories = Arc::new(InMemoryRepositories::new());
        let temp_dir = temp_dir("gittree-publish-loop-empty");
        let hooks = HookInstallConfig {
            pre_receive_source: temp_dir.join("pre-receive"),
            post_receive_source: temp_dir.join("post-receive"),
        };
        let (forgejo, _transport) = forgejo_client_with_responses(Vec::new());
        let state = super::CoordinatorAppState {
            repositories: repositories.clone(),
            repo_root: temp_dir.join("repos"),
            hooks,
            forgejo,
        };
        let result = tokio::time::timeout(
            StdDuration::from_millis(20),
            super::publish_outbox_loop_with_delay_and_publish(
                state,
                StdDuration::from_millis(1),
                super::publish_to_relay,
            ),
        )
        .await;
        assert!(result.is_err());

        let pending = repositories
            .pending_relay_publishes(&hex::decode("22".repeat(32)).expect("pubkey"), "repo", 1)
            .await
            .expect("pending");
        assert_eq!(pending, 0);
        let _ = fs::remove_dir_all(temp_dir);
    }

    #[tokio::test]
    async fn publish_outbox_loop_with_delay_marks_failed_publish_for_invalid_relay_url() {
        let repositories = Arc::new(InMemoryRepositories::new());
        repositories
            .enqueue_relay_publish(sample_publish_request(
                "not-a-url",
                KIND_GIT_REPO_ANNOUNCEMENT.0,
                vec![vec!["d".to_string(), "repo".to_string()]],
            ))
            .await
            .expect("enqueue");

        let temp_dir = temp_dir("gittree-publish-loop-failed");
        let hooks = HookInstallConfig {
            pre_receive_source: temp_dir.join("pre-receive"),
            post_receive_source: temp_dir.join("post-receive"),
        };
        let (forgejo, _transport) = forgejo_client_with_responses(Vec::new());
        let state = super::CoordinatorAppState {
            repositories: repositories.clone(),
            repo_root: temp_dir.join("repos"),
            hooks,
            forgejo,
        };
        let task = tokio::spawn(super::publish_outbox_loop_with_delay_and_publish(
            state,
            StdDuration::from_millis(1),
            super::publish_to_relay,
        ));
        tokio::time::sleep(StdDuration::from_millis(100)).await;
        task.abort();
        let _ = task.await;

        let claim_time =
            OffsetDateTime::now_utc() + TimeDuration::seconds(super::OUTBOX_RETRY_BASE_SECS + 5);
        let claimed = repositories
            .claim_relay_publish(claim_time)
            .await
            .expect("claim after retry");
        let claimed = claimed.expect("failed publish should be re-queued");
        assert!(claimed.attempt_count >= 2);
        assert_eq!(claimed.relay_url, "not-a-url");
        let _ = fs::remove_dir_all(temp_dir);
    }

    #[test]
    fn env_helpers_restore_previous_values_for_set_and_unset_paths() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        let key = "GITTREE_COORDINATOR_ENV_HELPER_TEST";

        with_unset_env_var(key, || {
            with_env_var(key, "value", || {
                assert_eq!(std::env::var(key).expect("set in closure"), "value");
            });
            assert!(std::env::var_os(key).is_none());
        });

        with_env_var(key, "original", || {
            with_env_var(key, "temporary", || {
                assert_eq!(std::env::var(key).expect("temporary value"), "temporary");
            });
            assert_eq!(std::env::var(key).expect("restored original"), "original");

            with_unset_env_var(key, || {
                assert!(std::env::var_os(key).is_none());
            });
            assert_eq!(
                std::env::var(key).expect("restored after unset"),
                "original"
            );
        });
    }

    #[tokio::test]
    async fn handle_event_with_storage_persists_announcement() {
        let npub = "npub1gjttreegkzys8jlhdnfm3qe39h2gka79cpndd0jsms5fk7tuhcnsdw56jq";
        let announcement = RepoAnnouncement {
            identifier: "repo".to_string(),
            name: None,
            description: None,
            root_commit: None,
            clone: vec![format!("https://gittr.ee/{npub}/repo.git")],
            web: Vec::new(),
            relays: vec!["wss://gittr.ee".to_string()],
            blossoms: Vec::new(),
            hashtags: Vec::new(),
            maintainers: Vec::new(),
        };
        let event = RelayEvent {
            kind: KIND_GIT_REPO_ANNOUNCEMENT.0,
            event_id: "33".repeat(32),
            pubkey: "11".repeat(32),
            created_at: 10,
            tags: announcement.to_tags(),
        };
        let forgejo_repo = super::forgejo_repo_name(&announcement.identifier, &event.pubkey);
        let forgejo_responses = vec![
            ForgejoResponse {
                status: 404,
                body: "not found".to_string(),
            },
            ForgejoResponse {
                status: 201,
                body: repo_json("gittree", &forgejo_repo),
            },
            ForgejoResponse {
                status: 200,
                body: "[]".to_string(),
            },
            ForgejoResponse {
                status: 201,
                body: "created".to_string(),
            },
        ];
        let (forgejo, transport) = forgejo_client_with_responses(forgejo_responses);
        let storage = InMemoryRepositories::new();
        let temp_dir = temp_dir("gittree-event-storage");
        let bin_dir = temp_dir.join("bin");
        fs::create_dir_all(&bin_dir).expect("bin dir");
        let pre_source = bin_dir.join("pre-receive");
        let post_source = bin_dir.join("post-receive");
        fs::write(&pre_source, "#!/bin/sh\necho pre\n").expect("pre hook");
        fs::write(&post_source, "#!/bin/sh\necho post\n").expect("post hook");
        let hooks = HookInstallConfig {
            pre_receive_source: pre_source,
            post_receive_source: post_source,
        };
        let repo_root = temp_dir.join("repos");
        let action =
            handle_announcement_event_with_storage(&repo_root, &hooks, &storage, &forgejo, &event)
                .await
                .expect("handle");
        assert!(matches!(action, CoordinatorAction::Provisioned { .. }));
        assert_eq!(transport.requests().len(), 4);
        let pubkey_bytes = hex::decode(&event.pubkey).expect("decode");
        let stored = storage
            .latest_announcement(&pubkey_bytes, &announcement.identifier)
            .await
            .expect("latest");
        assert!(stored.is_some());
        let mapping = storage
            .mapping_by_repo(&pubkey_bytes, &announcement.identifier)
            .await
            .expect("mapping");
        let mapping = mapping.expect("mapping stored");
        assert_eq!(mapping.forgejo_repo, forgejo_repo);
        let _ = fs::remove_dir_all(temp_dir);
    }

    #[tokio::test]
    async fn health_endpoint_returns_ok() {
        let repositories = std::sync::Arc::new(InMemoryRepositories::new());
        let temp_dir = temp_dir("gittree-coordinator-health");
        let bin_dir = temp_dir.join("bin");
        fs::create_dir_all(&bin_dir).expect("bin dir");
        let hooks = HookInstallConfig {
            pre_receive_source: bin_dir.join("pre-receive"),
            post_receive_source: bin_dir.join("post-receive"),
        };
        let (forgejo, _transport) = forgejo_client_with_responses(Vec::new());
        let app = super::build_router(super::CoordinatorAppState {
            repositories,
            repo_root: temp_dir.join("repos"),
            hooks,
            forgejo,
        });
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        let _ = fs::remove_dir_all(temp_dir);
    }

    #[tokio::test]
    async fn announcement_endpoint_provisions_repo() {
        let repositories = std::sync::Arc::new(InMemoryRepositories::new());
        let temp_dir = temp_dir("gittree-coordinator-http");
        let bin_dir = temp_dir.join("bin");
        fs::create_dir_all(&bin_dir).expect("bin dir");
        let pre_source = bin_dir.join("pre-receive");
        let post_source = bin_dir.join("post-receive");
        fs::write(&pre_source, "#!/bin/sh\necho pre\n").expect("pre hook");
        fs::write(&post_source, "#!/bin/sh\necho post\n").expect("post hook");
        let hooks = HookInstallConfig {
            pre_receive_source: pre_source,
            post_receive_source: post_source,
        };
        let repo_root = temp_dir.join("repos");
        let pubkey = "11".repeat(32);
        let forgejo_repo = super::forgejo_repo_name("repo", &pubkey);
        let forgejo_responses = vec![
            ForgejoResponse {
                status: 404,
                body: "not found".to_string(),
            },
            ForgejoResponse {
                status: 201,
                body: repo_json("gittree", &forgejo_repo),
            },
            ForgejoResponse {
                status: 200,
                body: "[]".to_string(),
            },
            ForgejoResponse {
                status: 201,
                body: "created".to_string(),
            },
        ];
        let (forgejo, _transport) = forgejo_client_with_responses(forgejo_responses);
        let app = super::build_router(super::CoordinatorAppState {
            repositories: repositories.clone(),
            repo_root: repo_root.clone(),
            hooks,
            forgejo,
        });

        let npub = "npub1gjttreegkzys8jlhdnfm3qe39h2gka79cpndd0jsms5fk7tuhcnsdw56jq";
        let announcement = RepoAnnouncement {
            identifier: "repo".to_string(),
            name: None,
            description: None,
            root_commit: None,
            clone: vec![format!("https://gittr.ee/{npub}/repo.git")],
            web: Vec::new(),
            relays: vec!["wss://gittr.ee".to_string()],
            blossoms: Vec::new(),
            hashtags: Vec::new(),
            maintainers: Vec::new(),
        };
        let pubkey_bytes = hex::decode(&pubkey).expect("pubkey");
        let payload = CoordinatorEventPayload {
            kind: KIND_GIT_REPO_ANNOUNCEMENT.0 as u64,
            event_id: "44".repeat(32),
            pubkey,
            created_at: 10,
            tags: announcement.to_tags(),
        };
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/announcement")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&payload).expect("body")))
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let action: CoordinatorActionPayload = serde_json::from_slice(&body).expect("action");
        assert!(matches!(
            action,
            CoordinatorActionPayload::Provisioned { .. }
        ));
        assert!(repo_root.join(npub).join("repo.git").exists());
        let mapping = repositories
            .mapping_by_repo(&pubkey_bytes, "repo")
            .await
            .expect("mapping");
        let mapping = mapping.expect("mapping stored");
        assert_eq!(mapping.forgejo_repo, forgejo_repo);
        let _ = fs::remove_dir_all(temp_dir);
    }

    #[test]
    fn observability_init_returns_registry() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_unset_env_var("GITTREE_LOG_JSON", || {
            if OBSERVABILITY.get().is_none() {
                let _ = OBSERVABILITY.set(init_observability().expect("init"));
            }
            let handle = OBSERVABILITY.get().expect("observability handle");
            assert!(handle.prometheus_registry().is_some());
        });
    }

    #[test]
    fn observability_init_maps_invalid_env_and_reinit_errors() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var("GITTREE_LOG_JSON", "invalid-bool", || {
            let err = init_observability().expect_err("invalid observability config");
            assert!(matches!(
                err,
                super::CoordinatorError::ObservabilityConfig(_)
            ));
        });

        let _ = OBSERVABILITY.get_or_init(|| init_observability().expect("init"));
        let err = init_observability().expect_err("reinit should fail");
        assert!(matches!(err, super::CoordinatorError::Observability(_)));
    }

    #[test]
    fn signed_event_from_job_encodes_fields() {
        let job = sample_job();
        let signed = super::signed_event_from_job(&job);
        assert_eq!(signed.id, "11".repeat(32));
        assert_eq!(signed.pubkey, "22".repeat(32));
        assert_eq!(signed.sig, "33".repeat(64));
        assert_eq!(signed.kind, job.kind);
        assert_eq!(signed.tags, job.tags);
    }

    #[test]
    fn relay_event_from_job_encodes_fields() {
        let job = sample_job();
        let event = super::relay_event_from_job(&job);
        assert_eq!(event.event_id, "11".repeat(32));
        assert_eq!(event.pubkey, "22".repeat(32));
        assert_eq!(event.kind, job.kind);
    }

    #[test]
    fn retry_after_scales_and_caps() {
        let now = OffsetDateTime::from_unix_timestamp(0).expect("ts");
        let first = super::retry_after(now, 1);
        assert_eq!(
            first - now,
            TimeDuration::seconds(super::OUTBOX_RETRY_BASE_SECS)
        );
        let capped = super::retry_after(now, 999);
        assert_eq!(
            capped - now,
            TimeDuration::seconds(super::OUTBOX_RETRY_MAX_SECS)
        );
    }

    fn sample_job() -> RelayPublishJob {
        RelayPublishJob {
            id: 1,
            relay_url: "wss://relay.example".to_string(),
            event_id: vec![0x11; 32],
            pubkey: vec![0x22; 32],
            created_at: 42,
            kind: 30617,
            tags: vec![vec!["d".to_string(), "repo".to_string()]],
            content: "content".to_string(),
            sig: vec![0x33; 64],
            forgejo_owner: "owner".to_string(),
            forgejo_repo: "repo".to_string(),
            identifier: "repo".to_string(),
            attempt_count: 1,
            publish_after: OffsetDateTime::from_unix_timestamp(0).expect("ts"),
        }
    }

    fn temp_dir(prefix: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        let mut path = std::env::temp_dir();
        path.push(format!("{prefix}-{nanos}-{}", std::process::id()));
        std::fs::create_dir_all(&path).expect("create temp dir");
        path
    }
}
