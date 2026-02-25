#![forbid(unsafe_code)]

use axum::Json;
use axum::Router;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use gittree_app_core::{RepoDetail, RepoListResponse};
use gittree_app_ui::AppUiState;
use gittree_app_ui::server::{
    AppUiError, list_repo_items, list_repo_items_for_npub, repo_detail_item,
};
use gittree_config::{ConfigError, UiConfig};
use gittree_observability::{ObservabilityConfigError, ObservabilityError, ObservabilityHandle};
use gittree_storage::{
    PostgresRepositories, ProfileRepository, RepoMappingRepository, StorageConfig, StorageError,
};
use leptos::config::LeptosOptions;
use leptos::prelude::provide_context;
use leptos_axum::{LeptosRoutes, handle_server_fns_with_context};
use std::future::Future;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

const ENV_APP_BIND: &str = "GITTREE_APP_BIND";
const ENV_APP_BASE_PATH: &str = "GITTREE_APP_BASE_PATH";
const ENV_APP_SITE_ROOT: &str = "GITTREE_APP_SITE_ROOT";
const ENV_APP_SITE_PKG_DIR: &str = "GITTREE_APP_SITE_PKG_DIR";

const ENV_STORAGE_READ_URL: &str = "GITTREE_STORAGE_READ_URL";
const ENV_STORAGE_WRITE_URL: &str = "GITTREE_STORAGE_WRITE_URL";
const ENV_STORAGE_MAX_CONNECTIONS: &str = "GITTREE_STORAGE_MAX_CONNECTIONS";
const ENV_STORAGE_MIN_CONNECTIONS: &str = "GITTREE_STORAGE_MIN_CONNECTIONS";
const ENV_STORAGE_IDLE_TIMEOUT_SECS: &str = "GITTREE_STORAGE_IDLE_TIMEOUT_SECS";
const ENV_STORAGE_MAX_LIFETIME_SECS: &str = "GITTREE_STORAGE_MAX_LIFETIME_SECS";
const ENV_STORAGE_APP_NAME: &str = "GITTREE_STORAGE_APP_NAME";

const DEFAULT_APP_BIND: &str = "127.0.0.1:8090";
const DEFAULT_APP_BASE_PATH: &str = "/ui";
const DEFAULT_APP_SITE_ROOT: &str = "crates/app-ui/dist";
const DEFAULT_APP_SITE_PKG_DIR: &str = "pkg";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppServiceConfig {
    pub bind: SocketAddr,
    pub base_path: String,
    pub site_root: PathBuf,
    pub site_pkg_dir: String,
    pub storage: StorageConfig,
    pub ui: UiConfig,
}

impl AppServiceConfig {
    pub fn from_env() -> Result<Self, AppServiceConfigError> {
        let storage = storage_from_env()?;
        let ui = UiConfig::from_env().map_err(AppServiceConfigError::Config)?;

        let bind = env_socket_addr(ENV_APP_BIND)?
            .unwrap_or_else(|| DEFAULT_APP_BIND.parse().expect("default app bind"));
        let base_path =
            env_string(ENV_APP_BASE_PATH)?.unwrap_or_else(|| DEFAULT_APP_BASE_PATH.to_string());
        let base_path = normalize_base_path(&base_path);
        let site_root =
            env_path(ENV_APP_SITE_ROOT)?.unwrap_or_else(|| PathBuf::from(DEFAULT_APP_SITE_ROOT));
        let site_pkg_dir = env_string(ENV_APP_SITE_PKG_DIR)?
            .unwrap_or_else(|| DEFAULT_APP_SITE_PKG_DIR.to_string());

        Ok(Self {
            bind,
            base_path,
            site_root,
            site_pkg_dir,
            storage,
            ui,
        })
    }
}

#[derive(Debug)]
pub enum AppServiceConfigError {
    Config(ConfigError),
    Storage(StorageConfigError),
    MissingEnv(&'static str),
    InvalidEnv { key: &'static str, value: String },
}

impl std::fmt::Display for AppServiceConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AppServiceConfigError::Config(err) => write!(f, "app config error: {err}"),
            AppServiceConfigError::Storage(err) => write!(f, "app storage config error: {err}"),
            AppServiceConfigError::MissingEnv(key) => write!(f, "missing env {key}"),
            AppServiceConfigError::InvalidEnv { key, value } => {
                write!(f, "invalid env {key}: {value}")
            }
        }
    }
}

impl std::error::Error for AppServiceConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            AppServiceConfigError::Config(err) => Some(err),
            AppServiceConfigError::Storage(err) => Some(err),
            AppServiceConfigError::MissingEnv(_) => None,
            AppServiceConfigError::InvalidEnv { .. } => None,
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

fn storage_from_env() -> Result<StorageConfig, AppServiceConfigError> {
    let read_connection = std::env::var(ENV_STORAGE_READ_URL).map_err(|_| {
        AppServiceConfigError::Storage(StorageConfigError::MissingEnv(ENV_STORAGE_READ_URL))
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
        AppServiceConfigError::Storage(StorageConfigError::InvalidConfig(err.to_string()))
    })?;

    Ok(config)
}

