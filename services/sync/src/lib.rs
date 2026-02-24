use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use bech32::{Bech32, Hrp};
use gittree_config::{ConfigError, RelayTargetsConfig, ServicesConfig};
use gittree_core::nip34_common::RepoAddress;
use gittree_core::{NostrEvent, RepoState, collect_clone_urls};
use gittree_observability::{
    ObservabilityConfig, ObservabilityConfigError, ObservabilityError, ObservabilityHandle,
};
use gittree_storage::{RelayCompatibilityRecord, StorageConfig};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

const ENV_STORAGE_READ_URL: &str = "GITTREE_STORAGE_READ_URL";
const ENV_STORAGE_WRITE_URL: &str = "GITTREE_STORAGE_WRITE_URL";
const ENV_STORAGE_MAX_CONNECTIONS: &str = "GITTREE_STORAGE_MAX_CONNECTIONS";
const ENV_STORAGE_MIN_CONNECTIONS: &str = "GITTREE_STORAGE_MIN_CONNECTIONS";
const ENV_STORAGE_IDLE_TIMEOUT_SECS: &str = "GITTREE_STORAGE_IDLE_TIMEOUT_SECS";
const ENV_STORAGE_MAX_LIFETIME_SECS: &str = "GITTREE_STORAGE_MAX_LIFETIME_SECS";
const ENV_STORAGE_APP_NAME: &str = "GITTREE_STORAGE_APP_NAME";
const ENV_SYNC_REPO_ROOT: &str = "GITTREE_SYNC_REPO_ROOT";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncConfig {
    pub bind: String,
    pub storage: StorageConfig,
    pub relay_urls: Vec<String>,
    pub repo_root: PathBuf,
}

impl SyncConfig {
    pub fn from_env() -> Result<Self, SyncConfigError> {
        let services = ServicesConfig::from_env_validated().map_err(SyncConfigError::Config)?;
        let storage = storage_from_env()?;
        let relay_targets =
            RelayTargetsConfig::from_env_validated().map_err(SyncConfigError::Config)?;
        let repo_root = env_path(ENV_SYNC_REPO_ROOT)?;
        Ok(Self {
            bind: services.sync.bind,
            storage,
            relay_urls: relay_targets.relay_urls,
            repo_root,
        })
    }
}

#[derive(Debug)]
pub enum SyncConfigError {
    Config(ConfigError),
    Storage(StorageConfigError),
    MissingEnv(&'static str),
    InvalidEnv { key: &'static str, value: String },
}

impl std::fmt::Display for SyncConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SyncConfigError::Config(err) => write!(f, "sync config error: {err}"),
            SyncConfigError::Storage(err) => write!(f, "sync storage config error: {err}"),
            SyncConfigError::MissingEnv(key) => write!(f, "missing env {key}"),
            SyncConfigError::InvalidEnv { key, value } => write!(f, "invalid env {key}: {value}"),
        }
    }
}

impl std::error::Error for SyncConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            SyncConfigError::Config(err) => Some(err),
            SyncConfigError::Storage(err) => Some(err),
            SyncConfigError::MissingEnv(_) => None,
            SyncConfigError::InvalidEnv { .. } => None,
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

fn storage_from_env() -> Result<StorageConfig, SyncConfigError> {
    let read_connection = std::env::var(ENV_STORAGE_READ_URL).map_err(|_| {
        SyncConfigError::Storage(StorageConfigError::MissingEnv(ENV_STORAGE_READ_URL))
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
        SyncConfigError::Storage(StorageConfigError::InvalidConfig(err.to_string()))
    })?;

    Ok(config)
}

fn env_u32(key: &'static str) -> Result<Option<u32>, SyncConfigError> {
    match std::env::var(key) {
        Ok(value) => {
            if value.trim().is_empty() {
                return Ok(None);
            }
            value.parse::<u32>().map(Some).map_err(|_| {
                SyncConfigError::Storage(StorageConfigError::InvalidEnv { key, value })
            })
        }
        Err(_) => Ok(None),
    }
}

fn env_u64(key: &'static str) -> Result<Option<u64>, SyncConfigError> {
    match std::env::var(key) {
        Ok(value) => {
            if value.trim().is_empty() {
                return Ok(None);
            }
            value.parse::<u64>().map(Some).map_err(|_| {
                SyncConfigError::Storage(StorageConfigError::InvalidEnv { key, value })
            })
        }
        Err(_) => Ok(None),
    }
}

fn env_path(key: &'static str) -> Result<PathBuf, SyncConfigError> {
    let value = match std::env::var(key) {
        Ok(value) => value,
        Err(_) => return Err(SyncConfigError::MissingEnv(key)),
    };
    if value.trim().is_empty() {
        return Err(SyncConfigError::InvalidEnv { key, value });
    }
    Ok(PathBuf::from(value))
}

#[derive(Debug)]
pub enum SyncError {
    Config(SyncConfigError),
    ObservabilityConfig(ObservabilityConfigError),
    Observability(ObservabilityError),
    Serve(String),
}

impl std::fmt::Display for SyncError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SyncError::Config(err) => write!(f, "sync error: {err}"),
            SyncError::ObservabilityConfig(err) => {
                write!(f, "sync observability config error: {err}")
            }
            SyncError::Observability(err) => write!(f, "sync observability error: {err}"),
            SyncError::Serve(err) => write!(f, "sync serve error: {err}"),
        }
    }
}

impl std::error::Error for SyncError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            SyncError::Config(err) => Some(err),
            SyncError::ObservabilityConfig(err) => Some(err),
            SyncError::Observability(err) => Some(err),
            SyncError::Serve(_) => None,
        }
    }
}

pub fn init_observability() -> Result<ObservabilityHandle, SyncError> {
    let config = load_observability_config()?;
    let handle = gittree_observability::init(&config).map_err(SyncError::Observability)?;
    Ok(handle)
}

fn load_observability_config() -> Result<ObservabilityConfig, SyncError> {
    ObservabilityConfig::from_env("gittree-sync").map_err(SyncError::ObservabilityConfig)
}

struct SyncAppState {
    repo_root: PathBuf,
}

