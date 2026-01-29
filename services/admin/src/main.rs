use gittree_admin::{AdminCli, AdminCliError, AdminCommand};
use gittree_core::{ForgejoRepo, RepoMapping};
use gittree_observability::ObservabilityHandle;
use gittree_storage::{
    PostgresRepositories, RepoMappingRecord, RepoMappingRepository, StorageConfig, StorageError,
};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde::de::DeserializeOwned;

const ENV_STORAGE_READ_URL: &str = "GITTREE_STORAGE_READ_URL";
const ENV_STORAGE_WRITE_URL: &str = "GITTREE_STORAGE_WRITE_URL";
const ENV_STORAGE_MAX_CONNECTIONS: &str = "GITTREE_STORAGE_MAX_CONNECTIONS";
const ENV_STORAGE_MIN_CONNECTIONS: &str = "GITTREE_STORAGE_MIN_CONNECTIONS";
const ENV_STORAGE_IDLE_TIMEOUT_SECS: &str = "GITTREE_STORAGE_IDLE_TIMEOUT_SECS";
const ENV_STORAGE_MAX_LIFETIME_SECS: &str = "GITTREE_STORAGE_MAX_LIFETIME_SECS";
const ENV_STORAGE_APP_NAME: &str = "GITTREE_STORAGE_APP_NAME";
const ENV_CONTROL_URL: &str = "GITTREE_CONTROL_URL";
const ENV_CONTROL_TOKEN: &str = "GITTREE_CONTROL_TOKEN";
const DEFAULT_CONTROL_URL: &str = "http://127.0.0.1:8088";

fn init_observability() -> Result<ObservabilityHandle, String> {
    let config = gittree_observability::ObservabilityConfig::from_env("gittree-admin")
        .map_err(|err| err.to_string())?;
    gittree_observability::init(&config).map_err(|err| err.to_string())
}

#[derive(Debug, Clone)]
struct ControlClientConfig {
    base_url: String,
    token: String,
}

impl ControlClientConfig {
    fn from_env() -> Result<Self, AdminError> {
        let base_url = match std::env::var(ENV_CONTROL_URL) {
            Ok(value) if !value.trim().is_empty() => value,
            _ => DEFAULT_CONTROL_URL.to_string(),
        };
        let token = std::env::var(ENV_CONTROL_TOKEN).map_err(|_| {
            AdminError::ControlConfig(ControlConfigError::MissingEnv(ENV_CONTROL_TOKEN))
        })?;
        if token.trim().is_empty() {
            return Err(AdminError::ControlConfig(ControlConfigError::MissingEnv(
                ENV_CONTROL_TOKEN,
            )));
        }
        Ok(Self { base_url, token })
    }
}

#[derive(Clone)]
struct ControlClient {
    base_url: String,
    token: String,
    client: Client,
}

impl ControlClient {
    fn new(config: ControlClientConfig) -> Result<Self, AdminError> {
        let client = Client::builder()
            .user_agent("gittree-admin")
            .build()
            .map_err(AdminError::ControlRequest)?;
        Ok(Self {
            base_url: config.base_url,
            token: config.token,
            client,
        })
    }

    fn endpoint(&self, path: &str) -> String {
        format!("{}{}", self.base_url.trim_end_matches('/'), path)
    }

    async fn post<T, R>(&self, path: &str, payload: &T) -> Result<R, AdminError>
    where
        T: Serialize + ?Sized,
        R: DeserializeOwned,
    {
        let response = self
            .client
            .post(self.endpoint(path))
            .bearer_auth(&self.token)
            .json(payload)
            .send()
            .await
            .map_err(AdminError::ControlRequest)?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(AdminError::ControlResponse(format!(
                "control request failed: {status} {body}"
            )));
        }
        response
            .json::<R>()
            .await
            .map_err(AdminError::ControlRequest)
    }

    async fn create_user(&self, payload: ControlCreateUser) -> Result<ControlUser, AdminError> {
        self.post("/control/users", &payload).await
    }

    async fn create_org(&self, payload: ControlCreateOrg) -> Result<ControlOrg, AdminError> {
        self.post("/control/orgs", &payload).await
    }

    async fn create_repo(&self, payload: ControlCreateRepo) -> Result<ControlRepo, AdminError> {
        self.post("/control/repos", &payload).await
    }

    async fn create_pull(&self, payload: ControlCreatePull) -> Result<ControlPull, AdminError> {
        self.post("/control/pulls", &payload).await
    }
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

#[derive(Debug, Serialize)]
struct ControlCreateUser {
    username: String,
    email: String,
    password: String,
    full_name: Option<String>,
    must_change_password: Option<bool>,
    send_notify: Option<bool>,
}

#[derive(Debug, Serialize)]
struct ControlCreateOrg {
    owner: String,
    name: String,
    full_name: Option<String>,
    description: Option<String>,
    visibility: Option<String>,
}

