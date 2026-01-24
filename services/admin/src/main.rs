use gittree_admin::{AdminCli, AdminCliError, AdminCommand};
use gittree_core::{ForgejoRepo, RepoMapping};
use gittree_observability::ObservabilityHandle;
use gittree_storage::{
    PostgresRepositories, RepoMappingRecord, RepoMappingRepository, StorageConfig, StorageError,
};

const ENV_STORAGE_READ_URL: &str = "GITTREE_STORAGE_READ_URL";
const ENV_STORAGE_WRITE_URL: &str = "GITTREE_STORAGE_WRITE_URL";
const ENV_STORAGE_MAX_CONNECTIONS: &str = "GITTREE_STORAGE_MAX_CONNECTIONS";
const ENV_STORAGE_MIN_CONNECTIONS: &str = "GITTREE_STORAGE_MIN_CONNECTIONS";
const ENV_STORAGE_IDLE_TIMEOUT_SECS: &str = "GITTREE_STORAGE_IDLE_TIMEOUT_SECS";
const ENV_STORAGE_MAX_LIFETIME_SECS: &str = "GITTREE_STORAGE_MAX_LIFETIME_SECS";
const ENV_STORAGE_APP_NAME: &str = "GITTREE_STORAGE_APP_NAME";

fn init_observability() -> Result<ObservabilityHandle, String> {
    let config = gittree_observability::ObservabilityConfig::from_env("gittree-admin")
        .map_err(|err| err.to_string())?;
    gittree_observability::init(&config).map_err(|err| err.to_string())
}

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();
    let _observability = match init_observability() {
        Ok(handle) => handle,
        Err(err) => {
            eprintln!("admin observability failed: {err}");
            std::process::exit(1);
        }
    };
    if let Err(err) = run().await {
        eprintln!("gittree-admin failed: {err}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), AdminError> {
    let cli = AdminCli::parse(std::env::args_os()).map_err(AdminError::Cli)?;
    if cli.help {
        println!("{}", AdminCli::help_text());
        return Ok(());
    }
    let command = cli
        .command
        .ok_or_else(|| AdminError::Cli(AdminCliError::MissingCommand))?;

    match command {
        AdminCommand::Map {
            forgejo,
            pubkey,
            identifier,
        } => {
            let forgejo = ForgejoRepo::parse(&forgejo).map_err(AdminError::Core)?;
            tracing::info!(
                owner = %forgejo.owner,
                name = %forgejo.name,
                pubkey = %pubkey,
                identifier = %identifier,
                "upserting repo mapping"
            );
            let mapping = RepoMapping::new(forgejo.owner, forgejo.name, pubkey, identifier)
                .map_err(AdminError::Core)?;
            let record = RepoMappingRecord::new(&mapping).map_err(AdminError::Storage)?;
            let storage = storage_from_env()?;
            let options = storage
                .write_connect_options()
                .map_err(AdminError::Storage)?;
            let pool = storage
                .pool_options()
                .map_err(AdminError::Storage)?
                .connect_with(options)
                .await
                .map_err(StorageError::from)
                .map_err(AdminError::Storage)?;
            let repo = PostgresRepositories::new(pool);
            repo.upsert_mapping(record)
                .await
                .map_err(AdminError::Storage)?;
            tracing::info!(
                owner = %mapping.forgejo.owner,
                name = %mapping.forgejo.name,
                "repo mapping stored"
            );
        }
    }

    Ok(())
}

#[derive(Debug)]
pub enum AdminError {
    Cli(AdminCliError),
    StorageConfig(StorageConfigError),
    Storage(StorageError),
    Core(gittree_core::CoreError),
}

impl std::fmt::Display for AdminError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AdminError::Cli(err) => write!(f, "admin cli error: {err}"),
            AdminError::StorageConfig(err) => write!(f, "admin storage config error: {err}"),
            AdminError::Storage(err) => write!(f, "admin storage error: {err}"),
            AdminError::Core(err) => write!(f, "admin mapping error: {err}"),
        }
    }
}

impl std::error::Error for AdminError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            AdminError::Cli(err) => Some(err),
            AdminError::StorageConfig(err) => Some(err),
            AdminError::Storage(err) => Some(err),
            AdminError::Core(err) => Some(err),
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

fn storage_from_env() -> Result<StorageConfig, AdminError> {
    let read_connection = std::env::var(ENV_STORAGE_READ_URL).map_err(|_| {
        AdminError::StorageConfig(StorageConfigError::MissingEnv(ENV_STORAGE_READ_URL))
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
        AdminError::StorageConfig(StorageConfigError::InvalidConfig(err.to_string()))
    })?;

    Ok(config)
}

fn env_u32(key: &'static str) -> Result<Option<u32>, AdminError> {
    match std::env::var(key) {
        Ok(value) => {
            if value.trim().is_empty() {
                return Ok(None);
            }
            value.parse::<u32>().map(Some).map_err(|_| {
                AdminError::StorageConfig(StorageConfigError::InvalidEnv { key, value })
            })
        }
        Err(_) => Ok(None),
    }
}

fn env_u64(key: &'static str) -> Result<Option<u64>, AdminError> {
    match std::env::var(key) {
        Ok(value) => {
            if value.trim().is_empty() {
                return Ok(None);
            }
            value.parse::<u64>().map(Some).map_err(|_| {
                AdminError::StorageConfig(StorageConfigError::InvalidEnv { key, value })
            })
        }
        Err(_) => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::ENV_STORAGE_IDLE_TIMEOUT_SECS;
    use super::ENV_STORAGE_MAX_LIFETIME_SECS;
    use super::ENV_STORAGE_READ_URL;
    use super::storage_from_env;
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
    fn storage_config_ignores_empty_timeouts() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var(
            ENV_STORAGE_READ_URL,
            "postgres://user:pass@localhost:5432/gittree",
            || {
                with_env_var(ENV_STORAGE_IDLE_TIMEOUT_SECS, "", || {
                    with_env_var(ENV_STORAGE_MAX_LIFETIME_SECS, "", || {
                        let config = storage_from_env().expect("config");
                        assert_eq!(config.idle_timeout_secs, None);
                        assert_eq!(config.max_lifetime_secs, None);
                    });
                });
            },
        );
    }
}
