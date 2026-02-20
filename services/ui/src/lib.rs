use axum::Router;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use bech32::{Bech32, Hrp};
use gittree_config::{ConfigError, ServicesConfig, UiConfig};
use gittree_core::parse_repo_path;
use gittree_observability::{ObservabilityConfigError, ObservabilityError, ObservabilityHandle};
use gittree_storage::{
    PostgresRepositories, RepoMappingRecord, RepoMappingRepository, StorageConfig, StorageError,
};
use serde::Serialize;
use std::path::PathBuf;
use std::sync::Arc;

const ENV_STORAGE_READ_URL: &str = "GITTREE_STORAGE_READ_URL";
const ENV_STORAGE_WRITE_URL: &str = "GITTREE_STORAGE_WRITE_URL";
const ENV_STORAGE_MAX_CONNECTIONS: &str = "GITTREE_STORAGE_MAX_CONNECTIONS";
const ENV_STORAGE_MIN_CONNECTIONS: &str = "GITTREE_STORAGE_MIN_CONNECTIONS";
const ENV_STORAGE_IDLE_TIMEOUT_SECS: &str = "GITTREE_STORAGE_IDLE_TIMEOUT_SECS";
const ENV_STORAGE_MAX_LIFETIME_SECS: &str = "GITTREE_STORAGE_MAX_LIFETIME_SECS";
const ENV_STORAGE_APP_NAME: &str = "GITTREE_STORAGE_APP_NAME";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UiServiceConfig {
    pub bind: String,
    pub storage: StorageConfig,
    pub ui: UiConfig,
}

impl UiServiceConfig {
    pub fn from_env() -> Result<Self, UiServiceConfigError> {
        let services =
            ServicesConfig::from_env_validated().map_err(UiServiceConfigError::Config)?;
        let storage = storage_from_env()?;
        let ui = UiConfig::from_env().map_err(UiServiceConfigError::Config)?;
        Ok(Self {
            bind: services.ui.bind,
            storage,
            ui,
        })
    }
}

#[derive(Debug)]
pub enum UiServiceConfigError {
    Config(ConfigError),
    Storage(StorageConfigError),
    MissingEnv(&'static str),
    InvalidEnv { key: &'static str, value: String },
}

impl std::fmt::Display for UiServiceConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UiServiceConfigError::Config(err) => write!(f, "ui config error: {err}"),
            UiServiceConfigError::Storage(err) => write!(f, "ui storage config error: {err}"),
            UiServiceConfigError::MissingEnv(key) => write!(f, "missing env {key}"),
            UiServiceConfigError::InvalidEnv { key, value } => {
                write!(f, "invalid env {key}: {value}")
            }
        }
    }
}

impl std::error::Error for UiServiceConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            UiServiceConfigError::Config(err) => Some(err),
            UiServiceConfigError::Storage(err) => Some(err),
            UiServiceConfigError::MissingEnv(_) => None,
            UiServiceConfigError::InvalidEnv { .. } => None,
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

fn storage_from_env() -> Result<StorageConfig, UiServiceConfigError> {
    let read_connection = std::env::var(ENV_STORAGE_READ_URL).map_err(|_| {
        UiServiceConfigError::Storage(StorageConfigError::MissingEnv(ENV_STORAGE_READ_URL))
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
        UiServiceConfigError::Storage(StorageConfigError::InvalidConfig(err.to_string()))
    })?;

    Ok(config)
}

fn env_u32(key: &'static str) -> Result<Option<u32>, UiServiceConfigError> {
    match std::env::var(key) {
        Ok(value) => {
            if value.trim().is_empty() {
                return Ok(None);
            }
            value.parse::<u32>().map(Some).map_err(|_| {
                UiServiceConfigError::Storage(StorageConfigError::InvalidEnv { key, value })
            })
        }
        Err(_) => Ok(None),
    }
}

fn env_u64(key: &'static str) -> Result<Option<u64>, UiServiceConfigError> {
    match std::env::var(key) {
        Ok(value) => {
            if value.trim().is_empty() {
                return Ok(None);
            }
            value.parse::<u64>().map(Some).map_err(|_| {
                UiServiceConfigError::Storage(StorageConfigError::InvalidEnv { key, value })
            })
        }
        Err(_) => Ok(None),
    }
}

#[derive(Debug)]
pub enum UiError {
    Config(UiServiceConfigError),
    ObservabilityConfig(ObservabilityConfigError),
    Observability(ObservabilityError),
    Storage(StorageError),
    Serve(String),
}

