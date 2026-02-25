use gittree_admin::{AdminCli, AdminCliError, AdminCommand};
use gittree_core::{ForgejoRepo, RepoMapping};
use gittree_observability::ObservabilityHandle;
use gittree_storage::{
    PostgresRepositories, RepoMappingRecord, RepoMappingRepository, StorageConfig, StorageError,
};
use reqwest::Client;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

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
    match gittree_observability::init(&config) {
        Ok(handle) => Ok(handle),
        Err(err) => Err(err.to_string()),
    }
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
    let command = match cli.command {
        Some(command) => command,
        None => return Err(AdminError::Cli(AdminCliError::MissingCommand)),
    };

    match command {
        AdminCommand::Map {
            forgejo,
            pubkey,
            identifier,
        } => {
            let forgejo = ForgejoRepo::parse(&forgejo).map_err(AdminError::Core)?;
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
    use super::AdminError;
    use super::ControlClient;
    use super::ControlClientConfig;
    use super::ControlConfigError;
    use super::ControlCreateOrg;
    use super::ControlCreatePull;
    use super::ControlCreateRepo;
    use super::ControlCreateUser;
    use super::DEFAULT_CONTROL_URL;
    use super::ENV_CONTROL_TOKEN;
    use super::ENV_CONTROL_URL;
    use super::init_observability;
    use super::ENV_STORAGE_IDLE_TIMEOUT_SECS;
    use super::ENV_STORAGE_MAX_CONNECTIONS;
    use super::ENV_STORAGE_MAX_LIFETIME_SECS;
    use super::ENV_STORAGE_MIN_CONNECTIONS;
    use super::ENV_STORAGE_READ_URL;
    use super::ForgejoRepo;
    use super::StorageConfigError;
    use super::StorageError;
    use super::control_client_from_env;
    use super::env_u32;
    use super::env_u64;
    use super::storage_from_env;
    use serde::{Deserialize, Serialize};
    use std::error::Error;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn with_env_var(key: &str, value: &str, f: &mut dyn FnMut()) {
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

    fn with_env_var_opt(key: &str, value: Option<&str>, f: &mut dyn FnMut()) {
        let previous = std::env::var_os(key);
        match value {
            Some(value) => {
                // SAFETY: tests serialize env mutation with ENV_LOCK and restore previous values.
                unsafe {
                    std::env::set_var(key, value);
                }
            }
            None => {
                // SAFETY: tests serialize env mutation with ENV_LOCK and restore previous values.
                unsafe {
                    std::env::remove_var(key);
                }
            }
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
            &mut || {
                with_env_var(ENV_STORAGE_IDLE_TIMEOUT_SECS, "", &mut || {
                    with_env_var(ENV_STORAGE_MAX_LIFETIME_SECS, "", &mut || {
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
        with_env_var(ENV_CONTROL_TOKEN, "token", &mut || {
            with_env_var(ENV_CONTROL_URL, "", &mut || {
                let config = ControlClientConfig::from_env().expect("config");
                assert_eq!(config.base_url, DEFAULT_CONTROL_URL);
            });
        });
    }

    #[test]
    fn control_config_uses_env_url() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var(ENV_CONTROL_TOKEN, "token", &mut || {
            with_env_var(ENV_CONTROL_URL, "http://localhost:9090", &mut || {
                let config = ControlClientConfig::from_env().expect("config");
                assert_eq!(config.base_url, "http://localhost:9090");
            });
        });
    }

    #[test]
    fn control_config_requires_non_empty_token() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var_opt(ENV_CONTROL_TOKEN, None, &mut || {
            let err = ControlClientConfig::from_env().expect_err("missing token");
            assert!(matches!(
                err,
                AdminError::ControlConfig(ControlConfigError::MissingEnv(ENV_CONTROL_TOKEN))
            ));
        });
        with_env_var(ENV_CONTROL_TOKEN, "   ", &mut || {
            let err = ControlClientConfig::from_env().expect_err("empty token");
            assert!(matches!(
                err,
                AdminError::ControlConfig(ControlConfigError::MissingEnv(ENV_CONTROL_TOKEN))
            ));
        });
    }

    #[test]
    fn control_client_from_env_requires_token() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var_opt(ENV_CONTROL_TOKEN, None, &mut || {
            let result = control_client_from_env();
            assert!(matches!(result, Err(AdminError::ControlConfig(_))));
        });
    }

    #[test]
    fn env_numeric_helpers_cover_missing_empty_and_invalid() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var_opt("GITTREE_TEST_U32", None, &mut || {
            assert_eq!(env_u32("GITTREE_TEST_U32").expect("missing"), None);
        });
        with_env_var("GITTREE_TEST_U32", "", &mut || {
            assert_eq!(env_u32("GITTREE_TEST_U32").expect("empty"), None);
        });
        with_env_var("GITTREE_TEST_U32", "42", &mut || {
            assert_eq!(env_u32("GITTREE_TEST_U32").expect("valid"), Some(42));
        });
        with_env_var("GITTREE_TEST_U32", "bad", &mut || {
            let err = env_u32("GITTREE_TEST_U32").expect_err("invalid");
            assert!(matches!(err, AdminError::StorageConfig(_)));
        });

        with_env_var_opt("GITTREE_TEST_U64", None, &mut || {
            assert_eq!(env_u64("GITTREE_TEST_U64").expect("missing"), None);
        });
        with_env_var("GITTREE_TEST_U64", "", &mut || {
            assert_eq!(env_u64("GITTREE_TEST_U64").expect("empty"), None);
        });
        with_env_var("GITTREE_TEST_U64", "84", &mut || {
            assert_eq!(env_u64("GITTREE_TEST_U64").expect("valid"), Some(84));
        });
        with_env_var("GITTREE_TEST_U64", "bad", &mut || {
            let err = env_u64("GITTREE_TEST_U64").expect_err("invalid");
            assert!(matches!(err, AdminError::StorageConfig(_)));
        });
    }

    #[test]
    fn storage_config_reports_missing_and_invalid_bounds() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var_opt(ENV_STORAGE_READ_URL, None, &mut || {
            let err = storage_from_env().expect_err("missing read");
            assert!(matches!(
                err,
                AdminError::StorageConfig(StorageConfigError::MissingEnv(ENV_STORAGE_READ_URL))
            ));
        });
        with_env_var(
            ENV_STORAGE_READ_URL,
            "postgres://user:pass@localhost:5432/gittree",
            &mut || {
                with_env_var(ENV_STORAGE_MAX_CONNECTIONS, "1", &mut || {
                    with_env_var(ENV_STORAGE_MIN_CONNECTIONS, "2", &mut || {
                        let err = storage_from_env().expect_err("invalid bounds");
                        assert!(matches!(err, AdminError::StorageConfig(_)));
                    });
                });
            },
        );
    }

    #[test]
    fn admin_error_display_and_sources_are_stable() {
        let cli = AdminError::Cli(super::AdminCliError::MissingCommand);
        assert!(cli.to_string().contains("admin cli error"));
        assert!(cli.source().is_some());

        let storage_cfg = AdminError::StorageConfig(StorageConfigError::MissingEnv("K"));
        assert!(
            storage_cfg
                .to_string()
                .contains("admin storage config error")
        );
        assert!(storage_cfg.source().is_some());

        let control_cfg = AdminError::ControlConfig(ControlConfigError::MissingEnv("K"));
        assert!(
            control_cfg
                .to_string()
                .contains("admin control config error")
        );
        assert!(control_cfg.source().is_some());

        let control_resp = AdminError::ControlResponse("boom".to_string());
        assert!(
            control_resp
                .to_string()
                .contains("admin control response error")
        );
        assert!(control_resp.source().is_none());

        let storage = AdminError::Storage(StorageError::Internal {
            message: "boom".to_string(),
        });
        assert!(storage.to_string().contains("admin storage error"));
        assert!(storage.source().is_some());

        let core_err = AdminError::Core(
            ForgejoRepo::parse("invalid").expect_err("invalid forgejo repo should fail"),
        );
        assert!(core_err.to_string().contains("admin mapping error"));
        assert!(core_err.source().is_some());

        let invalid_env = StorageConfigError::InvalidEnv {
            key: "KEY",
            value: "value".to_string(),
        };
        assert_eq!(invalid_env.to_string(), "invalid env KEY: value");
        let invalid_cfg = StorageConfigError::InvalidConfig("bad config".to_string());
        assert_eq!(invalid_cfg.to_string(), "bad config");
    }

    #[tokio::test]
    async fn admin_error_control_request_display_and_source_are_stable() {
        let request_error = reqwest::Client::new()
            .get("http://127.0.0.1:1/control/test")
            .send()
            .await
            .expect_err("request should fail");
        let admin_error = AdminError::ControlRequest(request_error);
        assert!(
            admin_error
                .to_string()
                .contains("admin control request error:")
        );
        assert!(admin_error.source().is_some());
    }

    #[test]
    fn with_env_helpers_restore_existing_values() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var("GITTREE_TEST_RESTORE", "before", &mut || {
            with_env_var("GITTREE_TEST_RESTORE", "during", &mut || {
                assert_eq!(
                    std::env::var("GITTREE_TEST_RESTORE").expect("during value"),
                    "during"
                );
            });
            assert_eq!(
                std::env::var("GITTREE_TEST_RESTORE").expect("restored value"),
                "before"
            );
        });

        with_env_var("GITTREE_TEST_RESTORE_OPT", "before", &mut || {
            with_env_var_opt("GITTREE_TEST_RESTORE_OPT", Some("during"), &mut || {
                assert_eq!(
                    std::env::var("GITTREE_TEST_RESTORE_OPT").expect("during value"),
                    "during"
                );
            });
            assert_eq!(
                std::env::var("GITTREE_TEST_RESTORE_OPT").expect("restored value"),
                "before"
            );
        });
    }

    #[test]
    fn init_observability_reports_reinit_error() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var("GITTREE_LOG_JSON", "false", &mut || {
            let _ = init_observability();
            let second = init_observability();
            assert!(second.is_err());
        });
    }

    fn start_mock_http_server(
        status: &str,
        content_type: &str,
        body: &str,
    ) -> (String, std::thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind server");
        let addr = listener.local_addr().expect("addr");
        let status = status.to_string();
        let content_type = content_type.to_string();
        let body = body.to_string();
        let handle = std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 4096];
                let _ = stream.read(&mut buf);
                let response = format!(
                    "HTTP/1.1 {status}\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.flush();
            }
        });
        (format!("http://{addr}"), handle)
    }

    #[derive(Debug, Serialize)]
    struct TestPostReq {
        a: u32,
    }

    #[derive(Debug, Deserialize)]
    struct TestPostResp {
        ok: bool,
    }

    #[test]
    fn control_client_endpoint_trims_trailing_slash() {
        let client = ControlClient::new(ControlClientConfig {
            base_url: "http://localhost:8088/".to_string(),
            token: "token".to_string(),
        })
        .expect("client");
        assert_eq!(
            client.endpoint("/control/users"),
            "http://localhost:8088/control/users"
        );
    }

    #[tokio::test]
    async fn control_client_post_handles_success_and_error_statuses() {
        let (base_url, ok_handle) =
            start_mock_http_server("200 OK", "application/json", "{\"ok\":true}");
        let ok_client = ControlClient::new(ControlClientConfig {
            base_url,
            token: "token".to_string(),
        })
        .expect("client");
        let ok: TestPostResp = ok_client
            .post("/control/test", &TestPostReq { a: 1 })
            .await
            .expect("post ok");
        assert!(ok.ok);
        ok_handle.join().expect("server join");

        let (base_url, err_handle) =
            start_mock_http_server("401 Unauthorized", "text/plain", "bad token");
        let err_client = ControlClient::new(ControlClientConfig {
            base_url,
            token: "token".to_string(),
        })
        .expect("client");
        let err = err_client
            .post::<_, TestPostResp>("/control/test", &TestPostReq { a: 1 })
            .await
            .expect_err("post should fail");
        assert!(matches!(err, AdminError::ControlResponse(_)));
        err_handle.join().expect("server join");
    }

    #[tokio::test]
    async fn control_client_post_reports_invalid_json() {
        let (base_url, handle) = start_mock_http_server("200 OK", "application/json", "{");
        let client = ControlClient::new(ControlClientConfig {
            base_url,
            token: "token".to_string(),
        })
        .expect("client");
        let err = client
            .post::<_, TestPostResp>("/control/test", &TestPostReq { a: 1 })
            .await
            .expect_err("invalid json");
        assert!(matches!(err, AdminError::ControlRequest(_)));
        handle.join().expect("server join");
    }

    #[tokio::test]
    async fn control_client_create_helpers_use_expected_paths() {
        let (user_url, user_handle) =
            start_mock_http_server("200 OK", "application/json", "{\"username\":\"alice\"}");
        let user_client = ControlClient::new(ControlClientConfig {
            base_url: user_url,
            token: "token".to_string(),
        })
        .expect("client");
        let user = user_client
            .create_user(ControlCreateUser {
                username: "alice".to_string(),
                email: "alice@example.com".to_string(),
                password: "secret".to_string(),
                full_name: None,
                must_change_password: None,
                send_notify: None,
            })
            .await
            .expect("create user");
        assert_eq!(user.username, "alice");
        user_handle.join().expect("server join");

        let (org_url, org_handle) = start_mock_http_server(
            "200 OK",
            "application/json",
            "{\"name\":\"acme\",\"full_name\":\"Acme\"}",
        );
        let org_client = ControlClient::new(ControlClientConfig {
            base_url: org_url,
            token: "token".to_string(),
        })
        .expect("client");
        let org = org_client
            .create_org(ControlCreateOrg {
                owner: "alice".to_string(),
                name: "acme".to_string(),
                full_name: None,
                description: None,
                visibility: None,
            })
            .await
            .expect("create org");
        assert_eq!(org.name, "acme");
        org_handle.join().expect("server join");

        let (repo_url, repo_handle) = start_mock_http_server(
            "200 OK",
            "application/json",
            "{\"owner\":\"alice\",\"name\":\"repo\"}",
        );
        let repo_client = ControlClient::new(ControlClientConfig {
            base_url: repo_url,
            token: "token".to_string(),
        })
        .expect("client");
        let repo = repo_client
            .create_repo(ControlCreateRepo {
                owner: "alice".to_string(),
                name: "repo".to_string(),
                description: None,
                private: None,
                auto_init: None,
            })
            .await
            .expect("create repo");
        assert_eq!(repo.name, "repo");
        repo_handle.join().expect("server join");

        let (pull_url, pull_handle) = start_mock_http_server(
            "200 OK",
            "application/json",
            "{\"number\":1,\"url\":\"http://example.test/pr/1\"}",
        );
        let pull_client = ControlClient::new(ControlClientConfig {
            base_url: pull_url,
            token: "token".to_string(),
        })
        .expect("client");
        let pull = pull_client
            .create_pull(ControlCreatePull {
                owner: "alice".to_string(),
                repo: "repo".to_string(),
                head: "feature".to_string(),
                base: "main".to_string(),
                title: "title".to_string(),
                body: None,
            })
            .await
            .expect("create pull");
        assert_eq!(pull.number, 1);
        pull_handle.join().expect("server join");
    }
}