fn env_u32(key: &'static str) -> Result<Option<u32>, AppServiceConfigError> {
    match std::env::var(key) {
        Ok(value) => {
            if value.trim().is_empty() {
                return Ok(None);
            }
            value.parse::<u32>().map(Some).map_err(|_| {
                AppServiceConfigError::Storage(StorageConfigError::InvalidEnv { key, value })
            })
        }
        Err(_) => Ok(None),
    }
}

fn env_u64(key: &'static str) -> Result<Option<u64>, AppServiceConfigError> {
    match std::env::var(key) {
        Ok(value) => {
            if value.trim().is_empty() {
                return Ok(None);
            }
            value.parse::<u64>().map(Some).map_err(|_| {
                AppServiceConfigError::Storage(StorageConfigError::InvalidEnv { key, value })
            })
        }
        Err(_) => Ok(None),
    }
}

fn env_socket_addr(key: &'static str) -> Result<Option<SocketAddr>, AppServiceConfigError> {
    match std::env::var(key) {
        Ok(value) => {
            if value.trim().is_empty() {
                return Ok(None);
            }
            value
                .parse::<SocketAddr>()
                .map(Some)
                .map_err(|_| AppServiceConfigError::InvalidEnv { key, value })
        }
        Err(_) => Ok(None),
    }
}

fn env_string(key: &'static str) -> Result<Option<String>, AppServiceConfigError> {
    match std::env::var(key) {
        Ok(value) => {
            if value.trim().is_empty() {
                return Ok(None);
            }
            Ok(Some(value))
        }
        Err(_) => Ok(None),
    }
}

fn env_path(key: &'static str) -> Result<Option<PathBuf>, AppServiceConfigError> {
    env_string(key).map(|value| value.map(PathBuf::from))
}

fn normalize_base_path(base_path: &str) -> String {
    let trimmed = base_path.trim();
    if trimmed.is_empty() || trimmed == "/" {
        return "/".to_string();
    }
    let trimmed = trimmed.trim_end_matches('/');
    if trimmed.starts_with('/') {
        trimmed.to_string()
    } else {
        format!("/{trimmed}")
    }
}

#[derive(Debug)]
pub enum AppError {
    Config(AppServiceConfigError),
    ObservabilityConfig(ObservabilityConfigError),
    Observability(ObservabilityError),
    Storage(StorageError),
    Serve(String),
}

impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AppError::Config(err) => write!(f, "app error: {err}"),
            AppError::ObservabilityConfig(err) => {
                write!(f, "app observability config error: {err}")
            }
            AppError::Observability(err) => write!(f, "app observability error: {err}"),
            AppError::Storage(err) => write!(f, "app storage error: {err}"),
            AppError::Serve(err) => write!(f, "app serve error: {err}"),
        }
    }
}

impl std::error::Error for AppError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            AppError::Config(err) => Some(err),
            AppError::ObservabilityConfig(err) => Some(err),
            AppError::Observability(err) => Some(err),
            AppError::Storage(err) => Some(err),
            AppError::Serve(_) => None,
        }
    }
}

pub fn init_observability() -> Result<ObservabilityHandle, AppError> {
    let config = gittree_observability::ObservabilityConfig::from_env("gittree-app")
        .map_err(AppError::ObservabilityConfig)?;
    let handle = gittree_observability::init(&config).map_err(AppError::Observability)?;
    Ok(handle)
}

pub fn build_repositories(config: &AppServiceConfig) -> Result<PostgresRepositories, AppError> {
    let pool_options = config.storage.pool_options().map_err(AppError::Storage)?;
    let connect_options = config
        .storage
        .read_connect_options()
        .map_err(AppError::Storage)?;
    let pool = pool_options.connect_lazy_with(connect_options);
    Ok(PostgresRepositories::new(pool))
}

async fn serve_with<InitFn, InitOut, ServeFn, ServeFut>(
    config: AppServiceConfig,
    init_fn: InitFn,
    serve_fn: ServeFn,
) -> Result<(), AppError>
where
    InitFn: FnOnce() -> Result<InitOut, AppError>,
    ServeFn: FnOnce(tokio::net::TcpListener, Router) -> ServeFut,
    ServeFut: Future<Output = Result<(), std::io::Error>>,
{
    let _observability = init_fn()?;
    let repositories = build_repositories(&config)?;
    let leptos_options = build_leptos_options(&config);
    let repositories = Arc::new(repositories);
    let repo_mappings: Arc<dyn RepoMappingRepository> = repositories.clone();
    let profiles: Arc<dyn ProfileRepository> = repositories.clone();
    let state = AppUiState::new(
        repo_mappings,
        profiles,
        config.ui.repo_root,
        config.ui.public_git_url,
        config.ui.auth_url,
        config.ui.app_url,
        config.ui.control_url,
        config.base_path,
        leptos_options,
    );

    let router = build_router(state.clone());
    let listener = tokio::net::TcpListener::bind(state.leptos_options.site_addr)
        .await
        .map_err(|err| AppError::Serve(err.to_string()))?;
    serve_fn(listener, router)
        .await
        .map_err(|err| AppError::Serve(err.to_string()))?;
    Ok(())
}

pub async fn serve(config: AppServiceConfig) -> Result<(), AppError> {
    serve_with(config, init_observability, run_axum_server).await
}

fn run_axum_server(
    listener: tokio::net::TcpListener,
    router: Router,
) -> impl Future<Output = Result<(), std::io::Error>> {
    async move { axum::serve(listener, router).await }
}