impl std::fmt::Display for UiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UiError::Config(err) => write!(f, "ui error: {err}"),
            UiError::ObservabilityConfig(err) => write!(f, "ui observability config error: {err}"),
            UiError::Observability(err) => write!(f, "ui observability error: {err}"),
            UiError::Storage(err) => write!(f, "ui storage error: {err}"),
            UiError::Serve(err) => write!(f, "ui serve error: {err}"),
        }
    }
}

impl std::error::Error for UiError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            UiError::Config(err) => Some(err),
            UiError::ObservabilityConfig(err) => Some(err),
            UiError::Observability(err) => Some(err),
            UiError::Storage(err) => Some(err),
            UiError::Serve(_) => None,
        }
    }
}

pub fn init_observability() -> Result<ObservabilityHandle, UiError> {
    let config = gittree_observability::ObservabilityConfig::from_env("gittree-ui")
        .map_err(UiError::ObservabilityConfig)?;
    let handle = gittree_observability::init(&config).map_err(UiError::Observability)?;
    Ok(handle)
}

pub fn build_repositories(config: &UiServiceConfig) -> Result<PostgresRepositories, UiError> {
    let pool_options = config.storage.pool_options().map_err(UiError::Storage)?;
    let connect_options = config
        .storage
        .read_connect_options()
        .map_err(UiError::Storage)?;
    let pool = pool_options.connect_lazy_with(connect_options);
    Ok(PostgresRepositories::new(pool))
}

struct UiAppState<R> {
    repositories: Arc<R>,
    repo_root: PathBuf,
    public_git_url: String,
}

impl<R> Clone for UiAppState<R> {
    fn clone(&self) -> Self {
        Self {
            repositories: Arc::clone(&self.repositories),
            repo_root: self.repo_root.clone(),
            public_git_url: self.public_git_url.clone(),
        }
    }
}

pub async fn serve(config: UiServiceConfig) -> Result<(), UiError> {
    let _observability = init_observability()?;
    let repositories = build_repositories(&config)?;
    let state = UiAppState {
        repositories: Arc::new(repositories),
        repo_root: config.ui.repo_root,
        public_git_url: config.ui.public_git_url,
    };
    let router = build_router(state);
    let listener = tokio::net::TcpListener::bind(&config.bind)
        .await
        .map_err(|err| UiError::Serve(err.to_string()))?;
    axum::serve(listener, router)
        .await
        .map_err(|err| UiError::Serve(err.to_string()))?;
    Ok(())
}

fn build_router<R>(state: UiAppState<R>) -> Router
where
    R: RepoMappingRepository + Send + Sync + 'static,
{
    Router::new()
        .route("/health", get(health_handler))
        .route("/", get(index_handler))
        .route("/:npub/:identifier", get(repo_handler))
        .with_state(Arc::new(state))
}

async fn health_handler() -> &'static str {
    "ok"
}

#[derive(Debug, Serialize)]
struct RepoListItem {
    npub: String,
    identifier: String,
    forgejo: String,
    clone_url: String,
}

#[derive(Debug)]
enum UiHttpError {
    BadRequest(String),
    NotFound(String),
    Storage(String),
}

impl IntoResponse for UiHttpError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            UiHttpError::BadRequest(message) => (StatusCode::BAD_REQUEST, message),
            UiHttpError::NotFound(message) => (StatusCode::NOT_FOUND, message),
            UiHttpError::Storage(message) => (StatusCode::INTERNAL_SERVER_ERROR, message),
        };
        (status, message).into_response()
    }
}

async fn index_handler<R>(
    State(state): State<Arc<UiAppState<R>>>,
) -> Result<Html<String>, UiHttpError>
where
    R: RepoMappingRepository + Send + Sync,
{
    let mappings = state
        .repositories
        .list_mappings()
        .await
        .map_err(|err| UiHttpError::Storage(err.to_string()))?;
    let mut items = Vec::with_capacity(mappings.len());
    for mapping in mappings {
        items.push(repo_list_item(&state.public_git_url, mapping)?);
    }
    Ok(Html(render_index(&items)))
}

