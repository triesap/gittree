use gittree_config::{ConfigError, ServicesConfig};
use gittree_core::{Nip34Event, RepoAnnouncement, extract_npub, parse_repo_path};
use gittree_observability::{ObservabilityConfigError, ObservabilityError, ObservabilityHandle};
use gittree_storage::StorageConfig;
use std::path::{Path, PathBuf};
use std::process::Command;

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
    ObservabilityConfig(ObservabilityConfigError),
    Observability(ObservabilityError),
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
        }
    }
}

impl std::error::Error for CoordinatorError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            CoordinatorError::Config(err) => Some(err),
            CoordinatorError::ObservabilityConfig(err) => Some(err),
            CoordinatorError::Observability(err) => Some(err),
        }
    }
}

pub fn init_observability() -> Result<ObservabilityHandle, CoordinatorError> {
    let config = gittree_observability::ObservabilityConfig::from_env("gittree-coordinator")
        .map_err(CoordinatorError::ObservabilityConfig)?;
    let handle = gittree_observability::init(&config).map_err(CoordinatorError::Observability)?;
    Ok(handle)
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
    pub pubkey: String,
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
}

impl std::fmt::Display for CoordinatorEventError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CoordinatorEventError::Parse(message) => write!(f, "{message}"),
            CoordinatorEventError::MissingNpub => write!(f, "missing npub in clone urls"),
            CoordinatorEventError::Plan(err) => write!(f, "{err}"),
            CoordinatorEventError::Init(err) => write!(f, "{err}"),
            CoordinatorEventError::Hooks(err) => write!(f, "{err}"),
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

#[cfg(test)]
mod tests {
    use super::CoordinatorAction;
    use super::CoordinatorConfig;
    use super::ENV_STORAGE_READ_URL;
    use super::HookInstallConfig;
    use super::ObservabilityHandle;
    use super::RelayEvent;
    use super::RepoAnnouncement;
    use super::build_provision_plan;
    use super::handle_announcement_event;
    use super::init_observability;
    use super::init_repo;
    use super::install_hooks;
    use gittree_core::kinds::KIND_GIT_REPO_ANNOUNCEMENT;
    use std::fs;
    use std::sync::Mutex;
    use std::sync::OnceLock;

    static ENV_LOCK: Mutex<()> = Mutex::new(());
    static OBSERVABILITY: OnceLock<ObservabilityHandle> = OnceLock::new();

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
            pubkey: "11".repeat(32),
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