async fn serve_with<InitFn, InitOut, ServeFn, ServeFut>(
    config: SyncConfig,
    init_fn: InitFn,
    serve_fn: ServeFn,
) -> Result<(), SyncError>
where
    InitFn: FnOnce() -> Result<InitOut, SyncError>,
    ServeFn: FnOnce(tokio::net::TcpListener, Router) -> ServeFut,
    ServeFut: Future<Output = Result<(), std::io::Error>>,
{
    let _observability = init_fn()?;
    let state = SyncAppState {
        repo_root: config.repo_root,
    };
    let router = build_router(state);
    let listener = tokio::net::TcpListener::bind(&config.bind)
        .await
        .map_err(|err| SyncError::Serve(err.to_string()))?;
    serve_fn(listener, router)
        .await
        .map_err(|err| SyncError::Serve(err.to_string()))?;
    Ok(())
}

pub async fn serve(config: SyncConfig) -> Result<(), SyncError> {
    serve_with(config, init_observability, run_axum_server).await
}

fn run_axum_server(
    listener: tokio::net::TcpListener,
    router: Router,
) -> impl Future<Output = Result<(), std::io::Error>> {
    async move { axum::serve(listener, router).await }
}

fn build_router(state: SyncAppState) -> Router {
    Router::new()
        .route("/health", get(health_handler))
        .route("/", post(post_receive_handler))
        .with_state(Arc::new(state))
}

async fn health_handler() -> &'static str {
    "ok"
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PostReceivePayload {
    pub pubkey: String,
    pub identifier: String,
    pub updates: Vec<RefUpdatePayload>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefUpdatePayload {
    pub old: String,
    pub new: String,
    pub reference: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncAckPayload {
    pub repo_path: String,
    pub updates: usize,
}

#[derive(Debug)]
enum SyncHttpError {
    BadRequest(String),
    Internal(String),
}

impl IntoResponse for SyncHttpError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            SyncHttpError::BadRequest(message) => (StatusCode::BAD_REQUEST, message),
            SyncHttpError::Internal(message) => (StatusCode::INTERNAL_SERVER_ERROR, message),
        };
        (status, message).into_response()
    }
}

async fn post_receive_handler(
    State(state): State<Arc<SyncAppState>>,
    Json(payload): Json<PostReceivePayload>,
) -> Result<Json<SyncAckPayload>, SyncHttpError> {
    RepoAddress::new(payload.pubkey.clone(), payload.identifier.clone())
        .map_err(|err| SyncHttpError::BadRequest(format!("invalid repo address: {err}")))?;
    let npub = npub_from_hex(&payload.pubkey)?;
    let repo_path = state
        .repo_root
        .join(npub)
        .join(format!("{}.git", payload.identifier));
    Ok(Json(SyncAckPayload {
        repo_path: repo_path.display().to_string(),
        updates: payload.updates.len(),
    }))
}

fn npub_from_hex(pubkey: &str) -> Result<String, SyncHttpError> {
    if pubkey.len() != 64 {
        return Err(SyncHttpError::BadRequest("invalid pubkey".to_string()));
    }
    let bytes =
        hex::decode(pubkey).map_err(|_| SyncHttpError::BadRequest("invalid pubkey".to_string()))?;
    encode_npub_bytes(&bytes)
}

fn encode_npub_bytes(bytes: &[u8]) -> Result<String, SyncHttpError> {
    encode_npub_bytes_with(bytes, |hrp, payload| {
        bech32::encode::<Bech32>(hrp, payload).map_err(|_| ())
    })
}