async fn repo_handler<R>(
    State(state): State<Arc<UiAppState<R>>>,
    Path((npub, identifier)): Path<(String, String)>,
) -> Result<Html<String>, UiHttpError>
where
    R: RepoMappingRepository + Send + Sync,
{
    let identifier = identifier.strip_suffix(".git").unwrap_or(&identifier);
    let repo_path = state
        .repo_root
        .join(&npub)
        .join(format!("{identifier}.git"));
    let parsed =
        parse_repo_path(&repo_path).map_err(|err| UiHttpError::BadRequest(err.to_string()))?;
    let pubkey_bytes = hex::decode(&parsed.pubkey)
        .map_err(|_| UiHttpError::BadRequest("invalid pubkey".to_string()))?;
    let mapping = state
        .repositories
        .mapping_by_repo(&pubkey_bytes, &parsed.identifier)
        .await
        .map_err(|err| UiHttpError::Storage(err.to_string()))?
        .ok_or_else(|| UiHttpError::NotFound("missing repo mapping".to_string()))?;
    let item = repo_list_item(&state.public_git_url, mapping)?;
    Ok(Html(render_repo(&item)))
}

fn repo_list_item(
    public_git_url: &str,
    mapping: RepoMappingRecord,
) -> Result<RepoListItem, UiHttpError> {
    let npub = npub_from_bytes(&mapping.pubkey);
    let forgejo = mapping.forgejo_full_name();
    let identifier = mapping.identifier;
    let clone_url = format!(
        "{}/{npub}/{}.git",
        public_git_url.trim_end_matches('/'),
        identifier
    );
    Ok(RepoListItem {
        npub,
        identifier,
        forgejo,
        clone_url,
    })
}

fn npub_from_bytes(bytes: &[u8]) -> String {
    let hrp = Hrp::parse("npub").expect("static npub hrp");
    bech32::encode::<Bech32>(hrp, bytes).expect("bech32 encode npub")
}

