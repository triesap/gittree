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
use gittree_storage::{
    AnnouncementRepository, PostgresRepositories, RepoAnnouncementRecord, RepoMappingRecord,
    RepoMappingRepository, StorageConfig, StorageError,
};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

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
    let value =
        std::env::var(key).map_err(|_| CoordinatorConfigError::MissingEnv(key))?;
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
    let pool_options = config.storage.pool_options().map_err(CoordinatorError::Storage)?;
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
    let _observability = init_observability()?;
    let repositories = build_repositories(&config)?;
    let forgejo =
        ForgejoClient::new(config.forgejo).map_err(CoordinatorError::Forgejo)?;
    let state = CoordinatorAppState {
        repositories: Arc::new(repositories),
        repo_root: config.repo_root,
        hooks: config.hooks,
        forgejo,
    };
    let router = build_router(state);
    let listener = tokio::net::TcpListener::bind(&config.bind)
        .await
        .map_err(|err| CoordinatorError::Serve(err.to_string()))?;
    axum::serve(listener, router)
        .await
        .map_err(|err| CoordinatorError::Serve(err.to_string()))?;
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
            CoordinatorAction::Provisioned { repo_path } => {
                CoordinatorActionPayload::Provisioned {
                    repo_path: repo_path.display().to_string(),
                }
            }
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
            CoordinatorHttpError::Internal(message) => {
                (StatusCode::INTERNAL_SERVER_ERROR, message)
            }
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
    parse_repo_path(&repo_path).map_err(|err| ProvisionPlanError::InvalidRepo(err.to_string()))?;
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
    use super::CoordinatorConfig;
    use super::ENV_STORAGE_READ_URL;
    use super::HookInstallConfig;
    use super::ObservabilityHandle;
    use super::RelayEvent;
    use super::handle_announcement_event_with_storage;
    use super::RepoAnnouncement;
    use super::build_provision_plan;
    use super::handle_announcement_event;
    use super::init_observability;
    use super::init_repo;
    use super::CoordinatorActionPayload;
    use super::CoordinatorEventPayload;
    use super::install_hooks;
    use async_trait::async_trait;
    use axum::body::{Body, to_bytes};
    use axum::http::Request;
    use gittree_config::ForgejoConfig;
    use gittree_core::kinds::KIND_GIT_REPO_ANNOUNCEMENT;
    use gittree_forgejo::{ForgejoClient, ForgejoRequest, ForgejoResponse, ForgejoTransport};
    use gittree_storage::{AnnouncementRepository, InMemoryRepositories, RepoMappingRepository};
    use std::collections::VecDeque;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex, OnceLock};
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
                    gittree_forgejo::ForgejoError::Request(
                        "missing mock response".to_string(),
                    )
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

    fn with_forgejo_envs<F: FnOnce()>(f: F) {
        with_env_var("GITTREE_FORGEJO_BASE_URL", "http://localhost:3000", || {
            with_env_var("GITTREE_FORGEJO_API_TOKEN", "token", || {
                with_env_var("GITTREE_FORGEJO_OWNER", "gittree", || {
                    with_env_var("GITTREE_FORGEJO_WEBHOOK_URL", "http://localhost:8090/", || {
                        with_env_var("GITTREE_FORGEJO_WEBHOOK_SECRET", "secret", f);
                    });
                });
            });
        });
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
                with_env_var(super::ENV_COORDINATOR_REPO_ROOT, "/tmp/gittree-repos", || {
                    with_env_var(super::ENV_COORDINATOR_PRE_RECEIVE_HOOK, "/tmp/pre", || {
                        with_env_var(super::ENV_COORDINATOR_POST_RECEIVE_HOOK, "/tmp/post", || {
                            with_env_var(super::ENV_STORAGE_IDLE_TIMEOUT_SECS, "", || {
                                with_env_var(super::ENV_STORAGE_MAX_LIFETIME_SECS, "", || {
                                    with_forgejo_envs(|| {
                                        let config =
                                            CoordinatorConfig::from_env().expect("config");
                                        assert_eq!(config.storage.idle_timeout_secs, None);
                                        assert_eq!(config.storage.max_lifetime_secs, None);
                                    });
                                });
                            });
                        });
                    });
                });
            },
        );
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
        let action = handle_announcement_event_with_storage(
            &repo_root,
            &hooks,
            &storage,
            &forgejo,
            &event,
        )
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
            .oneshot(Request::builder().uri("/health").body(Body::empty()).unwrap())
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
        let body = to_bytes(response.into_body(), usize::MAX).await.expect("body");
        let action: CoordinatorActionPayload =
            serde_json::from_slice(&body).expect("action");
        assert!(matches!(action, CoordinatorActionPayload::Provisioned { .. }));
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
        let handle = OBSERVABILITY.get_or_init(|| init_observability().expect("init"));
        assert!(handle.prometheus_registry().is_some());
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
