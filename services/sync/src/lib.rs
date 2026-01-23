use gittree_config::{ConfigError, ServicesConfig};
use gittree_core::{NostrEvent, RepoState, collect_clone_urls};
use gittree_storage::StorageConfig;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::{Duration, Instant};

const ENV_STORAGE_READ_URL: &str = "GITTREE_STORAGE_READ_URL";
const ENV_STORAGE_WRITE_URL: &str = "GITTREE_STORAGE_WRITE_URL";
const ENV_STORAGE_MAX_CONNECTIONS: &str = "GITTREE_STORAGE_MAX_CONNECTIONS";
const ENV_STORAGE_MIN_CONNECTIONS: &str = "GITTREE_STORAGE_MIN_CONNECTIONS";
const ENV_STORAGE_IDLE_TIMEOUT_SECS: &str = "GITTREE_STORAGE_IDLE_TIMEOUT_SECS";
const ENV_STORAGE_MAX_LIFETIME_SECS: &str = "GITTREE_STORAGE_MAX_LIFETIME_SECS";
const ENV_STORAGE_APP_NAME: &str = "GITTREE_STORAGE_APP_NAME";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncConfig {
    pub bind: String,
    pub storage: StorageConfig,
}

impl SyncConfig {
    pub fn from_env() -> Result<Self, SyncConfigError> {
        let services = ServicesConfig::from_env_validated().map_err(SyncConfigError::Config)?;
        let storage = storage_from_env()?;
        Ok(Self {
            bind: services.sync.bind,
            storage,
        })
    }
}

#[derive(Debug)]
pub enum SyncConfigError {
    Config(ConfigError),
    Storage(StorageConfigError),
}

impl std::fmt::Display for SyncConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SyncConfigError::Config(err) => write!(f, "sync config error: {err}"),
            SyncConfigError::Storage(err) => write!(f, "sync storage config error: {err}"),
        }
    }
}

impl std::error::Error for SyncConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            SyncConfigError::Config(err) => Some(err),
            SyncConfigError::Storage(err) => Some(err),
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
        Ok(value) => value
            .parse::<u32>()
            .map(Some)
            .map_err(|_| SyncConfigError::Storage(StorageConfigError::InvalidEnv { key, value })),
        Err(_) => Ok(None),
    }
}

fn env_u64(key: &'static str) -> Result<Option<u64>, SyncConfigError> {
    match std::env::var(key) {
        Ok(value) => value
            .parse::<u64>()
            .map(Some)
            .map_err(|_| SyncConfigError::Storage(StorageConfigError::InvalidEnv { key, value })),
        Err(_) => Ok(None),
    }
}

#[derive(Debug)]
pub enum SyncError {
    Config(SyncConfigError),
}

impl std::fmt::Display for SyncError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SyncError::Config(err) => write!(f, "sync error: {err}"),
        }
    }
}

impl std::error::Error for SyncError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            SyncError::Config(err) => Some(err),
        }
    }
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
    use super::ENV_STORAGE_READ_URL;
    use super::GitExecError;
    use super::GitExecutor;
    use super::RefChangeResult;
    use super::RefUpdatePlan;
    use super::SyncConfig;
    use super::SyncPlan;
    use super::SyncScheduleConfig;
    use super::SyncScheduler;
    use super::build_sync_plan;
    use super::execute_sync_plan;
    use gittree_core::NostrEvent;
    use gittree_core::RepoAnnouncement;
    use gittree_core::RepoState;
    use gittree_core::kinds::KIND_GIT_REPO_ANNOUNCEMENT;
    use std::collections::HashMap;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

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

    #[test]
    fn config_loads_from_env() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var(
            ENV_STORAGE_READ_URL,
            "postgres://user:pass@localhost:5432/gittree",
            || {
                with_env_var("GITTREE_SYNC_BIND", "127.0.0.1:9092", || {
                    let config = SyncConfig::from_env().expect("config");
                    assert_eq!(config.bind, "127.0.0.1:9092");
                    assert_eq!(
                        config.storage.read_connection,
                        "postgres://user:pass@localhost:5432/gittree"
                    );
                });
            },
        );
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
}