fn render_index(items: &[RepoListItem]) -> String {
    let mut html = String::new();
    html.push_str("<!doctype html><html><head><meta charset=\"utf-8\">");
    html.push_str("<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">");
    html.push_str("<title>gittree</title><style>");
    html.push_str(
        ":root{--bg1:#f4efe4;--bg2:#e8f0f4;--ink:#1c1c1c;--muted:#4b5a66;--accent:#c36b1b;}",
    );
    html.push_str("body{margin:0;font-family:'IBM Plex Mono','Fira Mono','Menlo',monospace;background:linear-gradient(120deg,var(--bg1),var(--bg2));color:var(--ink);}");
    html.push_str("main{max-width:960px;margin:48px auto;padding:32px;background:rgba(255,255,255,0.9);border:1px solid rgba(0,0,0,0.08);box-shadow:0 18px 30px rgba(0,0,0,0.08);}");
    html.push_str("h1{margin:0 0 12px;font-size:28px;letter-spacing:0.02em;}");
    html.push_str("p{margin:0 0 24px;color:var(--muted);}ul{list-style:none;padding:0;margin:0;}");
    html.push_str("li{padding:14px 0;border-bottom:1px solid rgba(0,0,0,0.08);}li:last-child{border-bottom:none;}");
    html.push_str(
        "a{color:var(--accent);text-decoration:none;}a:hover{text-decoration:underline;}",
    );
    html.push_str(
        ".meta{font-size:13px;color:var(--muted);} .clone{font-size:13px;word-break:break-all;}",
    );
    html.push_str("</style></head><body><main>");
    html.push_str("<h1>gittree</h1><p>nostr repositories synced through gittree.</p>");
    if items.is_empty() {
        html.push_str("<p class=\"meta\">no repositories yet.</p>");
    } else {
        html.push_str("<ul>");
        for item in items {
            html.push_str("<li>");
            html.push_str(&format!(
                "<div><a href=\"/{}/{}\">{}</a></div>",
                item.npub, item.identifier, item.identifier
            ));
            html.push_str(&format!("<div class=\"meta\">{}</div>", item.npub));
            html.push_str(&format!(
                "<div class=\"meta\">forgejo: {}</div>",
                item.forgejo
            ));
            html.push_str(&format!(
                "<div class=\"clone\">clone: {}</div>",
                item.clone_url
            ));
            html.push_str("</li>");
        }
        html.push_str("</ul>");
    }
    html.push_str("</main></body></html>");
    html
}

fn render_repo(item: &RepoListItem) -> String {
    let mut html = String::new();
    html.push_str("<!doctype html><html><head><meta charset=\"utf-8\">");
    html.push_str("<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">");
    html.push_str(&format!("<title>{}</title><style>", item.identifier));
    html.push_str(
        ":root{--bg1:#f4efe4;--bg2:#e8f0f4;--ink:#1c1c1c;--muted:#4b5a66;--accent:#c36b1b;}",
    );
    html.push_str("body{margin:0;font-family:'IBM Plex Mono','Fira Mono','Menlo',monospace;background:linear-gradient(120deg,var(--bg1),var(--bg2));color:var(--ink);}");
    html.push_str("main{max-width:960px;margin:48px auto;padding:32px;background:rgba(255,255,255,0.9);border:1px solid rgba(0,0,0,0.08);box-shadow:0 18px 30px rgba(0,0,0,0.08);}");
    html.push_str(
        "a{color:var(--accent);text-decoration:none;}a:hover{text-decoration:underline;}",
    );
    html.push_str(
        ".meta{font-size:13px;color:var(--muted);} .clone{font-size:13px;word-break:break-all;}",
    );
    html.push_str("</style></head><body><main>");
    html.push_str(&format!("<h1>{}</h1>", item.identifier));
    html.push_str(&format!("<p class=\"meta\">{}</p>", item.npub));
    html.push_str(&format!("<p class=\"meta\">forgejo: {}</p>", item.forgejo));
    html.push_str(&format!("<p class=\"clone\">clone: {}</p>", item.clone_url));
    html.push_str("<p><a href=\"/\">back to list</a></p>");
    html.push_str("</main></body></html>");
    html
}

#[cfg(test)]
mod tests {
    use super::{
        StorageConfigError, UiAppState, UiError, UiHttpError, UiServiceConfig,
        UiServiceConfigError, build_repositories, build_router, init_observability,
        npub_from_bytes, render_index, render_repo, repo_list_item,
    };
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::response::IntoResponse;
    use gittree_config::{ConfigError, UiConfig};
    use gittree_core::RepoMapping;
    use gittree_observability::{
        ObservabilityConfigError, ObservabilityError, ObservabilityHandle,
    };
    use gittree_storage::{
        InMemoryRepositories, RepoMappingRecord, RepoMappingRepository, StorageConfig, StorageError,
    };
    use std::error::Error;
    use std::sync::{Arc, Mutex, OnceLock};
    use tower::ServiceExt;

    static OBSERVABILITY: OnceLock<ObservabilityHandle> = OnceLock::new();

    fn with_env_vars<F: FnOnce()>(vars: &[(&str, Option<&str>)], f: F) {
        static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        let _guard = ENV_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .expect("env lock");
        let previous: Vec<(String, Option<std::ffi::OsString>)> = vars
            .iter()
            .map(|(key, _)| ((*key).to_string(), std::env::var_os(key)))
            .collect();

        for (key, value) in vars {
            match value {
                Some(value) => unsafe {
                    std::env::set_var(key, value);
                },
                None => unsafe {
                    std::env::remove_var(key);
                },
            }
        }

        f();

        for (key, value) in previous.into_iter().rev() {
            match value {
                Some(value) => unsafe {
                    std::env::set_var(&key, value);
                },
                None => unsafe {
                    std::env::remove_var(&key);
                },
            }
        }
    }

    fn with_minimum_config_env<F: FnOnce()>(extra: &[(&str, Option<&str>)], f: F) {
        let mut vars = vec![
            (
                "GITTREE_STORAGE_READ_URL",
                Some("postgres://user:pass@localhost:5432/gittree"),
            ),
            ("GITTREE_UI_REPO_ROOT", Some("/tmp/gittree")),
            ("GITTREE_UI_PUBLIC_GIT_URL", Some("http://localhost:8085")),
            ("GITTREE_UI_BIND", Some("127.0.0.1:9090")),
        ];
        vars.extend_from_slice(extra);
        with_env_vars(&vars, f);
    }

    fn test_ui_config() -> UiConfig {
        UiConfig {
            repo_root: "/tmp/gittree".into(),
            public_git_url: "http://localhost:8085".to_string(),
            auth_url: "http://localhost:8089".to_string(),
            app_url: "http://localhost:8090".to_string(),
            control_url: "http://localhost:8088".to_string(),
        }
    }

    fn test_storage_config() -> StorageConfig {
        StorageConfig {
            read_connection: "postgres://user:pass@localhost:5432/gittree".to_string(),
            write_connection: None,
            max_connections: 10,
            min_connections: 1,
            idle_timeout_secs: None,
            max_lifetime_secs: None,
            application_name: None,
        }
    }

    #[test]
    fn config_loads_from_env() {
        with_minimum_config_env(&[], || {
            let config = UiServiceConfig::from_env().expect("config");
            assert_eq!(config.bind, "127.0.0.1:9090");
            assert_eq!(config.ui.public_git_url, "http://localhost:8085");
        });
    }

    #[test]
    fn config_reports_storage_env_errors() {
        with_minimum_config_env(&[("GITTREE_STORAGE_READ_URL", None)], || {
            let missing = UiServiceConfig::from_env().expect_err("missing read url");
            assert!(matches!(
                missing,
                UiServiceConfigError::Storage(StorageConfigError::MissingEnv(
                    "GITTREE_STORAGE_READ_URL"
                ))
            ));
        });

        with_minimum_config_env(
            &[("GITTREE_STORAGE_MAX_CONNECTIONS", Some("invalid"))],
            || {
                let invalid = UiServiceConfig::from_env().expect_err("invalid max connections");
                assert!(matches!(
                    invalid,
                    UiServiceConfigError::Storage(StorageConfigError::InvalidEnv {
                        key: "GITTREE_STORAGE_MAX_CONNECTIONS",
                        ..
                    })
                ));
            },
        );
    }

    #[test]
    fn config_and_ui_errors_display_and_source_paths_are_stable() {
        let config = UiServiceConfigError::Config(ConfigError::InvalidConfig {
            field: "ui.repo_root",
            value: "bad".to_string(),
        });
        assert!(format!("{config}").contains("ui config error"));
        assert!(config.source().is_some());

        let storage = UiServiceConfigError::Storage(StorageConfigError::MissingEnv("READ"));
        assert!(format!("{storage}").contains("ui storage config error"));
        assert!(storage.source().is_some());

        let missing = UiServiceConfigError::MissingEnv("KEY");
        assert_eq!(format!("{missing}"), "missing env KEY");
        assert!(missing.source().is_none());

        let invalid = UiServiceConfigError::InvalidEnv {
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

        let ui_error = UiError::Config(UiServiceConfigError::MissingEnv("ENV"));
        assert!(format!("{ui_error}").contains("ui error"));
        assert!(ui_error.source().is_some());

        let observability_config =
            UiError::ObservabilityConfig(ObservabilityConfigError::InvalidEnv {
                key: "GITTREE_LOG_JSON",
                value: "nope".to_string(),
            });
        assert!(format!("{observability_config}").contains("observability config error"));
        assert!(observability_config.source().is_some());

        let observability =
            UiError::Observability(ObservabilityError::MetricsInit("bad".to_string()));
        assert!(format!("{observability}").contains("observability error"));
        assert!(observability.source().is_some());

        let storage_error = UiError::Storage(StorageError::Internal {
            message: "db".to_string(),
        });
        assert!(format!("{storage_error}").contains("ui storage error"));
        assert!(storage_error.source().is_some());

        let serve = UiError::Serve("bind".to_string());
        assert_eq!(format!("{serve}"), "ui serve error: bind");
        assert!(serve.source().is_none());
    }

    #[test]
    fn ui_http_error_maps_all_status_codes() {
        assert_eq!(
            UiHttpError::BadRequest("bad".to_string())
                .into_response()
                .status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            UiHttpError::NotFound("missing".to_string())
                .into_response()
                .status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            UiHttpError::Storage("storage".to_string())
                .into_response()
                .status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }

    #[test]
    fn state_clone_and_render_helpers_cover_empty_and_detail_paths() {
        let repositories = Arc::new(InMemoryRepositories::new());
        let state = UiAppState {
            repositories,
            repo_root: "/tmp/gittree".into(),
            public_git_url: "http://localhost:8085/".to_string(),
        };
        let cloned = state.clone();
        assert_eq!(cloned.repo_root, state.repo_root);
        assert_eq!(cloned.public_git_url, state.public_git_url);

        let empty = render_index(&[]);
        assert!(empty.contains("no repositories yet."));

        let mapping = RepoMapping::new("owner", "repo", "11".repeat(32), "repo").expect("mapping");
        let record = RepoMappingRecord::new(&mapping).expect("record");
        let item = repo_list_item("http://localhost:8085/", record).expect("item");
        assert_eq!(
            item.clone_url,
            "http://localhost:8085".to_string() + "/" + &item.npub + "/repo.git"
        );
        let detail = render_repo(&item);
        assert!(detail.contains("back to list"));
        assert!(detail.contains("owner/repo"));
    }

    #[test]
    fn npub_from_bytes_encodes_payloads() {
        let encoded = npub_from_bytes(&[0x11; 32]);
        assert!(encoded.starts_with("npub1"));
    }

    #[tokio::test]
    async fn build_repositories_maps_invalid_pool_config() {
        let config = UiServiceConfig {
            bind: "127.0.0.1:9090".to_string(),
            storage: StorageConfig {
                max_connections: 0,
                min_connections: 0,
                ..test_storage_config()
            },
            ui: test_ui_config(),
        };
        let err = build_repositories(&config).expect_err("invalid pool");
        assert!(matches!(err, UiError::Storage(_)));
    }

    #[test]
    fn init_observability_returns_registry_once() {
        let handle = OBSERVABILITY.get_or_init(|| init_observability().expect("init"));
        assert!(handle.prometheus_registry().is_some());
    }

    #[test]
    fn with_env_var_restores_previous_values() {
        unsafe {
            std::env::set_var("GITTREE_UI_BIND", "127.0.0.1:7777");
        }
        with_env_vars(&[("GITTREE_UI_BIND", Some("127.0.0.1:9090"))], || {
            assert_eq!(
                std::env::var("GITTREE_UI_BIND").as_deref(),
                Ok("127.0.0.1:9090")
            );
        });
        assert_eq!(
            std::env::var("GITTREE_UI_BIND").as_deref(),
            Ok("127.0.0.1:7777")
        );
        unsafe {
            std::env::remove_var("GITTREE_UI_BIND");
        }
    }

    #[tokio::test]
    async fn health_endpoint_returns_ok() {
        let repositories = Arc::new(InMemoryRepositories::new());
        let state = UiAppState {
            repositories,
            repo_root: "/tmp/gittree".into(),
            public_git_url: "http://localhost:8085".to_string(),
        };
        let app = build_router(state);
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
    async fn index_lists_mappings() {
        let repositories = Arc::new(InMemoryRepositories::new());
        let mapping = RepoMapping::new("owner", "repo", "11".repeat(32), "repo").expect("mapping");
        let record = RepoMappingRecord::new(&mapping).expect("record");
        repositories
            .upsert_mapping(record)
            .await
            .expect("insert mapping");

        let state = UiAppState {
            repositories: repositories.clone(),
            repo_root: "/tmp/gittree".into(),
            public_git_url: "http://localhost:8085".to_string(),
        };
        let app = build_router(state);
        let response = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .expect("response");
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let body = String::from_utf8(body.to_vec()).expect("utf8");
        assert!(body.contains("repo"));
        assert!(body.contains("owner/repo"));
        assert!(body.contains("http://localhost:8085"));
    }

    #[tokio::test]
    async fn repo_page_renders_detail() {
        let repositories = Arc::new(InMemoryRepositories::new());
        let mapping = RepoMapping::new("owner", "repo", "11".repeat(32), "repo").expect("mapping");
        let record = RepoMappingRecord::new(&mapping).expect("record");
        let npub = npub_from_bytes(&record.pubkey);
        repositories
            .upsert_mapping(record)
            .await
            .expect("insert mapping");

        let state = UiAppState {
            repositories: repositories.clone(),
            repo_root: "/tmp/gittree".into(),
            public_git_url: "http://localhost:8085".to_string(),
        };
        let app = build_router(state);
        let response = app
            .oneshot(
                Request::builder()
                    .uri(format!("/{npub}/repo.git"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let body = String::from_utf8(body.to_vec()).expect("utf8");
        assert!(body.contains("clone"));
        assert!(body.contains("owner/repo"));
    }

    #[tokio::test]
    async fn repo_page_reports_bad_request_and_missing_mapping() {
        let repositories = Arc::new(InMemoryRepositories::new());
        let state = UiAppState {
            repositories: repositories.clone(),
            repo_root: "/tmp/gittree".into(),
            public_git_url: "http://localhost:8085".to_string(),
        };
        let app = build_router(state);

        let bad_request = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/invalid-npub/repo")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(bad_request.status(), StatusCode::BAD_REQUEST);

        let npub = npub_from_bytes(&[0x22; 32]);
        let missing = app
            .oneshot(
                Request::builder()
                    .uri(format!("/{npub}/missing"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    }
}