fn build_leptos_options(config: &AppServiceConfig) -> LeptosOptions {
    LeptosOptions::builder()
        .output_name("gittree-app-ui")
        .site_root(config.site_root.to_string_lossy())
        .site_pkg_dir(config.site_pkg_dir.clone())
        .site_addr(config.bind)
        .build()
}

fn build_router(state: AppUiState) -> Router {
    let routes = leptos_axum::generate_route_list(gittree_app_ui::GittreeApp);
    let base_path = state.base_path.clone();
    let shell = |_options: LeptosOptions| gittree_app_ui::GittreeApp();

    let app = Router::new()
        .route("/health", get(health_handler))
        .route("/api/repos", get(api_list_repos_handler))
        .route(
            "/api/repos/{npub}/{identifier}",
            get(api_repo_detail_handler),
        )
        .route(
            "/api/users/{npub}/repos",
            get(api_list_repos_by_owner_handler),
        )
        .route("/api/{*fn_name}", post(server_fn_route_handler))
        .leptos_routes_with_context(&state, routes, provide_empty_context, gittree_app_ui::GittreeApp)
        .fallback(leptos_axum::file_and_error_handler::<AppUiState, _>(shell));

    let app = app.with_state(state);

    if base_path == "/" {
        app
    } else {
        Router::new().nest(&base_path, app)
    }
}

fn provide_empty_context() {}

async fn server_fn_route_handler(
    State(state): State<AppUiState>,
    req: axum::extract::Request,
) -> axum::response::Response {
    handle_server_fns_with_context(move || provide_context(state.clone()), req)
        .await
        .into_response()
}

#[derive(Debug)]
enum AppApiError {
    Ui(AppUiError),
}

impl From<AppUiError> for AppApiError {
    fn from(value: AppUiError) -> Self {
        AppApiError::Ui(value)
    }
}

impl IntoResponse for AppApiError {
    fn into_response(self) -> axum::response::Response {
        let (status, message) = match self {
            AppApiError::Ui(AppUiError::BadRequest(message)) => (StatusCode::BAD_REQUEST, message),
            AppApiError::Ui(AppUiError::NotFound(message)) => (StatusCode::NOT_FOUND, message),
            AppApiError::Ui(AppUiError::Storage(message)) => {
                (StatusCode::INTERNAL_SERVER_ERROR, message)
            }
            AppApiError::Ui(AppUiError::Internal(message)) => {
                (StatusCode::INTERNAL_SERVER_ERROR, message)
            }
        };
        (status, message).into_response()
    }
}

async fn api_list_repos_handler(
    State(state): State<AppUiState>,
) -> Result<Json<RepoListResponse>, AppApiError> {
    let items = list_repo_items(&state).await?;
    Ok(Json(RepoListResponse { items }))
}

async fn api_list_repos_by_owner_handler(
    State(state): State<AppUiState>,
    Path(npub): Path<String>,
) -> Result<Json<RepoListResponse>, AppApiError> {
    let items = list_repo_items_for_npub(&state, &npub).await?;
    Ok(Json(RepoListResponse { items }))
}

async fn api_repo_detail_handler(
    State(state): State<AppUiState>,
    Path((npub, identifier)): Path<(String, String)>,
) -> Result<Json<RepoDetail>, AppApiError> {
    let detail = repo_detail_item(&state, &npub, &identifier).await?;
    Ok(Json(detail))
}

async fn health_handler() -> &'static str {
    "ok"
}

#[cfg(test)]
mod tests {
    use super::{
        AppApiError, AppError, AppServiceConfig, AppServiceConfigError, AppUiState,
        StorageConfigError, build_router, run_axum_server, server_fn_route_handler,
    };
    use async_trait::async_trait;
    use axum::body::Body;
    use axum::extract::{Path, State};
    use axum::http::{Method, Request, StatusCode};
    use axum::response::IntoResponse;
    use axum::routing::get;
    use gittree_app_core::RepoListResponse;
    use gittree_app_ui::server::AppUiError;
    use gittree_config::{ConfigError, UiConfig};
    use gittree_observability::{ObservabilityConfigError, ObservabilityError};
    use gittree_storage::{
        InMemoryRepositories, ProfileRecord, ProfileRepository, ProfileVisibility,
        RepoMappingRecord, RepoMappingRepository, StorageConfig, StorageError,
    };
    use leptos::config::LeptosOptions;
    use std::error::Error;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tower::util::ServiceExt;