#[derive(Debug, Serialize)]
struct ControlCreateRepo {
    owner: String,
    name: String,
    description: Option<String>,
    private: Option<bool>,
    auto_init: Option<bool>,
}

#[derive(Debug, Serialize)]
struct ControlCreatePull {
    owner: String,
    repo: String,
    head: String,
    base: String,
    title: String,
    body: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ControlUser {
    username: String,
    email: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ControlOrg {
    name: String,
    full_name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ControlRepo {
    owner: String,
    name: String,
    html_url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ControlPull {
    number: u64,
    url: String,
    html_url: Option<String>,
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
        AdminCommand::CreateUser {
            username,
            email,
            password,
            full_name,
            must_change_password,
            send_notify,
        } => {
            let control = control_client_from_env()?;
            let user = control
                .create_user(ControlCreateUser {
                    username,
                    email,
                    password,
                    full_name,
                    must_change_password,
                    send_notify,
                })
                .await?;
            println!(
                "created user {}{}",
                user.username,
                user.email
                    .as_ref()
                    .map(|email| format!(" ({email})"))
                    .unwrap_or_default()
            );
        }
        AdminCommand::CreateOrg {
            owner,
            name,
            full_name,
            description,
            visibility,
        } => {
            let control = control_client_from_env()?;
            let org = control
                .create_org(ControlCreateOrg {
                    owner,
                    name,
                    full_name,
                    description,
                    visibility,
                })
                .await?;
            println!(
                "created org {}{}",
                org.name,
                org.full_name
                    .as_ref()
                    .map(|full| format!(" ({full})"))
                    .unwrap_or_default()
            );
        }
        AdminCommand::CreateRepo {
            owner,
            name,
            description,
            private,
            auto_init,
        } => {
            let control = control_client_from_env()?;
            let repo = control
                .create_repo(ControlCreateRepo {
                    owner,
                    name,
                    description,
                    private,
                    auto_init,
                })
                .await?;
            println!(
                "created repo {} ({}){}",
                repo.name,
                repo.owner,
                repo.html_url
                    .as_ref()
                    .map(|url| format!(" {url}"))
                    .unwrap_or_default()
            );
        }
        AdminCommand::CreatePull {
            owner,
            repo,
            head,
            base,
            title,
            body,
        } => {
            let control = control_client_from_env()?;
            let pull = control
                .create_pull(ControlCreatePull {
                    owner,
                    repo,
                    head,
                    base,
                    title,
                    body,
                })
                .await?;
            println!(
                "created pull #{} ({}){}",
                pull.number,
                pull.url,
                pull.html_url
                    .as_ref()
                    .map(|url| format!(" {url}"))
                    .unwrap_or_default()
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
    ControlConfig(ControlConfigError),
    ControlRequest(reqwest::Error),
    ControlResponse(String),
}

impl std::fmt::Display for AdminError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AdminError::Cli(err) => write!(f, "admin cli error: {err}"),
            AdminError::StorageConfig(err) => write!(f, "admin storage config error: {err}"),
            AdminError::Storage(err) => write!(f, "admin storage error: {err}"),
            AdminError::Core(err) => write!(f, "admin mapping error: {err}"),
            AdminError::ControlConfig(err) => write!(f, "admin control config error: {err}"),
            AdminError::ControlRequest(err) => write!(f, "admin control request error: {err}"),
            AdminError::ControlResponse(err) => write!(f, "admin control response error: {err}"),
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
            AdminError::ControlConfig(err) => Some(err),
            AdminError::ControlRequest(err) => Some(err),
            AdminError::ControlResponse(_) => None,
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

#[derive(Debug)]
pub enum ControlConfigError {
    MissingEnv(&'static str),
}

impl std::fmt::Display for ControlConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ControlConfigError::MissingEnv(key) => write!(f, "missing env {key}"),
        }
    }
}

impl std::error::Error for ControlConfigError {}

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

fn control_client_from_env() -> Result<ControlClient, AdminError> {
    let config = ControlClientConfig::from_env()?;
    ControlClient::new(config)
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
    use super::ControlClientConfig;
    use super::DEFAULT_CONTROL_URL;
    use super::ENV_CONTROL_TOKEN;
    use super::ENV_CONTROL_URL;
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

    #[test]
    fn control_config_uses_default_url() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var(ENV_CONTROL_TOKEN, "token", || {
            with_env_var(ENV_CONTROL_URL, "", || {
                let config = ControlClientConfig::from_env().expect("config");
                assert_eq!(config.base_url, DEFAULT_CONTROL_URL);
            });
        });
    }

    #[test]
    fn control_config_uses_env_url() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var(ENV_CONTROL_TOKEN, "token", || {
            with_env_var(ENV_CONTROL_URL, "http://localhost:9090", || {
                let config = ControlClientConfig::from_env().expect("config");
                assert_eq!(config.base_url, "http://localhost:9090");
            });
        });
    }
}