fn encode_npub_bytes_with<F>(bytes: &[u8], encode: F) -> Result<String, SyncHttpError>
where
    F: FnOnce(Hrp, &[u8]) -> Result<String, ()>,
{
    let hrp = Hrp::parse("npub").expect("static npub hrp");
    encode(hrp, bytes).map_err(|_| SyncHttpError::Internal("npub encode failed".to_string()))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncPlan {
    pub identifier: String,
    pub clone_urls: Vec<String>,
    pub updates: Vec<RefUpdatePlan>,
    pub deletions: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefUpdatePlan {
    pub reference: String,
    pub target: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelaySelection {
    pub relays: Vec<String>,
    pub warnings: Vec<String>,
}

pub fn select_relay_urls(
    configured: &[String],
    compatibility: &[RelayCompatibilityRecord],
) -> RelaySelection {
    let mut compatible = Vec::new();
    for url in configured {
        if compatibility
            .iter()
            .any(|record| record.relay_url == *url && record.compatible)
        {
            compatible.push(url.clone());
        }
    }

    if !compatible.is_empty() {
        return RelaySelection {
            relays: compatible,
            warnings: Vec::new(),
        };
    }

    let mut warnings = Vec::new();
    if configured.is_empty() {
        warnings.push("no relay urls configured".to_string());
    } else {
        warnings.push("no compatible relays found; using configured list".to_string());
    }

    RelaySelection {
        relays: configured.to_vec(),
        warnings,
    }
}

pub fn build_sync_plan(
    state: &RepoState,
    local_refs: &HashMap<String, String>,
    events: &[NostrEvent],
    maintainers: &[String],
) -> SyncPlan {
    let desired = state.ref_map();
    let mut updates = Vec::new();
    for (reference, target) in &desired {
        match local_refs.get(reference) {
            Some(existing) if existing == target => {}
            _ => updates.push(RefUpdatePlan {
                reference: reference.clone(),
                target: target.clone(),
            }),
        }
    }

    let mut deletions = Vec::new();
    for reference in local_refs.keys() {
        if reference == "HEAD" {
            continue;
        }
        if !desired.contains_key(reference) {
            deletions.push(reference.clone());
        }
    }

    let clone_urls = collect_clone_urls(events, maintainers, &state.identifier);
    SyncPlan {
        identifier: state.identifier.clone(),
        clone_urls,
        updates,
        deletions,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncReport {
    pub remote_results: Vec<RemoteFetchResult>,
    pub update_results: Vec<RefChangeResult>,
    pub delete_results: Vec<RefChangeResult>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFetchResult {
    pub url: String,
    pub success: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefChangeResult {
    pub reference: String,
    pub success: bool,
    pub error: Option<String>,
}

pub trait GitExecutor {
    fn fetch(&self, repo_path: &Path, remote: &str, timeout: Duration) -> Result<(), GitExecError>;
    fn update_ref(
        &self,
        repo_path: &Path,
        reference: &str,
        target: &str,
    ) -> Result<(), GitExecError>;
    fn delete_ref(&self, repo_path: &Path, reference: &str) -> Result<(), GitExecError>;
}

pub struct CommandGitExecutor;

impl GitExecutor for CommandGitExecutor {
    fn fetch(&self, repo_path: &Path, remote: &str, timeout: Duration) -> Result<(), GitExecError> {
        let repo_path = path_to_str(repo_path)?;
        let mut command = std::process::Command::new("git");
        command
            .arg("-C")
            .arg(repo_path)
            .arg("fetch")
            .arg(remote)
            .arg("--prune");
        run_with_timeout(command, timeout)
    }

    fn update_ref(
        &self,
        repo_path: &Path,
        reference: &str,
        target: &str,
    ) -> Result<(), GitExecError> {
        let repo_path = path_to_str(repo_path)?;
        let mut command = std::process::Command::new("git");
        command
            .arg("-C")
            .arg(repo_path)
            .arg("update-ref")
            .arg(reference)
            .arg(target);
        run_with_timeout(command, Duration::from_secs(5))
    }

    fn delete_ref(&self, repo_path: &Path, reference: &str) -> Result<(), GitExecError> {
        let repo_path = path_to_str(repo_path)?;
        let mut command = std::process::Command::new("git");
        command
            .arg("-C")
            .arg(repo_path)
            .arg("update-ref")
            .arg("-d")
            .arg(reference);
        run_with_timeout(command, Duration::from_secs(5))
    }
}

#[derive(Debug)]
pub enum GitExecError {
    Io(std::io::Error),
    Timeout,
    InvalidPath(String),
    CommandFailed(String),
}

impl std::fmt::Display for GitExecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GitExecError::Io(err) => write!(f, "git io error: {err}"),
            GitExecError::Timeout => write!(f, "git command timed out"),
            GitExecError::InvalidPath(message) => write!(f, "{message}"),
            GitExecError::CommandFailed(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for GitExecError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            GitExecError::Io(err) => Some(err),
            GitExecError::Timeout => None,
            GitExecError::InvalidPath(_) => None,
            GitExecError::CommandFailed(_) => None,
        }
    }
}

pub fn execute_sync_plan<E: GitExecutor>(
    executor: &E,
    repo_path: &Path,
    plan: &SyncPlan,
    timeout: Duration,
) -> SyncReport {
    let mut remote_results = Vec::new();
    for remote in &plan.clone_urls {
        match executor.fetch(repo_path, remote, timeout) {
            Ok(()) => remote_results.push(RemoteFetchResult {
                url: remote.clone(),
                success: true,
                error: None,
            }),
            Err(err) => remote_results.push(RemoteFetchResult {
                url: remote.clone(),
                success: false,
                error: Some(err.to_string()),
            }),
        }
    }

    let mut update_results = Vec::new();
    for update in &plan.updates {
        match executor.update_ref(repo_path, &update.reference, &update.target) {
            Ok(()) => update_results.push(RefChangeResult {
                reference: update.reference.clone(),
                success: true,
                error: None,
            }),
            Err(err) => update_results.push(RefChangeResult {
                reference: update.reference.clone(),
                success: false,
                error: Some(err.to_string()),
            }),
        }
    }

    let mut delete_results = Vec::new();
    for reference in &plan.deletions {
        match executor.delete_ref(repo_path, reference) {
            Ok(()) => delete_results.push(RefChangeResult {
                reference: reference.clone(),
                success: true,
                error: None,
            }),
            Err(err) => delete_results.push(RefChangeResult {
                reference: reference.clone(),
                success: false,
                error: Some(err.to_string()),
            }),
        }
    }

    SyncReport {
        remote_results,
        update_results,
        delete_results,
    }
}

fn path_to_str(path: &Path) -> Result<&str, GitExecError> {
    path.to_str()
        .ok_or_else(|| GitExecError::InvalidPath("repo path is not utf-8".to_string()))
}

fn run_with_timeout(
    mut command: std::process::Command,
    timeout: Duration,
) -> Result<(), GitExecError> {
    let mut child = command.spawn().map_err(GitExecError::Io)?;
    let start = Instant::now();
    loop {
        if let Some(status) = child.try_wait().map_err(GitExecError::Io)? {
            if status.success() {
                return Ok(());
            }
            return Err(GitExecError::CommandFailed(format!(
                "git command failed with status {status}"
            )));
        }
        if start.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(GitExecError::Timeout);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[derive(Debug, Clone)]
pub struct SyncScheduleConfig {
    pub interval: Duration,
    pub max_backoff: Duration,
    pub max_concurrent: usize,
}

impl Default for SyncScheduleConfig {
    fn default() -> Self {
        Self {
            interval: Duration::from_secs(60),
            max_backoff: Duration::from_secs(300),
            max_concurrent: 4,
        }
    }
}

#[derive(Debug)]
pub struct SyncScheduler {
    config: SyncScheduleConfig,
    failure_streak: u32,
    limiter: SyncLimiter,
}

impl SyncScheduler {
    pub fn new(config: SyncScheduleConfig) -> Self {
        let limiter = SyncLimiter::new(config.max_concurrent);
        Self {
            config,
            failure_streak: 0,
            limiter,
        }
    }

    pub fn next_delay(&mut self, last_success: bool) -> Duration {
        if last_success {
            self.failure_streak = 0;
            return self.config.interval;
        }

        self.failure_streak = self.failure_streak.saturating_add(1);
        let base = self.config.interval.as_secs_f64();
        let multiplier = 2f64.powi(self.failure_streak as i32);
        let backoff = base * multiplier;
        let capped = backoff.min(self.config.max_backoff.as_secs_f64());
        Duration::from_secs_f64(capped)
    }

    pub fn try_start_repo(&mut self, repo_key: &str) -> bool {
        self.limiter.try_start(repo_key)
    }

    pub fn finish_repo(&mut self, repo_key: &str) {
        self.limiter.finish(repo_key);
    }
}

#[derive(Debug)]
struct SyncLimiter {
    max_concurrent: usize,
    active: HashSet<String>,
}

impl SyncLimiter {
    fn new(max_concurrent: usize) -> Self {
        Self {
            max_concurrent: max_concurrent.max(1),
            active: HashSet::new(),
        }
    }

    fn try_start(&mut self, repo_key: &str) -> bool {
        if self.active.contains(repo_key) {
            return false;
        }
        if self.active.len() >= self.max_concurrent {
            return false;
        }
        self.active.insert(repo_key.to_string());
        true
    }

    fn finish(&mut self, repo_key: &str) {
        self.active.remove(repo_key);
    }
}

#[cfg(test)]
mod tests {
    use super::CommandGitExecutor;
    use super::ENV_STORAGE_READ_URL;
    use super::GitExecError;
    use super::GitExecutor;
    use super::PostReceivePayload;
    use super::RefChangeResult;
    use super::RefUpdatePayload;
    use super::RefUpdatePlan;
    use super::StorageConfigError;
    use super::SyncAckPayload;
    use super::SyncConfig;
    use super::SyncConfigError;
    use super::SyncError;
    use super::SyncHttpError;
    use super::SyncPlan;
    use super::SyncScheduleConfig;
    use super::SyncScheduler;
    use super::build_sync_plan;
    use super::execute_sync_plan;
    use super::init_observability;
    use super::npub_from_hex;
    use super::path_to_str;
    use super::run_with_timeout;
    use super::select_relay_urls;
    use axum::body::{Body, to_bytes};
    use axum::http::Request;
    use axum::response::IntoResponse;
    use gittree_config::ConfigError;
    use gittree_core::NostrEvent;
    use gittree_core::RepoAnnouncement;
    use gittree_core::RepoState;
    use gittree_core::kinds::KIND_GIT_REPO_ANNOUNCEMENT;
    use gittree_core::{RelayCapability, RelayCompatibilityReport};
    use gittree_observability::{ObservabilityConfigError, ObservabilityError};
    use gittree_storage::{RelayCompatibilityRecord, RelayProbeMetadata};
    use std::collections::HashMap;
    use std::error::Error;
    use std::ffi::OsString;
    #[cfg(unix)]
    use std::os::unix::ffi::OsStringExt;
    use std::path::PathBuf;
    use std::process::Command;
    use std::sync::Mutex;
    use tower::ServiceExt;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn env_guard() -> std::sync::MutexGuard<'static, ()> {
        ENV_LOCK.lock().expect("env lock poisoned")
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

    fn without_env_var<F: FnOnce()>(key: &str, f: F) {
        let previous = std::env::var_os(key);
        // SAFETY: tests run single-threaded in this crate; we restore the previous value after.
        unsafe {
            std::env::remove_var(key);
        }
        f();
        match previous {
            Some(old) => unsafe {
                std::env::set_var(key, old);
            },
            None => {}
        }
    }

    #[test]
    fn config_loads_from_env() {
        let _guard = env_guard();
        with_env_var(
            ENV_STORAGE_READ_URL,
            "postgres://user:pass@localhost:5432/gittree",
            || {
                with_env_var("GITTREE_SYNC_BIND", "127.0.0.1:9092", || {
                    with_env_var("GITTREE_RELAY_URLS", "wss://relay.example", || {
                        with_env_var(super::ENV_SYNC_REPO_ROOT, "/tmp/gittree-sync", || {
                            let config = SyncConfig::from_env().expect("config");
                            assert_eq!(config.bind, "127.0.0.1:9092");
                            assert_eq!(
                                config.storage.read_connection,
                                "postgres://user:pass@localhost:5432/gittree"
                            );
                            assert_eq!(config.relay_urls, vec!["wss://relay.example".to_string()]);
                            assert_eq!(config.repo_root, PathBuf::from("/tmp/gittree-sync"));
                        });
                    });
                });
            },
        );
    }

    #[test]
    fn config_ignores_empty_pool_timeouts() {
        let _guard = env_guard();
        with_env_var(
            ENV_STORAGE_READ_URL,
            "postgres://user:pass@localhost:5432/gittree",
            || {
                with_env_var(super::ENV_SYNC_REPO_ROOT, "/tmp/gittree-sync", || {
                    with_env_var(super::ENV_STORAGE_MAX_CONNECTIONS, "", || {
                        with_env_var(super::ENV_STORAGE_MIN_CONNECTIONS, "", || {
                            with_env_var(super::ENV_STORAGE_IDLE_TIMEOUT_SECS, "", || {
                                with_env_var(super::ENV_STORAGE_MAX_LIFETIME_SECS, "", || {
                                    let config = SyncConfig::from_env().expect("config");
                                    assert_eq!(config.storage.max_connections, 10);
                                    assert_eq!(config.storage.min_connections, 2);
                                    assert_eq!(config.storage.idle_timeout_secs, None);
                                    assert_eq!(config.storage.max_lifetime_secs, None);
                                });
                            });
                        });
                    });
                });
            },
        );
    }

    #[test]
    fn config_requires_storage_read_url() {
        let _guard = env_guard();
        with_env_var("GITTREE_RELAY_URLS", "wss://relay.example", || {
            with_env_var(super::ENV_SYNC_REPO_ROOT, "/tmp/gittree-sync", || {
                without_env_var(ENV_STORAGE_READ_URL, || {
                    let err = SyncConfig::from_env().expect_err("missing storage read url");
                    assert!(matches!(
                        err,
                        SyncConfigError::Storage(StorageConfigError::MissingEnv(
                            super::ENV_STORAGE_READ_URL
                        ))
                    ));
                });
            });
        });
    }

    #[test]
    fn config_requires_repo_root() {
        let _guard = env_guard();
        with_env_var(
            ENV_STORAGE_READ_URL,
            "postgres://user:pass@localhost:5432/gittree",
            || {
                with_env_var("GITTREE_RELAY_URLS", "wss://relay.example", || {
                    without_env_var(super::ENV_SYNC_REPO_ROOT, || {
                        let err = SyncConfig::from_env().expect_err("missing repo root");
                        assert!(matches!(
                            err,
                            SyncConfigError::MissingEnv(super::ENV_SYNC_REPO_ROOT)
                        ));
                    });
                });
            },
        );
    }

    #[test]
    fn config_rejects_invalid_numeric_storage_values_and_repo_root() {
        let _guard = env_guard();
        with_env_var(
            ENV_STORAGE_READ_URL,
            "postgres://user:pass@localhost:5432/gittree",
            || {
                with_env_var("GITTREE_RELAY_URLS", "wss://relay.example", || {
                    with_env_var(super::ENV_SYNC_REPO_ROOT, "/tmp/gittree-sync", || {
                        with_env_var(super::ENV_STORAGE_MAX_CONNECTIONS, "oops", || {
                            let err = SyncConfig::from_env().expect_err("invalid max connections");
                            assert!(matches!(
                                err,
                                SyncConfigError::Storage(StorageConfigError::InvalidEnv {
                                    key: super::ENV_STORAGE_MAX_CONNECTIONS,
                                    ..
                                })
                            ));
                        });

                        with_env_var(super::ENV_STORAGE_IDLE_TIMEOUT_SECS, "bad", || {
                            let err = SyncConfig::from_env().expect_err("invalid idle timeout");
                            assert!(matches!(
                                err,
                                SyncConfigError::Storage(StorageConfigError::InvalidEnv {
                                    key: super::ENV_STORAGE_IDLE_TIMEOUT_SECS,
                                    ..
                                })
                            ));
                        });
                    });
                });
            },
        );

        with_env_var(
            ENV_STORAGE_READ_URL,
            "postgres://user:pass@localhost:5432/gittree",
            || {
                with_env_var("GITTREE_RELAY_URLS", "wss://relay.example", || {
                    with_env_var(super::ENV_SYNC_REPO_ROOT, "   ", || {
                        let err = SyncConfig::from_env().expect_err("blank repo root");
                        assert!(matches!(
                            err,
                            SyncConfigError::InvalidEnv {
                                key: super::ENV_SYNC_REPO_ROOT,
                                ..
                            }
                        ));
                    });
                });
            },
        );
    }

    #[test]
    fn config_rejects_invalid_pool_bounds() {
        let _guard = env_guard();
        with_env_var(
            ENV_STORAGE_READ_URL,
            "postgres://user:pass@localhost:5432/gittree",
            || {
                with_env_var("GITTREE_RELAY_URLS", "wss://relay.example", || {
                    with_env_var(super::ENV_SYNC_REPO_ROOT, "/tmp/gittree-sync", || {
                        with_env_var(super::ENV_STORAGE_MAX_CONNECTIONS, "1", || {
                            with_env_var(super::ENV_STORAGE_MIN_CONNECTIONS, "2", || {
                                let err = SyncConfig::from_env().expect_err("invalid pool bounds");
                                assert!(matches!(
                                    err,
                                    SyncConfigError::Storage(StorageConfigError::InvalidConfig(_))
                                ));
                            });
                        });
                    });
                });
            },
        );
    }

    #[test]
    fn config_rejects_invalid_relay_url() {
        let _guard = env_guard();
        with_env_var(
            ENV_STORAGE_READ_URL,
            "postgres://user:pass@localhost:5432/gittree",
            || {
                with_env_var(super::ENV_SYNC_REPO_ROOT, "/tmp/gittree-sync", || {
                    with_env_var("GITTREE_RELAY_URLS", "not-a-url", || {
                        let err = SyncConfig::from_env().expect_err("invalid relay url");
                        assert!(matches!(err, SyncConfigError::Config(_)));
                    });
                });
            },
        );
    }

    #[test]
    fn with_env_var_restores_previous_value() {
        let _guard = env_guard();
        const KEY: &str = "GITTREE_SYNC_TEST_ENV_RESTORE";
        unsafe {
            std::env::set_var(KEY, "before");
        }
        with_env_var(KEY, "during", || {
            assert_eq!(std::env::var(KEY).ok().as_deref(), Some("during"));
        });
        assert_eq!(std::env::var(KEY).ok().as_deref(), Some("before"));
        unsafe {
            std::env::remove_var(KEY);
        }
    }

    #[test]
    fn without_env_var_restores_previous_value() {
        let _guard = env_guard();
        const KEY: &str = "GITTREE_SYNC_TEST_ENV_RESTORE";
        with_env_var(KEY, "before", || {
            without_env_var(KEY, || {
                assert!(std::env::var(KEY).is_err());
            });
            assert_eq!(std::env::var(KEY).ok().as_deref(), Some("before"));
        });
    }

    #[test]
    fn config_and_sync_error_display_and_source_paths_are_stable() {
        let config = SyncConfigError::Config(ConfigError::InvalidConfig {
            field: "sync.repo_root",
            value: "bad".to_string(),
        });
        assert!(format!("{config}").contains("sync config error"));
        assert!(config.source().is_some());

        let storage = SyncConfigError::Storage(StorageConfigError::MissingEnv("READ"));
        assert!(format!("{storage}").contains("sync storage config error"));
        assert!(storage.source().is_some());

        let missing = SyncConfigError::MissingEnv("KEY");
        assert_eq!(format!("{missing}"), "missing env KEY");
        assert!(missing.source().is_none());

        let invalid = SyncConfigError::InvalidEnv {
            key: "KEY",
            value: "bad".to_string(),
        };
        assert_eq!(format!("{invalid}"), "invalid env KEY: bad");
        assert!(invalid.source().is_none());

        assert_eq!(
            format!("{}", StorageConfigError::MissingEnv("READ")),
            "missing env READ"
        );
        assert_eq!(
            format!(
                "{}",
                StorageConfigError::InvalidEnv {
                    key: "MAX",
                    value: "bad".to_string(),
                }
            ),
            "invalid env MAX: bad"
        );
        assert_eq!(
            format!(
                "{}",
                StorageConfigError::InvalidConfig("invalid".to_string())
            ),
            "invalid"
        );

        let sync_config = SyncError::Config(SyncConfigError::MissingEnv("KEY"));
        assert!(format!("{sync_config}").contains("sync error"));
        assert!(sync_config.source().is_some());

        let sync_observability_config =
            SyncError::ObservabilityConfig(ObservabilityConfigError::InvalidEnv {
                key: "KEY",
                value: "bad".to_string(),
            });
        assert!(format!("{sync_observability_config}").contains("sync observability config error"));
        assert!(sync_observability_config.source().is_some());

        let sync_observability =
            SyncError::Observability(ObservabilityError::MetricsInit("boom".to_string()));
        assert!(format!("{sync_observability}").contains("sync observability error"));
        assert!(sync_observability.source().is_some());

        let sync_serve = SyncError::Serve("bind".to_string());
        assert_eq!(format!("{sync_serve}"), "sync serve error: bind");
        assert!(sync_serve.source().is_none());
    }

    #[test]
    fn sync_http_and_npub_validation_cover_error_paths() {
        assert_eq!(
            SyncHttpError::BadRequest("bad".to_string())
                .into_response()
                .status(),
            axum::http::StatusCode::BAD_REQUEST
        );
        assert_eq!(
            SyncHttpError::Internal("oops".to_string())
                .into_response()
                .status(),
            axum::http::StatusCode::INTERNAL_SERVER_ERROR
        );

        let too_short = npub_from_hex("abcd").expect_err("short key");
        assert_eq!(
            too_short.into_response().status(),
            axum::http::StatusCode::BAD_REQUEST
        );

        let invalid_hex = npub_from_hex(&"zz".repeat(32)).expect_err("invalid hex");
        assert_eq!(
            invalid_hex.into_response().status(),
            axum::http::StatusCode::BAD_REQUEST
        );

        let encode_err = super::encode_npub_bytes_with(&[0_u8; 32], |_hrp, _bytes| Err(()))
            .expect_err("forced encode failure");
        assert_eq!(
            encode_err.into_response().status(),
            axum::http::StatusCode::INTERNAL_SERVER_ERROR
        );
    }

    #[test]
    fn select_relays_warns_when_configured_list_is_empty() {
        let selection = select_relay_urls(&[], &[]);
        assert!(selection.relays.is_empty());
        assert_eq!(
            selection.warnings,
            vec!["no relay urls configured".to_string()]
        );
    }

    #[test]
    fn run_with_timeout_and_command_executor_cover_failure_modes() {
        let mut success = Command::new("sh");
        success.arg("-c").arg("exit 0");
        run_with_timeout(success, std::time::Duration::from_millis(200)).expect("success");

        let mut failure = Command::new("sh");
        failure.arg("-c").arg("exit 1");
        let failed = run_with_timeout(failure, std::time::Duration::from_millis(200))
            .expect_err("command failure");
        assert!(matches!(failed, GitExecError::CommandFailed(_)));

        let mut timeout = Command::new("sh");
        timeout.arg("-c").arg("sleep 1");
        let timed_out =
            run_with_timeout(timeout, std::time::Duration::from_millis(20)).expect_err("timeout");
        assert!(matches!(timed_out, GitExecError::Timeout));

        let missing_command = Command::new("/definitely/missing/command");
        let spawn_err = run_with_timeout(missing_command, std::time::Duration::from_millis(20))
            .expect_err("spawn should fail");
        assert!(matches!(spawn_err, GitExecError::Io(_)));

        let executor = CommandGitExecutor;
        let repo_path = std::path::Path::new("/definitely/missing/repo");
        let fetch_err = executor
            .fetch(
                repo_path,
                "https://gittr.ee/repo.git",
                std::time::Duration::from_secs(1),
            )
            .expect_err("missing repo should fail");
        assert!(matches!(fetch_err, GitExecError::CommandFailed(_)));

        let update_err = executor
            .update_ref(repo_path, "refs/heads/main", &"11".repeat(20))
            .expect_err("missing repo should fail update");
        assert!(matches!(update_err, GitExecError::CommandFailed(_)));

        let delete_err = executor
            .delete_ref(repo_path, "refs/heads/main")
            .expect_err("missing repo should fail delete");
        assert!(matches!(delete_err, GitExecError::CommandFailed(_)));
    }

    #[test]
    fn git_exec_error_display_and_source_cover_variants() {
        let io = GitExecError::Io(std::io::Error::new(std::io::ErrorKind::Other, "boom"));
        assert!(io.to_string().contains("git io error"));
        assert!(io.source().is_some());

        let timeout = GitExecError::Timeout;
        assert_eq!(timeout.to_string(), "git command timed out");
        assert!(timeout.source().is_none());

        let invalid_path = GitExecError::InvalidPath("bad path".to_string());
        assert_eq!(invalid_path.to_string(), "bad path");
        assert!(invalid_path.source().is_none());

        let failed = GitExecError::CommandFailed("failed".to_string());
        assert_eq!(failed.to_string(), "failed");
        assert!(failed.source().is_none());
    }

    #[cfg(unix)]
    #[test]
    fn path_to_str_rejects_non_utf8_path() {
        let value = OsString::from_vec(vec![0xf0, 0x28, 0x8c, 0xbc]);
        let path = PathBuf::from(value);
        let err = path_to_str(&path).expect_err("invalid utf8 path");
        assert!(matches!(err, GitExecError::InvalidPath(_)));
    }

    #[cfg(unix)]
    #[test]
    fn command_executor_rejects_non_utf8_repo_paths() {
        let value = OsString::from_vec(vec![0xf0, 0x28, 0x8c, 0xbc]);
        let path = PathBuf::from(value);
        let executor = CommandGitExecutor;
        let fetch_err = executor
            .fetch(
                &path,
                "https://gittr.ee/repo.git",
                std::time::Duration::from_secs(1),
            )
            .expect_err("invalid path should fail fetch");
        assert!(matches!(fetch_err, GitExecError::InvalidPath(_)));

        let update_err = executor
            .update_ref(&path, "refs/heads/main", &"11".repeat(20))
            .expect_err("invalid path should fail update");
        assert!(matches!(update_err, GitExecError::InvalidPath(_)));

        let delete_err = executor
            .delete_ref(&path, "refs/heads/main")
            .expect_err("invalid path should fail delete");
        assert!(matches!(delete_err, GitExecError::InvalidPath(_)));
    }

    #[tokio::test]
    async fn serve_maps_bind_error_after_observability_init() {
        let occupied = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("occupied listener");
        let bind = occupied.local_addr().expect("occupied addr");
        let config = SyncConfig {
            bind: bind.to_string(),
            storage: super::StorageConfig {
                read_connection: "postgres://user:pass@localhost:5432/gittree".to_string(),
                write_connection: None,
                max_connections: 10,
                min_connections: 1,
                idle_timeout_secs: None,
                max_lifetime_secs: None,
                application_name: None,
            },
            relay_urls: vec!["wss://relay.example".to_string()],
            repo_root: PathBuf::from("/tmp/gittree-sync"),
        };
        let err = super::serve_with(config, || Ok(()), super::run_axum_server)
            .await
            .expect_err("bind error");
        assert!(matches!(err, SyncError::Serve(_)));
    }

    #[tokio::test]
    async fn serve_with_maps_server_errors() {
        let config = SyncConfig {
            bind: "127.0.0.1:0".to_string(),
            storage: super::StorageConfig {
                read_connection: "postgres://user:pass@localhost:5432/gittree".to_string(),
                write_connection: None,
                max_connections: 10,
                min_connections: 1,
                idle_timeout_secs: None,
                max_lifetime_secs: None,
                application_name: None,
            },
            relay_urls: vec!["wss://relay.example".to_string()],
            repo_root: PathBuf::from("/tmp/gittree-sync"),
        };
        let err = super::serve_with(
            config,
            || Ok(()),
            |_listener, _router| async { Err(std::io::Error::other("boom")) },
        )
        .await
        .expect_err("serve error");
        assert!(matches!(err, SyncError::Serve(message) if message.contains("boom")));
    }

    #[tokio::test]
    async fn serve_with_returns_ok_when_server_finishes_cleanly() {
        let config = SyncConfig {
            bind: "127.0.0.1:0".to_string(),
            storage: super::StorageConfig {
                read_connection: "postgres://user:pass@localhost:5432/gittree".to_string(),
                write_connection: None,
                max_connections: 10,
                min_connections: 1,
                idle_timeout_secs: None,
                max_lifetime_secs: None,
                application_name: None,
            },
            relay_urls: vec!["wss://relay.example".to_string()],
            repo_root: PathBuf::from("/tmp/gittree-sync"),
        };
        let result = super::serve_with(
            config,
            || Ok(()),
            |_listener, _router| async { Ok::<(), std::io::Error>(()) },
        )
        .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn serve_wrapper_maps_errors() {
        let config = SyncConfig {
            bind: "not-a-bind".to_string(),
            storage: super::StorageConfig {
                read_connection: "postgres://user:pass@localhost:5432/gittree".to_string(),
                write_connection: None,
                max_connections: 10,
                min_connections: 1,
                idle_timeout_secs: None,
                max_lifetime_secs: None,
                application_name: None,
            },
            relay_urls: vec!["wss://relay.example".to_string()],
            repo_root: PathBuf::from("/tmp/gittree-sync"),
        };
        let err = super::serve(config)
            .await
            .expect_err("expected serve error");
        assert!(matches!(
            err,
            SyncError::Serve(_) | SyncError::Observability(_) | SyncError::ObservabilityConfig(_)
        ));
    }

    #[tokio::test]
    async fn run_axum_server_can_start_and_be_aborted() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let app = super::build_router(super::SyncAppState {
            repo_root: PathBuf::from("/tmp/gittree-sync"),
        });
        let task = tokio::spawn(async move { super::run_axum_server(listener, app).await });
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        task.abort();
        let _ = task.await;
    }

    #[tokio::test]
    async fn health_endpoint_returns_ok() {
        let app = super::build_router(super::SyncAppState {
            repo_root: PathBuf::from("/tmp/gittree-sync"),
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
    }

    #[tokio::test]
    async fn post_receive_endpoint_returns_repo_path() {
        let repo_root = PathBuf::from("/tmp/gittree-sync");
        let app = super::build_router(super::SyncAppState {
            repo_root: repo_root.clone(),
        });
        let payload = PostReceivePayload {
            pubkey: "11".repeat(32),
            identifier: "repo".to_string(),
            updates: vec![RefUpdatePayload {
                old: "0".repeat(40),
                new: "1".repeat(40),
                reference: "refs/heads/main".to_string(),
            }],
        };
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/")
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
        let ack: SyncAckPayload = serde_json::from_slice(&body).expect("ack");
        let npub = super::npub_from_hex(&payload.pubkey).expect("npub");
        let expected = repo_root.join(npub).join("repo.git");
        assert_eq!(ack.repo_path, expected.display().to_string());
        assert_eq!(ack.updates, 1);
    }

    #[tokio::test]
    async fn post_receive_rejects_invalid_repo_address() {
        let app = super::build_router(super::SyncAppState {
            repo_root: PathBuf::from("/tmp/gittree-sync"),
        });
        let payload = PostReceivePayload {
            pubkey: "11".repeat(32),
            identifier: "".to_string(),
            updates: Vec::new(),
        };
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&payload).expect("body")))
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), axum::http::StatusCode::BAD_REQUEST);
    }

    #[test]
    fn plan_builder_selects_updates_and_deletions() {
        let mut state_map = HashMap::new();
        state_map.insert("HEAD".to_string(), "ref: refs/heads/main".to_string());
        state_map.insert("refs/heads/main".to_string(), "11".repeat(20));
        state_map.insert("refs/tags/v1".to_string(), "22".repeat(20));
        let state = RepoState {
            identifier: "repo".to_string(),
            state: state_map,
        };

        let mut local_refs = HashMap::new();
        local_refs.insert("refs/heads/main".to_string(), "11".repeat(20));
        local_refs.insert("refs/heads/old".to_string(), "33".repeat(20));

        let announcement = RepoAnnouncement {
            identifier: "repo".to_string(),
            name: None,
            description: None,
            root_commit: None,
            clone: vec!["https://gittr.ee/npub1example/repo.git".to_string()],
            web: Vec::new(),
            relays: vec!["wss://gittr.ee".to_string()],
            blossoms: Vec::new(),
            hashtags: Vec::new(),
            maintainers: Vec::new(),
        };
        let maintainer = "aa".repeat(32);
        let event = NostrEvent::new(
            KIND_GIT_REPO_ANNOUNCEMENT.0,
            maintainer.clone(),
            0,
            announcement.to_tags(),
        );
        let plan = build_sync_plan(&state, &local_refs, &[event], &[maintainer]);
        assert_eq!(plan.identifier, "repo");
        assert_eq!(plan.clone_urls.len(), 1);
        assert!(plan.deletions.contains(&"refs/heads/old".to_string()));
        assert!(plan.updates.contains(&RefUpdatePlan {
            reference: "refs/tags/v1".to_string(),
            target: "22".repeat(20),
        }));
        assert!(!plan.updates.contains(&RefUpdatePlan {
            reference: "refs/heads/main".to_string(),
            target: "11".repeat(20),
        }));
    }

    #[test]
    fn plan_builder_keeps_head_out_of_deletions() {
        let mut state_map = HashMap::new();
        state_map.insert("HEAD".to_string(), "ref: refs/heads/main".to_string());
        state_map.insert("refs/heads/main".to_string(), "11".repeat(20));
        let state = RepoState {
            identifier: "repo".to_string(),
            state: state_map,
        };

        let mut local_refs = HashMap::new();
        local_refs.insert("HEAD".to_string(), "ref: refs/heads/main".to_string());
        local_refs.insert("refs/heads/main".to_string(), "11".repeat(20));
        local_refs.insert("refs/heads/old".to_string(), "22".repeat(20));

        let plan = build_sync_plan(&state, &local_refs, &[], &[]);
        assert!(!plan.deletions.contains(&"HEAD".to_string()));
        assert!(plan.deletions.contains(&"refs/heads/old".to_string()));
    }

    struct MockGitExecutor {
        fetch_fail: Vec<String>,
        update_fail: Vec<String>,
        delete_fail: Vec<String>,
    }

    impl GitExecutor for MockGitExecutor {
        fn fetch(
            &self,
            _repo_path: &std::path::Path,
            remote: &str,
            _timeout: std::time::Duration,
        ) -> Result<(), GitExecError> {
            if self.fetch_fail.iter().any(|value| value == remote) {
                Err(GitExecError::CommandFailed("fetch failed".to_string()))
            } else {
                Ok(())
            }
        }

        fn update_ref(
            &self,
            _repo_path: &std::path::Path,
            reference: &str,
            _target: &str,
        ) -> Result<(), GitExecError> {
            if self.update_fail.iter().any(|value| value == reference) {
                Err(GitExecError::CommandFailed("update failed".to_string()))
            } else {
                Ok(())
            }
        }

        fn delete_ref(
            &self,
            _repo_path: &std::path::Path,
            reference: &str,
        ) -> Result<(), GitExecError> {
            if self.delete_fail.iter().any(|value| value == reference) {
                Err(GitExecError::CommandFailed("delete failed".to_string()))
            } else {
                Ok(())
            }
        }
    }

    #[test]
    fn executor_records_remote_errors() {
        let plan = SyncPlan {
            identifier: "repo".to_string(),
            clone_urls: vec!["https://good".to_string(), "https://bad".to_string()],
            updates: vec![RefUpdatePlan {
                reference: "refs/heads/main".to_string(),
                target: "11".repeat(20),
            }],
            deletions: vec!["refs/heads/old".to_string()],
        };
        let executor = MockGitExecutor {
            fetch_fail: vec!["https://bad".to_string()],
            update_fail: vec!["refs/heads/main".to_string()],
            delete_fail: Vec::new(),
        };
        let report = execute_sync_plan(
            &executor,
            std::path::Path::new("/tmp"),
            &plan,
            std::time::Duration::from_secs(1),
        );
        assert!(
            report
                .remote_results
                .iter()
                .any(|result| result.url == "https://bad" && !result.success)
        );
        assert!(
            report
                .remote_results
                .iter()
                .any(|result| result.url == "https://good" && result.success)
        );
        assert_eq!(
            report.update_results,
            vec![RefChangeResult {
                reference: "refs/heads/main".to_string(),
                success: false,
                error: Some("update failed".to_string()),
            }]
        );
        assert_eq!(
            report.delete_results,
            vec![RefChangeResult {
                reference: "refs/heads/old".to_string(),
                success: true,
                error: None,
            }]
        );
    }

    #[test]
    fn executor_records_update_success_and_delete_failure() {
        let plan = SyncPlan {
            identifier: "repo".to_string(),
            clone_urls: vec!["https://good".to_string()],
            updates: vec![RefUpdatePlan {
                reference: "refs/heads/main".to_string(),
                target: "11".repeat(20),
            }],
            deletions: vec!["refs/heads/old".to_string()],
        };
        let executor = MockGitExecutor {
            fetch_fail: Vec::new(),
            update_fail: Vec::new(),
            delete_fail: vec!["refs/heads/old".to_string()],
        };
        let report = execute_sync_plan(
            &executor,
            std::path::Path::new("/tmp"),
            &plan,
            std::time::Duration::from_secs(1),
        );
        assert_eq!(
            report.update_results,
            vec![RefChangeResult {
                reference: "refs/heads/main".to_string(),
                success: true,
                error: None,
            }]
        );
        assert_eq!(
            report.delete_results,
            vec![RefChangeResult {
                reference: "refs/heads/old".to_string(),
                success: false,
                error: Some("delete failed".to_string()),
            }]
        );
    }

    #[test]
    fn scheduler_backoff_increases_and_caps() {
        let mut scheduler = SyncScheduler::new(SyncScheduleConfig {
            interval: std::time::Duration::from_secs(10),
            max_backoff: std::time::Duration::from_secs(25),
            max_concurrent: 2,
        });
        let first = scheduler.next_delay(false);
        let second = scheduler.next_delay(false);
        let third = scheduler.next_delay(false);
        assert!(first >= std::time::Duration::from_secs(10));
        assert!(second > first);
        assert_eq!(third, std::time::Duration::from_secs(25));
        let reset = scheduler.next_delay(true);
        assert_eq!(reset, std::time::Duration::from_secs(10));
    }

    #[test]
    fn scheduler_limits_concurrency_per_repo() {
        let mut scheduler = SyncScheduler::new(SyncScheduleConfig {
            interval: std::time::Duration::from_secs(1),
            max_backoff: std::time::Duration::from_secs(5),
            max_concurrent: 1,
        });
        assert!(scheduler.try_start_repo("repo-a"));
        assert!(!scheduler.try_start_repo("repo-a"));
        assert!(!scheduler.try_start_repo("repo-b"));
        scheduler.finish_repo("repo-a");
        assert!(scheduler.try_start_repo("repo-b"));
    }

    #[test]
    fn scheduler_default_values_are_stable() {
        let defaults = SyncScheduleConfig::default();
        assert_eq!(defaults.interval, std::time::Duration::from_secs(60));
        assert_eq!(defaults.max_backoff, std::time::Duration::from_secs(300));
        assert_eq!(defaults.max_concurrent, 4);
    }

    #[test]
    fn select_relays_prefers_compatible() {
        let report = RelayCompatibilityReport {
            relay_url: "wss://relay.example".to_string(),
            supported: vec![RelayCapability::Nip01, RelayCapability::Nip34],
            missing_required: Vec::new(),
            missing_optional: Vec::new(),
        };
        let record = RelayCompatibilityRecord::new(&report, 0, &RelayProbeMetadata::default())
            .expect("record");
        let selection = select_relay_urls(
            &vec![
                "wss://relay.example".to_string(),
                "wss://relay.other".to_string(),
            ],
            &[record],
        );
        assert_eq!(selection.relays, vec!["wss://relay.example".to_string()]);
        assert!(selection.warnings.is_empty());
    }

    #[test]
    fn select_relays_falls_back_when_none_compatible() {
        let report = RelayCompatibilityReport {
            relay_url: "wss://relay.example".to_string(),
            supported: vec![RelayCapability::Nip01],
            missing_required: vec![RelayCapability::Nip34],
            missing_optional: Vec::new(),
        };
        let record = RelayCompatibilityRecord::new(&report, 0, &RelayProbeMetadata::default())
            .expect("record");
        let selection = select_relay_urls(&vec!["wss://relay.example".to_string()], &[record]);
        assert_eq!(selection.relays, vec!["wss://relay.example".to_string()]);
        assert!(!selection.warnings.is_empty());
    }

    #[test]
    fn observability_init_returns_registry() {
        let _guard = env_guard();
        without_env_var("GITTREE_LOG_JSON", || {
            let _ = init_observability();
            let second = init_observability();
            assert!(matches!(
                second,
                Err(SyncError::Observability(
                    ObservabilityError::SubscriberInit(_)
                ))
            ));
        });
    }

    #[test]
    fn observability_init_reports_config_error() {
        let _guard = env_guard();
        with_env_var("GITTREE_LOG_JSON", "not-a-bool", || {
            let err = super::load_observability_config().expect_err("invalid observability config");
            assert!(matches!(
                err,
                SyncError::ObservabilityConfig(ObservabilityConfigError::InvalidEnv { .. })
            ));
        });
    }
}