    fn test_ui_config() -> UiConfig {
        UiConfig {
            repo_root: PathBuf::from("/tmp/gittree"),
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

    fn test_state(
        repositories: Arc<dyn RepoMappingRepository>,
        profiles: Arc<dyn ProfileRepository>,
    ) -> AppUiState {
        AppUiState::new(
            repositories,
            profiles,
            "/tmp/gittree".into(),
            "http://localhost:8085".to_string(),
            "http://localhost:8089".to_string(),
            "http://localhost:8090".to_string(),
            "http://localhost:8088".to_string(),
            "/".to_string(),
            LeptosOptions::builder()
                .output_name("gittree-app-ui")
                .site_root("crates/app-ui/dist")
                .site_pkg_dir("pkg")
                .site_addr("127.0.0.1:0".parse::<std::net::SocketAddr>().expect("addr"))
                .build(),
        )
    }

    #[derive(Clone, Default)]
    struct FailingListMappings;

    #[async_trait]
    impl RepoMappingRepository for FailingListMappings {
        async fn upsert_mapping(&self, _record: RepoMappingRecord) -> Result<(), StorageError> {
            Ok(())
        }

        async fn mapping_by_forgejo(
            &self,
            _owner: &str,
            _repo: &str,
        ) -> Result<Option<RepoMappingRecord>, StorageError> {
            Ok(None)
        }

        async fn mapping_by_repo(
            &self,
            _pubkey: &[u8],
            _identifier: &str,
        ) -> Result<Option<RepoMappingRecord>, StorageError> {
            Ok(None)
        }

        async fn list_mappings(&self) -> Result<Vec<RepoMappingRecord>, StorageError> {
            Err(StorageError::Internal {
                message: "list mappings failed".to_string(),
            })
        }
    }

    fn pubkey_hex(byte: u8) -> String {
        format!("{:02x}", byte).repeat(32)
    }

    #[test]
    fn normalize_base_path_handles_edge_cases() {
        assert_eq!(super::normalize_base_path(""), "/");
        assert_eq!(super::normalize_base_path("/"), "/");
        assert_eq!(super::normalize_base_path("ui"), "/ui");
        assert_eq!(super::normalize_base_path("/ui/"), "/ui");
    }

    #[test]
    fn provide_empty_context_is_callable() {
        super::provide_empty_context();
    }

    #[test]
    fn env_helpers_return_none_for_missing_key_without_env_mutation() {
        let missing_key: &'static str = Box::leak(
            (0..)
                .map(|index| format!("GITTREE_APP_TEST_MISSING_KEY_{index}"))
                .find(|key| std::env::var_os(key).is_none())
                .expect("find missing env key")
                .into_boxed_str(),
        );

        assert_eq!(super::env_u32(missing_key).expect("env_u32"), None);
        assert_eq!(super::env_u64(missing_key).expect("env_u64"), None);
        assert_eq!(
            super::env_socket_addr(missing_key).expect("env_socket_addr"),
            None
        );
        assert_eq!(super::env_string(missing_key).expect("env_string"), None);
        assert_eq!(super::env_path(missing_key).expect("env_path"), None);
    }

    #[test]
    fn app_and_storage_error_display_paths_are_stable() {
        let config_error = AppServiceConfigError::Config(ConfigError::InvalidConfig {
            field: "ui.repo_root",
            value: "bad".to_string(),
        });
        assert!(format!("{config_error}").contains("app config error"));
        assert!(config_error.source().is_some());

        let storage_error = AppServiceConfigError::Storage(StorageConfigError::MissingEnv("READ"));
        assert!(format!("{storage_error}").contains("app storage config error"));
        assert!(storage_error.source().is_some());

        let missing_env = AppServiceConfigError::MissingEnv("MISSING");
        assert_eq!(format!("{missing_env}"), "missing env MISSING");
        assert!(missing_env.source().is_none());

        let invalid_env = AppServiceConfigError::InvalidEnv {
            key: "KEY",
            value: "bad".to_string(),
        };
        assert_eq!(format!("{invalid_env}"), "invalid env KEY: bad");
        assert!(invalid_env.source().is_none());

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
                StorageConfigError::InvalidConfig("invalid pool".to_string())
            ),
            "invalid pool"
        );
    }

    #[test]
    fn app_error_display_and_source_cover_variants() {
        let config = AppError::Config(AppServiceConfigError::MissingEnv("MISSING"));
        assert!(format!("{config}").contains("app error"));
        assert!(config.source().is_some());

        let observability_config =
            AppError::ObservabilityConfig(ObservabilityConfigError::InvalidEnv {
                key: "KEY",
                value: "bad".to_string(),
            });
        assert!(format!("{observability_config}").contains("observability config error"));
        assert!(observability_config.source().is_some());

        let observability =
            AppError::Observability(ObservabilityError::MetricsInit("failed".to_string()));
        assert!(format!("{observability}").contains("observability error"));
        assert!(observability.source().is_some());

        let storage = AppError::Storage(StorageError::Internal {
            message: "db".to_string(),
        });
        assert!(format!("{storage}").contains("app storage error"));
        assert!(storage.source().is_some());

        let serve = AppError::Serve("bind".to_string());
        assert_eq!(format!("{serve}"), "app serve error: bind");
        assert!(serve.source().is_none());
    }

    #[test]
    fn app_api_error_maps_all_status_codes() {
        assert_eq!(
            AppApiError::Ui(AppUiError::BadRequest("bad".to_string()))
                .into_response()
                .status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            AppApiError::Ui(AppUiError::NotFound("missing".to_string()))
                .into_response()
                .status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            AppApiError::Ui(AppUiError::Storage("storage".to_string()))
                .into_response()
                .status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
        assert_eq!(
            AppApiError::Ui(AppUiError::Internal("internal".to_string()))
                .into_response()
                .status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }

    #[tokio::test]
    async fn build_repositories_handles_valid_and_invalid_storage() {
        let valid_config = AppServiceConfig {
            bind: "127.0.0.1:8090".parse().expect("bind"),
            base_path: "/".to_string(),
            site_root: PathBuf::from("crates/app-ui/dist"),
            site_pkg_dir: "pkg".to_string(),
            storage: test_storage_config(),
            ui: test_ui_config(),
        };
        let _repos = super::build_repositories(&valid_config).expect("repositories");

        let invalid_config = AppServiceConfig {
            storage: StorageConfig {
                max_connections: 0,
                min_connections: 0,
                ..test_storage_config()
            },
            ..valid_config
        };
        let err = super::build_repositories(&invalid_config).expect_err("invalid pool config");
        assert!(err.to_string().contains("app storage error"));

        let invalid_connection = AppServiceConfig {
            storage: StorageConfig {
                read_connection: "not-a-connection".to_string(),
                ..test_storage_config()
            },
            ..invalid_config
        };
        let err =
            super::build_repositories(&invalid_connection).expect_err("invalid connect options");
        assert!(err.to_string().contains("app storage error"));
    }

    #[tokio::test]
    async fn serve_maps_bind_errors_after_setup() {
        let occupied = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("occupied listener");
        let bind = occupied.local_addr().expect("occupied addr");
        let config = AppServiceConfig {
            bind,
            base_path: "/".to_string(),
            site_root: PathBuf::from("crates/app-ui/dist"),
            site_pkg_dir: "pkg".to_string(),
            storage: test_storage_config(),
            ui: test_ui_config(),
        };

        let err = super::serve_with(config, || Ok(()), run_axum_server)
            .await
            .expect_err("bind error");
        assert!(err.to_string().contains("app serve error"));
    }

    #[tokio::test]
    async fn run_axum_server_can_start_and_be_aborted() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener");
        let addr = listener.local_addr().expect("addr");
        let app = axum::Router::new().route("/health", get(super::health_handler));
        let task = tokio::spawn(run_axum_server(listener, app));
        tokio::time::sleep(Duration::from_millis(25)).await;
        let mut stream = tokio::net::TcpStream::connect(addr)
            .await
            .expect("connect health socket");
        stream
            .write_all(b"GET /health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .await
            .expect("write request");
        let mut response = Vec::new();
        stream
            .read_to_end(&mut response)
            .await
            .expect("read response");
        assert!(response.starts_with(b"HTTP/1.1 200"));
        task.abort();
        let join_err = task.await.expect_err("abort");
        assert!(join_err.is_cancelled());
    }

    #[tokio::test]
    async fn serve_with_maps_server_errors() {
        let config = AppServiceConfig {
            bind: "127.0.0.1:0".parse().expect("bind"),
            base_path: "/".to_string(),
            site_root: PathBuf::from("crates/app-ui/dist"),
            site_pkg_dir: "pkg".to_string(),
            storage: test_storage_config(),
            ui: test_ui_config(),
        };
        let err = super::serve_with(
            config,
            || Ok(()),
            |_listener, _router| async { Err(std::io::Error::other("boom")) },
        )
        .await
        .expect_err("serve error");
        assert!(err.to_string().contains("boom"));
    }

    #[tokio::test]
    async fn serve_with_maps_storage_errors_before_server_start() {
        let config = AppServiceConfig {
            bind: "127.0.0.1:0".parse().expect("bind"),
            base_path: "/".to_string(),
            site_root: PathBuf::from("crates/app-ui/dist"),
            site_pkg_dir: "pkg".to_string(),
            storage: StorageConfig {
                max_connections: 0,
                min_connections: 0,
                ..test_storage_config()
            },
            ui: test_ui_config(),
        };
        let err = super::serve_with(config, || Ok(()), run_axum_server)
            .await
            .expect_err("storage error");
        assert!(err.to_string().contains("app storage error"));
    }

    #[tokio::test]
    async fn serve_with_returns_ok_when_server_finishes_cleanly() {
        let config = AppServiceConfig {
            bind: "127.0.0.1:0".parse().expect("bind"),
            base_path: "/".to_string(),
            site_root: PathBuf::from("crates/app-ui/dist"),
            site_pkg_dir: "pkg".to_string(),
            storage: test_storage_config(),
            ui: test_ui_config(),
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
    async fn serve_wrapper_executes_default_path() {
        let occupied = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("occupied listener");
        let bind = occupied.local_addr().expect("occupied addr");
        let config = AppServiceConfig {
            bind,
            base_path: "/".to_string(),
            site_root: PathBuf::from("crates/app-ui/dist"),
            site_pkg_dir: "pkg".to_string(),
            storage: test_storage_config(),
            ui: test_ui_config(),
        };
        let err = super::serve(config).await.expect_err("wrapper error");
        drop(occupied);
        assert!(
            err.to_string().contains("app serve error")
                || err.to_string().contains("app observability")
        );
    }

    #[test]
    fn init_observability_returns_registry() {
        let first = super::init_observability();
        let first_registry_valid = first
            .as_ref()
            .map(|handle| handle.prometheus_registry().is_some())
            .unwrap_or(true);
        assert!(first_registry_valid);
        let second = super::init_observability();
        let second_error = second.err().map(|err| err.to_string()).unwrap_or_default();
        assert!(second_error.is_empty() || second_error.contains("app observability error"));
    }

    #[tokio::test]
    async fn build_router_nests_routes_for_non_root_base_path() {
        let repositories = Arc::new(InMemoryRepositories::new());
        let profiles: Arc<dyn ProfileRepository> = repositories.clone();
        let repositories: Arc<dyn RepoMappingRepository> = repositories;
        let state = AppUiState::new(
            repositories,
            profiles,
            "/tmp/gittree".into(),
            "http://localhost:8085".to_string(),
            "http://localhost:8089".to_string(),
            "http://localhost:8090".to_string(),
            "http://localhost:8088".to_string(),
            "/ui".to_string(),
            LeptosOptions::builder()
                .output_name("gittree-app-ui")
                .site_root("crates/app-ui/dist")
                .site_pkg_dir("pkg")
                .site_addr("127.0.0.1:0".parse::<std::net::SocketAddr>().expect("addr"))
                .build(),
        );
        let app = build_router(state);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/ui/health")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn build_router_serves_root_base_path_and_server_fn_route() {
        let repositories = Arc::new(InMemoryRepositories::new());
        let profiles: Arc<dyn ProfileRepository> = repositories.clone();
        let repositories: Arc<dyn RepoMappingRepository> = repositories;
        let state = test_state(repositories, profiles);
        let app = build_router(state);

        let health_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(health_response.status(), StatusCode::OK);

        let server_fn_path = leptos::server_fn::axum::server_fn_paths()
            .find(|(path, method)| path.starts_with("/api/") && *method == Method::POST)
            .map(|(path, _)| path.to_string())
            .expect("registered /api server fn path");

        let server_fn_response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(server_fn_path)
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_ne!(server_fn_response.status(), StatusCode::NOT_FOUND);
        assert_ne!(server_fn_response.status(), StatusCode::METHOD_NOT_ALLOWED);
    }

    #[tokio::test]
    async fn server_fn_route_handler_resolves_registered_paths() {
        let repositories = Arc::new(InMemoryRepositories::new());
        let profiles: Arc<dyn ProfileRepository> = repositories.clone();
        let repositories: Arc<dyn RepoMappingRepository> = repositories;
        let state = test_state(repositories, profiles);
        let server_fn_path = leptos::server_fn::axum::server_fn_paths()
            .find(|(path, method)| path.starts_with("/api/") && *method == Method::POST)
            .map(|(path, _)| path.to_string())
            .expect("registered /api server fn path");

        let response = server_fn_route_handler(
            State(state),
            Request::builder()
                .method("POST")
                .uri(server_fn_path)
                .header("content-type", "application/json")
                .body(Body::from("{}"))
                .expect("request"),
        )
        .await;
        assert_ne!(response.status(), StatusCode::NOT_FOUND);
        assert_ne!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
    }

    #[test]
    fn build_leptos_options_uses_config_fields() {
        let config = AppServiceConfig {
            bind: "127.0.0.1:8091".parse().expect("bind"),
            base_path: "/ui".to_string(),
            site_root: PathBuf::from("/tmp/gittree-dist"),
            site_pkg_dir: "pkg-assets".to_string(),
            storage: test_storage_config(),
            ui: test_ui_config(),
        };
        let options = super::build_leptos_options(&config);
        assert_eq!(options.site_addr, config.bind);
        assert_eq!(options.site_pkg_dir.as_ref(), "pkg-assets");
        assert_eq!(options.site_root.as_ref(), "/tmp/gittree-dist");
    }

    #[tokio::test]
    async fn health_endpoint_returns_ok() {
        let repositories = Arc::new(InMemoryRepositories::new());
        let profiles: Arc<dyn ProfileRepository> = repositories.clone();
        let repositories: Arc<dyn RepoMappingRepository> = repositories;
        let state = test_state(repositories, profiles);
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
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn api_list_repos_returns_entries() {
        let repositories = Arc::new(InMemoryRepositories::new());
        let record = RepoMappingRecord {
            forgejo_owner: "owner".to_string(),
            forgejo_repo: "repo".to_string(),
            pubkey: vec![0x11; 32],
            identifier: "repo".to_string(),
        };
        let pubkey_hex = pubkey_hex(0x11);
        repositories
            .upsert_mapping(record.clone())
            .await
            .expect("insert mapping");
        let profile = ProfileRecord::new(
            &pubkey_hex,
            None,
            None,
            None,
            None,
            None,
            ProfileVisibility::Public,
            10,
            10,
        )
        .expect("profile");
        repositories.upsert_profile(profile).await.expect("profile");

        let profiles: Arc<dyn ProfileRepository> = repositories.clone();
        let repositories: Arc<dyn RepoMappingRepository> = repositories;
        let state = test_state(repositories, profiles);
        let app = build_router(state);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/repos")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let parsed: RepoListResponse = serde_json::from_slice(&body).expect("json");
        assert_eq!(parsed.items.len(), 1);
        assert_eq!(parsed.items[0].forgejo, "owner/repo");
    }

    #[tokio::test]
    async fn api_list_repos_by_owner_filters_entries() {
        let repositories = Arc::new(InMemoryRepositories::new());
        let record_a = RepoMappingRecord {
            forgejo_owner: "owner".to_string(),
            forgejo_repo: "repo".to_string(),
            pubkey: vec![0x11; 32],
            identifier: "repo".to_string(),
        };
        let record_b = RepoMappingRecord {
            forgejo_owner: "other".to_string(),
            forgejo_repo: "else".to_string(),
            pubkey: vec![0x22; 32],
            identifier: "else".to_string(),
        };
        let pubkey_hex_a = pubkey_hex(0x11);
        let pubkey_hex_b = pubkey_hex(0x22);
        repositories
            .upsert_mapping(record_a.clone())
            .await
            .expect("insert mapping");
        repositories
            .upsert_mapping(record_b.clone())
            .await
            .expect("insert mapping");
        let profile_a = ProfileRecord::new(
            &pubkey_hex_a,
            None,
            None,
            None,
            None,
            None,
            ProfileVisibility::Public,
            10,
            10,
        )
        .expect("profile");
        let profile_b = ProfileRecord::new(
            &pubkey_hex_b,
            None,
            None,
            None,
            None,
            None,
            ProfileVisibility::Public,
            10,
            10,
        )
        .expect("profile");
        repositories
            .upsert_profile(profile_a)
            .await
            .expect("profile");
        repositories
            .upsert_profile(profile_b)
            .await
            .expect("profile");

        let profiles: Arc<dyn ProfileRepository> = repositories.clone();
        let repositories: Arc<dyn RepoMappingRepository> = repositories;
        let state = test_state(repositories, profiles);
        let app = build_router(state);
        let npub = gittree_app_core::npub_from_bytes(&[0x11; 32]).expect("npub");
        let response = app
            .oneshot(
                Request::builder()
                    .uri(format!("/api/users/{npub}/repos"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let parsed: RepoListResponse = serde_json::from_slice(&body).expect("json");
        assert_eq!(parsed.items.len(), 1);
        assert_eq!(parsed.items[0].forgejo, "owner/repo");
    }

    #[tokio::test]
    async fn api_list_repos_by_owner_rejects_invalid_npub() {
        let repositories = Arc::new(InMemoryRepositories::new());
        let profiles: Arc<dyn ProfileRepository> = repositories.clone();
        let repositories: Arc<dyn RepoMappingRepository> = repositories;
        let state = test_state(repositories, profiles);
        let app = build_router(state);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/users/not-a-valid-npub/repos")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn api_repo_detail_returns_entry() {
        let repositories = Arc::new(InMemoryRepositories::new());
        let record = RepoMappingRecord {
            forgejo_owner: "owner".to_string(),
            forgejo_repo: "repo".to_string(),
            pubkey: vec![0x11; 32],
            identifier: "repo".to_string(),
        };
        let pubkey_hex = pubkey_hex(0x11);
        let npub = gittree_app_core::npub_from_bytes(&record.pubkey).expect("npub");
        repositories
            .upsert_mapping(record.clone())
            .await
            .expect("insert mapping");
        let profile = ProfileRecord::new(
            &pubkey_hex,
            None,
            None,
            None,
            None,
            None,
            ProfileVisibility::Public,
            10,
            10,
        )
        .expect("profile");
        repositories.upsert_profile(profile).await.expect("profile");

        let profiles: Arc<dyn ProfileRepository> = repositories.clone();
        let repositories: Arc<dyn RepoMappingRepository> = repositories;
        let state = test_state(repositories, profiles);
        let app = build_router(state);
        let response = app
            .oneshot(
                Request::builder()
                    .uri(format!("/api/repos/{npub}/repo"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let parsed: gittree_app_core::RepoDetail = serde_json::from_slice(&body).expect("json");
        assert_eq!(parsed.identifier, "repo");
        assert_eq!(parsed.forgejo, "owner/repo");
    }

    #[tokio::test]
    async fn api_repo_detail_rejects_invalid_npub() {
        let repositories = Arc::new(InMemoryRepositories::new());
        let profiles: Arc<dyn ProfileRepository> = repositories.clone();
        let repositories: Arc<dyn RepoMappingRepository> = repositories;
        let state = test_state(repositories, profiles);
        let app = build_router(state);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/repos/not-a-valid-npub/repo")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn api_repo_detail_returns_not_found_for_missing_identifier() {
        let repositories = Arc::new(InMemoryRepositories::new());
        let record = RepoMappingRecord {
            forgejo_owner: "owner".to_string(),
            forgejo_repo: "repo".to_string(),
            pubkey: vec![0x11; 32],
            identifier: "repo".to_string(),
        };
        let pubkey_hex = pubkey_hex(0x11);
        let npub = gittree_app_core::npub_from_bytes(&record.pubkey).expect("npub");
        repositories
            .upsert_mapping(record.clone())
            .await
            .expect("insert mapping");
        let profile = ProfileRecord::new(
            &pubkey_hex,
            None,
            None,
            None,
            None,
            None,
            ProfileVisibility::Public,
            10,
            10,
        )
        .expect("profile");
        repositories.upsert_profile(profile).await.expect("profile");

        let profiles: Arc<dyn ProfileRepository> = repositories.clone();
        let repositories: Arc<dyn RepoMappingRepository> = repositories;
        let state = test_state(repositories, profiles);
        let app = build_router(state);
        let response = app
            .oneshot(
                Request::builder()
                    .uri(format!("/api/repos/{npub}/missing"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn api_list_repos_hides_private_profiles() {
        let repositories = Arc::new(InMemoryRepositories::new());
        let public_record = RepoMappingRecord {
            forgejo_owner: "owner".to_string(),
            forgejo_repo: "repo".to_string(),
            pubkey: vec![0x11; 32],
            identifier: "repo".to_string(),
        };
        let private_record = RepoMappingRecord {
            forgejo_owner: "owner".to_string(),
            forgejo_repo: "secret".to_string(),
            pubkey: vec![0x22; 32],
            identifier: "secret".to_string(),
        };
        let public_pubkey_hex = pubkey_hex(0x11);
        let private_pubkey_hex = pubkey_hex(0x22);
        repositories
            .upsert_mapping(public_record.clone())
            .await
            .expect("insert mapping");
        repositories
            .upsert_mapping(private_record.clone())
            .await
            .expect("insert mapping");
        let profile_public = ProfileRecord::new(
            &public_pubkey_hex,
            None,
            None,
            None,
            None,
            None,
            ProfileVisibility::Public,
            10,
            10,
        )
        .expect("profile");
        let profile_private = ProfileRecord::new(
            &private_pubkey_hex,
            None,
            None,
            None,
            None,
            None,
            ProfileVisibility::Private,
            10,
            10,
        )
        .expect("profile");
        repositories
            .upsert_profile(profile_public)
            .await
            .expect("profile");
        repositories
            .upsert_profile(profile_private)
            .await
            .expect("profile");

        let profiles: Arc<dyn ProfileRepository> = repositories.clone();
        let repositories: Arc<dyn RepoMappingRepository> = repositories;
        let state = test_state(repositories, profiles);
        let app = build_router(state);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/repos")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let parsed: RepoListResponse = serde_json::from_slice(&body).expect("json");
        assert_eq!(parsed.items.len(), 1);
        assert_eq!(parsed.items[0].forgejo, "owner/repo");
    }

    #[tokio::test]
    async fn api_repo_detail_hides_private_profile() {
        let repositories = Arc::new(InMemoryRepositories::new());
        let record = RepoMappingRecord {
            forgejo_owner: "owner".to_string(),
            forgejo_repo: "secret".to_string(),
            pubkey: vec![0x22; 32],
            identifier: "secret".to_string(),
        };
        let pubkey_hex = pubkey_hex(0x22);
        let npub = gittree_app_core::npub_from_bytes(&record.pubkey).expect("npub");
        repositories
            .upsert_mapping(record.clone())
            .await
            .expect("insert mapping");
        let profile = ProfileRecord::new(
            &pubkey_hex,
            None,
            None,
            None,
            None,
            None,
            ProfileVisibility::Private,
            10,
            10,
        )
        .expect("profile");
        repositories.upsert_profile(profile).await.expect("profile");

        let profiles: Arc<dyn ProfileRepository> = repositories.clone();
        let repositories: Arc<dyn RepoMappingRepository> = repositories;
        let state = test_state(repositories, profiles);
        let app = build_router(state);
        let response = app
            .oneshot(
                Request::builder()
                    .uri(format!("/api/repos/{npub}/secret"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn api_handlers_execute_direct_invocations() {
        let repositories = Arc::new(InMemoryRepositories::new());
        let record = RepoMappingRecord {
            forgejo_owner: "owner".to_string(),
            forgejo_repo: "repo".to_string(),
            pubkey: vec![0x33; 32],
            identifier: "repo".to_string(),
        };
        let pubkey_hex = pubkey_hex(0x33);
        let npub = gittree_app_core::npub_from_bytes(&record.pubkey).expect("npub");
        repositories
            .upsert_mapping(record)
            .await
            .expect("insert mapping");
        let profile = ProfileRecord::new(
            &pubkey_hex,
            None,
            None,
            None,
            None,
            None,
            ProfileVisibility::Public,
            10,
            10,
        )
        .expect("profile");
        repositories.upsert_profile(profile).await.expect("profile");

        let profiles: Arc<dyn ProfileRepository> = repositories.clone();
        let mappings: Arc<dyn RepoMappingRepository> = repositories;
        let state = test_state(mappings, profiles);

        let listed = super::api_list_repos_handler(State(state.clone()))
            .await
            .expect("list repos");
        assert_eq!(listed.0.items.len(), 1);

        let listed_for_owner =
            super::api_list_repos_by_owner_handler(State(state.clone()), Path(npub.clone()))
                .await
                .expect("list owner repos");
        assert_eq!(listed_for_owner.0.items.len(), 1);

        let detail =
            super::api_repo_detail_handler(State(state), Path((npub, "repo".to_string())))
                .await
                .expect("repo detail");
        assert_eq!(detail.0.identifier, "repo");
    }

    #[tokio::test]
    async fn api_list_repos_handler_maps_internal_errors() {
        let profiles: Arc<dyn ProfileRepository> = Arc::new(InMemoryRepositories::new());
        let mappings_impl = Arc::new(FailingListMappings::default());
        mappings_impl
            .upsert_mapping(RepoMappingRecord {
                forgejo_owner: "owner".to_string(),
                forgejo_repo: "repo".to_string(),
                pubkey: vec![0x44; 32],
                identifier: "repo".to_string(),
            })
            .await
            .expect("noop upsert");
        assert!(
            mappings_impl
                .mapping_by_forgejo("owner", "repo")
                .await
                .expect("mapping by forgejo")
                .is_none()
        );
        assert!(
            mappings_impl
                .mapping_by_repo(&[0x44; 32], "repo")
                .await
                .expect("mapping by repo")
                .is_none()
        );
        let mappings: Arc<dyn RepoMappingRepository> = mappings_impl;
        let state = test_state(mappings, profiles);

        let err = super::api_list_repos_handler(State(state))
            .await
            .expect_err("storage error");
        assert_eq!(err.into_response().status(), StatusCode::INTERNAL_SERVER_ERROR);
    }
}
