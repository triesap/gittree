use gittree_config::{ConfigError, ServicesConfig};
use gittree_core::{RepoAnnouncement, parse_repo_path};
use gittree_storage::StorageConfig;
use std::path::{Path, PathBuf};

const ENV_STORAGE_READ_URL: &str = "GITTREE_STORAGE_READ_URL";
const ENV_STORAGE_WRITE_URL: &str = "GITTREE_STORAGE_WRITE_URL";
const ENV_STORAGE_MAX_CONNECTIONS: &str = "GITTREE_STORAGE_MAX_CONNECTIONS";
const ENV_STORAGE_MIN_CONNECTIONS: &str = "GITTREE_STORAGE_MIN_CONNECTIONS";
const ENV_STORAGE_IDLE_TIMEOUT_SECS: &str = "GITTREE_STORAGE_IDLE_TIMEOUT_SECS";
const ENV_STORAGE_MAX_LIFETIME_SECS: &str = "GITTREE_STORAGE_MAX_LIFETIME_SECS";
const ENV_STORAGE_APP_NAME: &str = "GITTREE_STORAGE_APP_NAME";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoordinatorConfig {
    pub bind: String,
    pub storage: StorageConfig,
}

impl CoordinatorConfig {
    pub fn from_env() -> Result<Self, CoordinatorConfigError> {
        let services =
            ServicesConfig::from_env_validated().map_err(CoordinatorConfigError::Config)?;
        let storage = storage_from_env()?;
        Ok(Self {
            bind: services.coordinator.bind,
            storage,
        })
    }
}

#[derive(Debug)]
pub enum CoordinatorConfigError {
    Config(ConfigError),
    Storage(StorageConfigError),
}

impl std::fmt::Display for CoordinatorConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CoordinatorConfigError::Config(err) => write!(f, "coordinator config error: {err}"),
            CoordinatorConfigError::Storage(err) => {
                write!(f, "coordinator storage config error: {err}")
            }
        }
    }
}

impl std::error::Error for CoordinatorConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            CoordinatorConfigError::Config(err) => Some(err),
            CoordinatorConfigError::Storage(err) => Some(err),
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
        Ok(value) => value.parse::<u32>().map(Some).map_err(|_| {
            CoordinatorConfigError::Storage(StorageConfigError::InvalidEnv { key, value })
        }),
        Err(_) => Ok(None),
    }
}

fn env_u64(key: &'static str) -> Result<Option<u64>, CoordinatorConfigError> {
    match std::env::var(key) {
        Ok(value) => value.parse::<u64>().map(Some).map_err(|_| {
            CoordinatorConfigError::Storage(StorageConfigError::InvalidEnv { key, value })
        }),
        Err(_) => Ok(None),
    }
}

#[derive(Debug)]
pub enum CoordinatorError {
    Config(CoordinatorConfigError),
}

impl std::fmt::Display for CoordinatorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CoordinatorError::Config(err) => write!(f, "coordinator error: {err}"),
        }
    }
}

impl std::error::Error for CoordinatorError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            CoordinatorError::Config(err) => Some(err),
        }
    }
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

#[cfg(test)]
mod tests {
    use super::CoordinatorConfig;
    use super::ENV_STORAGE_READ_URL;
    use super::RepoAnnouncement;
    use super::build_provision_plan;
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
                with_env_var("GITTREE_COORDINATOR_BIND", "127.0.0.1:9091", || {
                    let config = CoordinatorConfig::from_env().expect("config");
                    assert_eq!(config.bind, "127.0.0.1:9091");
                    assert_eq!(
                        config.storage.read_connection,
                        "postgres://user:pass@localhost:5432/gittree"
                    );
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
}
