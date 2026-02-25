use async_trait::async_trait;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use gittree_config::{ConfigError, ServicesConfig};
use gittree_git_hook::{
    PostReceivePayload, RefUpdatePayload, parse_forgejo_push, verify_forgejo_signature,
};
use gittree_observability::{ObservabilityConfigError, ObservabilityError, ObservabilityHandle};
use gittree_storage::{PostgresRepositories, RepoMappingRepository, StorageConfig, StorageError};
use serde::{Deserialize, Serialize};
use std::future::{Future, pending};
use std::sync::Arc;

const ENV_STORAGE_READ_URL: &str = "GITTREE_STORAGE_READ_URL";
const ENV_STORAGE_WRITE_URL: &str = "GITTREE_STORAGE_WRITE_URL";
const ENV_STORAGE_MAX_CONNECTIONS: &str = "GITTREE_STORAGE_MAX_CONNECTIONS";
const ENV_STORAGE_MIN_CONNECTIONS: &str = "GITTREE_STORAGE_MIN_CONNECTIONS";
const ENV_STORAGE_IDLE_TIMEOUT_SECS: &str = "GITTREE_STORAGE_IDLE_TIMEOUT_SECS";
const ENV_STORAGE_MAX_LIFETIME_SECS: &str = "GITTREE_STORAGE_MAX_LIFETIME_SECS";
const ENV_STORAGE_APP_NAME: &str = "GITTREE_STORAGE_APP_NAME";
const ENV_SYNC_URL: &str = "GITTREE_SYNC_URL";
const ENV_FORGEJO_WEBHOOK_SECRET: &str = "GITTREE_FORGEJO_WEBHOOK_SECRET";

const SIGNATURE_HEADERS: [&str; 3] = [
    "x-gitea-signature",
    "x-forgejo-signature",
    "x-hub-signature-256",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebhookConfig {
    pub bind: String,
    pub storage: StorageConfig,
    pub sync_url: String,
    pub forgejo_secret: String,
}

impl WebhookConfig {
    pub fn from_env() -> Result<Self, WebhookConfigError> {
        let services = ServicesConfig::from_env_validated().map_err(WebhookConfigError::Config)?;
        let storage = storage_from_env()?;
        let sync_url = env_required_string(ENV_SYNC_URL)?;
        let forgejo_secret = env_required_string(ENV_FORGEJO_WEBHOOK_SECRET)?;
        Ok(Self {
            bind: services.webhook.bind,
            storage,
            sync_url,
            forgejo_secret,
        })
    }
}

#[derive(Debug)]
pub enum WebhookConfigError {
    Config(ConfigError),
    Storage(StorageConfigError),
    MissingEnv(&'static str),
    InvalidEnv { key: &'static str, value: String },
}

impl std::fmt::Display for WebhookConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WebhookConfigError::Config(err) => write!(f, "webhook config error: {err}"),
            WebhookConfigError::Storage(err) => write!(f, "webhook storage config error: {err}"),
            WebhookConfigError::MissingEnv(key) => write!(f, "missing env {key}"),
            WebhookConfigError::InvalidEnv { key, value } => {
                write!(f, "invalid env {key}: {value}")
            }
        }
    }
}

impl std::error::Error for WebhookConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            WebhookConfigError::Config(err) => Some(err),
            WebhookConfigError::Storage(err) => Some(err),
            WebhookConfigError::MissingEnv(_) => None,
            WebhookConfigError::InvalidEnv { .. } => None,
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

fn storage_from_env() -> Result<StorageConfig, WebhookConfigError> {
    let read_connection = std::env::var(ENV_STORAGE_READ_URL).map_err(|_| {
        WebhookConfigError::Storage(StorageConfigError::MissingEnv(ENV_STORAGE_READ_URL))
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
        WebhookConfigError::Storage(StorageConfigError::InvalidConfig(err.to_string()))
    })?;

    Ok(config)
}

fn env_u32(key: &'static str) -> Result<Option<u32>, WebhookConfigError> {
    match std::env::var(key) {
        Ok(value) => {
            if value.trim().is_empty() {
                return Ok(None);
            }
            value.parse::<u32>().map(Some).map_err(|_| {
                WebhookConfigError::Storage(StorageConfigError::InvalidEnv { key, value })
            })
        }
        Err(_) => Ok(None),
    }
}

fn env_u64(key: &'static str) -> Result<Option<u64>, WebhookConfigError> {
    match std::env::var(key) {
        Ok(value) => {
            if value.trim().is_empty() {
                return Ok(None);
            }
            value.parse::<u64>().map(Some).map_err(|_| {
                WebhookConfigError::Storage(StorageConfigError::InvalidEnv { key, value })
            })
        }
        Err(_) => Ok(None),
    }
}

fn env_required_string(key: &'static str) -> Result<String, WebhookConfigError> {
    let value = std::env::var(key).map_err(|_| WebhookConfigError::MissingEnv(key))?;
    if value.trim().is_empty() {
        return Err(WebhookConfigError::InvalidEnv { key, value });
    }
    Ok(value)
}

#[derive(Debug)]
pub enum WebhookError {
    Config(WebhookConfigError),
    ObservabilityConfig(ObservabilityConfigError),
    Observability(ObservabilityError),
    Storage(StorageError),
    Notify(String),
    Serve(String),
}

impl std::fmt::Display for WebhookError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WebhookError::Config(err) => write!(f, "webhook error: {err}"),
            WebhookError::ObservabilityConfig(err) => {
                write!(f, "webhook observability config error: {err}")
            }
            WebhookError::Observability(err) => {
                write!(f, "webhook observability error: {err}")
            }
            WebhookError::Storage(err) => write!(f, "webhook storage error: {err}"),
            WebhookError::Notify(err) => write!(f, "webhook notify error: {err}"),
            WebhookError::Serve(err) => write!(f, "webhook serve error: {err}"),
        }
    }
}

impl std::error::Error for WebhookError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            WebhookError::Config(err) => Some(err),
            WebhookError::ObservabilityConfig(err) => Some(err),
            WebhookError::Observability(err) => Some(err),
            WebhookError::Storage(err) => Some(err),
            WebhookError::Notify(_) => None,
            WebhookError::Serve(_) => None,
        }
    }
}

pub fn init_observability() -> Result<ObservabilityHandle, WebhookError> {
    let config = gittree_observability::ObservabilityConfig::from_env("gittree-webhook")
        .map_err(WebhookError::ObservabilityConfig)?;
    let handle = gittree_observability::init(&config).map_err(WebhookError::Observability)?;
    Ok(handle)
}

pub fn build_repositories(config: &WebhookConfig) -> Result<PostgresRepositories, WebhookError> {
    let pool_options = config
        .storage
        .pool_options()
        .map_err(WebhookError::Storage)?;
    let connect_options = config
        .storage
        .read_connect_options()
        .map_err(WebhookError::Storage)?;
    let pool = pool_options.connect_lazy_with(connect_options);
    Ok(PostgresRepositories::new(pool))
}

#[async_trait]
pub trait SyncNotifier: Send + Sync {
    async fn notify(&self, payload: PostReceivePayload) -> Result<(), String>;
}

#[derive(Clone)]
pub struct HttpSyncNotifier {
    endpoint: String,
    client: reqwest::Client,
}

impl HttpSyncNotifier {
    pub fn new(endpoint: impl Into<String>) -> Result<Self, String> {
        Self::from_builder_result(endpoint.into(), reqwest::Client::builder().build())
    }

    fn from_builder_result<E: ToString>(
        endpoint: String,
        result: Result<reqwest::Client, E>,
    ) -> Result<Self, String> {
        let client = match result {
            Ok(client) => client,
            Err(err) => return Err(err.to_string()),
        };
        Ok(Self {
            endpoint,
            client,
        })
    }

    #[cfg(test)]
    fn new_with_result<E: ToString>(
        endpoint: impl Into<String>,
        result: Result<reqwest::Client, E>,
    ) -> Result<Self, String> {
        Self::from_builder_result(endpoint.into(), result)
    }
}

#[async_trait]
impl SyncNotifier for HttpSyncNotifier {
    async fn notify(&self, payload: PostReceivePayload) -> Result<(), String> {
        let response = self
            .client
            .post(&self.endpoint)
            .json(&payload)
            .send()
            .await
            .map_err(|err| err.to_string())?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(format!("sync error {status}: {body}"));
        }
        Ok(())
    }
}

struct WebhookAppState<R, N> {
    repositories: Arc<R>,
    notifier: N,
    forgejo_secret: String,
}

impl<R, N> Clone for WebhookAppState<R, N>
where
    N: Clone,
{
    fn clone(&self) -> Self {
        Self {
            repositories: Arc::clone(&self.repositories),
            notifier: self.notifier.clone(),
            forgejo_secret: self.forgejo_secret.clone(),
        }
    }
}

pub async fn serve(config: WebhookConfig) -> Result<(), WebhookError> {
    serve_with_init(config, init_observability).await
}

async fn serve_with_init<I, O>(config: WebhookConfig, init: I) -> Result<(), WebhookError>
where
    I: FnOnce() -> Result<O, WebhookError>,
{
    let _observability = init()?;
    let repositories = build_repositories(&config)?;
    let notifier = HttpSyncNotifier::new(config.sync_url.clone()).map_err(WebhookError::Notify)?;
    let state = WebhookAppState {
        repositories: Arc::new(repositories),
        notifier,
        forgejo_secret: config.forgejo_secret,
    };
    let router = build_router(state);
    let listener = tokio::net::TcpListener::bind(&config.bind)
        .await
        .map_err(|err| WebhookError::Serve(err.to_string()))?;
    run_http_server_with_shutdown(listener, router, pending()).await
}

async fn run_http_server_with_shutdown<Shutdown>(
    listener: tokio::net::TcpListener,
    router: Router,
    shutdown: Shutdown,
) -> Result<(), WebhookError>
where
    Shutdown: Future<Output = ()> + Send + 'static,
{
    let result = axum::serve(listener, router)
        .with_graceful_shutdown(shutdown)
        .await;
    map_serve_result(result)
}

fn map_serve_result(result: std::io::Result<()>) -> Result<(), WebhookError> {
    match result {
        Ok(()) => Ok(()),
        Err(err) => Err(WebhookError::Serve(err.to_string())),
    }
}

fn build_router<R, N>(state: WebhookAppState<R, N>) -> Router
where
    R: RepoMappingRepository + Send + Sync + 'static,
    N: SyncNotifier + Clone + Send + Sync + 'static,
{
    Router::new()
        .route("/health", get(health_handler))
        .route("/", post(forgejo_handler))
        .with_state(Arc::new(state))
}

async fn health_handler() -> &'static str {
    "ok"
}

#[derive(Debug, Deserialize, Serialize)]
pub struct WebhookAckPayload {
    pub repo: String,
    pub updates: usize,
}

#[derive(Debug)]
enum WebhookHttpError {
    BadRequest(String),
    Unauthorized(String),
    NotFound(String),
    Upstream(String),
    Internal(String),
}

impl IntoResponse for WebhookHttpError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            WebhookHttpError::BadRequest(message) => (StatusCode::BAD_REQUEST, message),
            WebhookHttpError::Unauthorized(message) => (StatusCode::UNAUTHORIZED, message),
            WebhookHttpError::NotFound(message) => (StatusCode::NOT_FOUND, message),
            WebhookHttpError::Upstream(message) => (StatusCode::BAD_GATEWAY, message),
            WebhookHttpError::Internal(message) => (StatusCode::INTERNAL_SERVER_ERROR, message),
        };
        (status, message).into_response()
    }
}

async fn forgejo_handler<R, N>(
    State(state): State<Arc<WebhookAppState<R, N>>>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<WebhookAckPayload>, WebhookHttpError>
where
    R: RepoMappingRepository + Send + Sync,
    N: SyncNotifier + Send + Sync,
{
    let signature = extract_signature(&headers)?;
    verify_forgejo_signature(&state.forgejo_secret, &body, signature)
        .map_err(|err| WebhookHttpError::Unauthorized(err.to_string()))?;
    let payload = std::str::from_utf8(&body)
        .map_err(|_| WebhookHttpError::BadRequest("invalid payload".to_string()))?;
    let event =
        parse_forgejo_push(payload).map_err(|err| WebhookHttpError::BadRequest(err.to_string()))?;
    let mapping = state
        .repositories
        .mapping_by_forgejo(&event.owner, &event.repo)
        .await
        .map_err(|err| WebhookHttpError::Internal(err.to_string()))?
        .ok_or_else(|| WebhookHttpError::NotFound("missing repo mapping".to_string()))?;

    let payload = PostReceivePayload {
        pubkey: hex::encode(&mapping.pubkey),
        identifier: mapping.identifier.clone(),
        updates: vec![RefUpdatePayload {
            old: event.before,
            new: event.after,
            reference: event.reference,
        }],
    };

    state
        .notifier
        .notify(payload)
        .await
        .map_err(WebhookHttpError::Upstream)?;

    Ok(Json(WebhookAckPayload {
        repo: mapping.forgejo_full_name(),
        updates: 1,
    }))
}

fn extract_signature(headers: &HeaderMap) -> Result<&str, WebhookHttpError> {
    for name in SIGNATURE_HEADERS {
        if let Some(value) = headers.get(name) {
            return value
                .to_str()
                .map_err(|_| WebhookHttpError::Unauthorized("invalid signature".to_string()));
        }
    }
    Err(WebhookHttpError::Unauthorized(
        "missing signature".to_string(),
    ))
}

#[cfg(test)]
mod tests {
    use super::HttpSyncNotifier;
    use super::ObservabilityHandle;
    use super::StorageConfigError;
    use super::SyncNotifier;
    use super::WebhookAckPayload;
    use super::WebhookAppState;
    use super::WebhookConfig;
    use super::WebhookConfigError;
    use super::WebhookError;
    use super::build_router;
    use async_trait::async_trait;
    use axum::Router;
    use axum::body::Body;
    use axum::http::{HeaderMap, HeaderValue, Request, StatusCode};
    use axum::response::IntoResponse;
    use gittree_config::ConfigError;
    use gittree_core::RepoMapping;
    use gittree_observability::{ObservabilityConfigError, ObservabilityError};
    use gittree_storage::{
        InMemoryRepositories, RepoMappingRecord, RepoMappingRepository, StorageConfig, StorageError,
    };
    use hmac::Mac;
    use std::error::Error;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex, OnceLock};
    use tower::ServiceExt;

    static ENV_LOCK: Mutex<()> = Mutex::new(());
    static OBSERVABILITY: OnceLock<ObservabilityHandle> = OnceLock::new();

    #[derive(Clone, Default)]
    struct MockNotifier {
        payloads: Arc<Mutex<Vec<gittree_git_hook::PostReceivePayload>>>,
    }

    impl MockNotifier {
        fn payloads(&self) -> Vec<gittree_git_hook::PostReceivePayload> {
            self.payloads.lock().expect("payloads").clone()
        }
    }

    #[async_trait]
    impl SyncNotifier for MockNotifier {
        async fn notify(
            &self,
            payload: gittree_git_hook::PostReceivePayload,
        ) -> Result<(), String> {
            self.payloads.lock().expect("payloads").push(payload);
            Ok(())
        }
    }

    #[derive(Clone, Default)]
    struct FailingNotifier;

    #[async_trait]
    impl SyncNotifier for FailingNotifier {
        async fn notify(
            &self,
            _payload: gittree_git_hook::PostReceivePayload,
        ) -> Result<(), String> {
            Err("upstream down".to_string())
        }
    }

    #[derive(Clone, Default)]
    struct ErrorRepoMappingRepository;

    #[async_trait]
    impl RepoMappingRepository for ErrorRepoMappingRepository {
        async fn upsert_mapping(&self, _record: RepoMappingRecord) -> Result<(), StorageError> {
            Ok(())
        }

        async fn mapping_by_forgejo(
            &self,
            _owner: &str,
            _repo: &str,
        ) -> Result<Option<RepoMappingRecord>, StorageError> {
            Err(StorageError::Internal {
                message: "mapping lookup failed".to_string(),
            })
        }

        async fn mapping_by_repo(
            &self,
            _pubkey: &[u8],
            _identifier: &str,
        ) -> Result<Option<RepoMappingRecord>, StorageError> {
            Ok(None)
        }

        async fn list_mappings(&self) -> Result<Vec<RepoMappingRecord>, StorageError> {
            Ok(Vec::new())
        }
    }

    fn forgejo_push_payload() -> &'static str {
        r#"
        {
            "ref": "refs/heads/main",
            "before": "0000000000000000000000000000000000000000",
            "after": "1111111111111111111111111111111111111111",
            "repository": {
                "name": "repo",
                "full_name": "owner/repo",
                "owner": { "username": "owner" }
            }
        }"#
    }

    fn sign_payload(secret: &[u8], payload: &[u8]) -> String {
        let mut mac = hmac::Hmac::<sha2::Sha256>::new_from_slice(secret).expect("mac");
        mac.update(payload);
        hex::encode(mac.finalize().into_bytes())
    }

    fn signed_request(payload: &[u8], signature_header: &str, signature: &str) -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/json")
            .header(signature_header, format!("sha256={signature}"))
            .body(Body::from(payload.to_vec()))
            .expect("request")
    }

    fn start_mock_http_server(
        status: &str,
        content_type: &str,
        body: &str,
    ) -> (String, std::thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test server");
        let addr = listener.local_addr().expect("local addr");
        let status = status.to_string();
        let content_type = content_type.to_string();
        let body = body.to_string();
        let handle = std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut request = [0u8; 1024];
                let _ = stream.read(&mut request);
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

    fn sample_sync_payload() -> gittree_git_hook::PostReceivePayload {
        gittree_git_hook::PostReceivePayload {
            pubkey: "11".repeat(32),
            identifier: "repo".to_string(),
            updates: vec![gittree_git_hook::RefUpdatePayload {
                old: "0".repeat(40),
                new: "1".repeat(40),
                reference: "refs/heads/main".to_string(),
            }],
        }
    }

    fn with_env_var<F: FnOnce()>(key: &str, value: &str, f: F) {
        let previous = std::env::var_os(key);
        unsafe {
            std::env::set_var(key, value);
        }
        f();
        restore_env_var(key, previous);
    }

    fn restore_env_var(key: &str, previous: Option<std::ffi::OsString>) {
        match previous {
            Some(old) => unsafe {
                std::env::set_var(key, old);
            },
            None => unsafe {
                std::env::remove_var(key);
            },
        }
    }

    fn with_removed_env_var<F: FnOnce()>(key: &str, f: F) {
        let previous = std::env::var_os(key);
        // SAFETY: tests in this module use ENV_LOCK when mutating process env values.
        unsafe {
            std::env::remove_var(key);
        }
        f();
        if let Some(old) = previous {
            // SAFETY: tests in this module use ENV_LOCK when mutating process env values.
            unsafe {
                std::env::set_var(key, old);
            }
        }
    }

    #[test]
    fn config_loads_from_env() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var(
            "GITTREE_STORAGE_READ_URL",
            "postgres://user:pass@localhost:5432/gittree",
            || {
                with_env_var("GITTREE_SYNC_URL", "http://localhost:8084", || {
                    with_env_var("GITTREE_FORGEJO_WEBHOOK_SECRET", "secret", || {
                        with_env_var("GITTREE_WEBHOOK_BIND", "127.0.0.1:9099", || {
                            let config = WebhookConfig::from_env().expect("config");
                            assert_eq!(config.bind, "127.0.0.1:9099");
                            assert_eq!(config.sync_url, "http://localhost:8084");
                            assert_eq!(config.forgejo_secret, "secret");
                        });
                    });
                });
            },
        );
    }

    #[test]
    fn config_reports_missing_and_invalid_required_env_values() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_removed_env_var("GITTREE_STORAGE_READ_URL", || {
            let err = WebhookConfig::from_env().expect_err("missing read url");
            assert!(matches!(
                err,
                WebhookConfigError::Storage(StorageConfigError::MissingEnv(
                    "GITTREE_STORAGE_READ_URL"
                ))
            ));
        });

        with_env_var(
            "GITTREE_STORAGE_READ_URL",
            "postgres://user:pass@localhost:5432/gittree",
            || {
                with_env_var("GITTREE_SYNC_URL", "   ", || {
                    with_env_var("GITTREE_FORGEJO_WEBHOOK_SECRET", "secret", || {
                        let err = WebhookConfig::from_env().expect_err("invalid sync url");
                        assert!(matches!(
                            err,
                            WebhookConfigError::InvalidEnv {
                                key: "GITTREE_SYNC_URL",
                                ..
                            }
                        ));
                    });
                });
            },
        );
    }

    #[test]
    fn config_reports_invalid_storage_numeric_and_bounds() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var(
            "GITTREE_STORAGE_READ_URL",
            "postgres://user:pass@localhost:5432/gittree",
            || {
                with_env_var("GITTREE_SYNC_URL", "http://localhost:8084", || {
                    with_env_var("GITTREE_FORGEJO_WEBHOOK_SECRET", "secret", || {
                        with_env_var("GITTREE_STORAGE_MAX_CONNECTIONS", "nope", || {
                            let err =
                                WebhookConfig::from_env().expect_err("invalid max connections");
                            assert!(matches!(
                                err,
                                WebhookConfigError::Storage(StorageConfigError::InvalidEnv {
                                    key: "GITTREE_STORAGE_MAX_CONNECTIONS",
                                    ..
                                })
                            ));
                        });
                        with_env_var("GITTREE_STORAGE_MAX_CONNECTIONS", "1", || {
                            with_env_var("GITTREE_STORAGE_MIN_CONNECTIONS", "2", || {
                                let err =
                                    WebhookConfig::from_env().expect_err("invalid pool bounds");
                                assert!(matches!(
                                    err,
                                    WebhookConfigError::Storage(StorageConfigError::InvalidConfig(
                                        _
                                    ))
                                ));
                            });
                        });
                        with_env_var("GITTREE_STORAGE_IDLE_TIMEOUT_SECS", "invalid", || {
                            let err = WebhookConfig::from_env().expect_err("invalid idle timeout");
                            assert!(matches!(
                                err,
                                WebhookConfigError::Storage(StorageConfigError::InvalidEnv {
                                    key: "GITTREE_STORAGE_IDLE_TIMEOUT_SECS",
                                    ..
                                })
                            ));
                        });
                    });
                });
            },
        );
    }

    #[test]
    fn config_ignores_empty_storage_timeout_values() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var(
            "GITTREE_STORAGE_READ_URL",
            "postgres://user:pass@localhost:5432/gittree",
            || {
                with_env_var("GITTREE_SYNC_URL", "http://localhost:8084", || {
                    with_env_var("GITTREE_FORGEJO_WEBHOOK_SECRET", "secret", || {
                        with_env_var("GITTREE_STORAGE_IDLE_TIMEOUT_SECS", "   ", || {
                            with_env_var("GITTREE_STORAGE_MAX_LIFETIME_SECS", "", || {
                                let config = WebhookConfig::from_env().expect("config");
                                assert_eq!(config.storage.idle_timeout_secs, None);
                                assert_eq!(config.storage.max_lifetime_secs, None);
                            });
                        });
                    });
                });
            },
        );
    }

    #[test]
    fn config_ignores_empty_storage_pool_values() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var(
            "GITTREE_STORAGE_READ_URL",
            "postgres://user:pass@localhost:5432/gittree",
            || {
                with_env_var("GITTREE_SYNC_URL", "http://localhost:8084", || {
                    with_env_var("GITTREE_FORGEJO_WEBHOOK_SECRET", "secret", || {
                        with_env_var("GITTREE_STORAGE_MAX_CONNECTIONS", " ", || {
                            with_env_var("GITTREE_STORAGE_MIN_CONNECTIONS", "", || {
                                let config = WebhookConfig::from_env().expect("config");
                                assert_eq!(config.storage.max_connections, 10);
                                assert_eq!(config.storage.min_connections, 2);
                            });
                        });
                    });
                });
            },
        );
    }

    #[test]
    fn env_numeric_parsers_return_none_when_vars_are_unset() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        // SAFETY: tests in this module use ENV_LOCK when mutating process env values.
        unsafe {
            std::env::remove_var("GITTREE_STORAGE_MAX_CONNECTIONS");
            std::env::remove_var("GITTREE_STORAGE_IDLE_TIMEOUT_SECS");
        }
        assert_eq!(
            super::env_u32("GITTREE_STORAGE_MAX_CONNECTIONS").expect("u32"),
            None
        );
        assert_eq!(
            super::env_u64("GITTREE_STORAGE_IDLE_TIMEOUT_SECS").expect("u64"),
            None
        );
    }

    #[test]
    fn env_required_string_rejects_missing_and_empty_values() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_removed_env_var("GITTREE_WEBHOOK_TEST_REQUIRED", || {
            let err = super::env_required_string("GITTREE_WEBHOOK_TEST_REQUIRED")
                .expect_err("missing env");
            assert!(matches!(
                err,
                WebhookConfigError::MissingEnv("GITTREE_WEBHOOK_TEST_REQUIRED")
            ));
        });

        with_env_var("GITTREE_WEBHOOK_TEST_REQUIRED", "   ", || {
            let err = super::env_required_string("GITTREE_WEBHOOK_TEST_REQUIRED")
                .expect_err("invalid env");
            assert!(matches!(
                err,
                WebhookConfigError::InvalidEnv {
                    key: "GITTREE_WEBHOOK_TEST_REQUIRED",
                    ..
                }
            ));
        });
    }

    #[test]
    fn with_env_var_restores_existing_value() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        // SAFETY: tests in this module use ENV_LOCK when mutating process env values.
        unsafe {
            std::env::set_var("GITTREE_WEBHOOK_TEST_RESTORE", "before");
        }
        with_env_var("GITTREE_WEBHOOK_TEST_RESTORE", "during", || {
            assert_eq!(
                std::env::var("GITTREE_WEBHOOK_TEST_RESTORE").expect("during value"),
                "during"
            );
        });
        assert_eq!(
            std::env::var("GITTREE_WEBHOOK_TEST_RESTORE").expect("restored value"),
            "before"
        );
        // SAFETY: tests in this module use ENV_LOCK when mutating process env values.
        unsafe {
            std::env::remove_var("GITTREE_WEBHOOK_TEST_RESTORE");
        }
    }

    #[test]
    fn with_removed_env_var_restores_existing_value() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        // SAFETY: tests in this module use ENV_LOCK when mutating process env values.
        unsafe {
            std::env::set_var("GITTREE_WEBHOOK_TEST_REMOVED", "before");
        }
        with_removed_env_var("GITTREE_WEBHOOK_TEST_REMOVED", || {
            assert!(std::env::var("GITTREE_WEBHOOK_TEST_REMOVED").is_err());
        });
        assert_eq!(
            std::env::var("GITTREE_WEBHOOK_TEST_REMOVED").expect("restored value"),
            "before"
        );
        // SAFETY: tests in this module use ENV_LOCK when mutating process env values.
        unsafe {
            std::env::remove_var("GITTREE_WEBHOOK_TEST_REMOVED");
        }
    }

    #[test]
    fn env_helpers_restore_unset_values_after_scope() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        // SAFETY: tests in this module use ENV_LOCK when mutating process env values.
        unsafe {
            std::env::remove_var("GITTREE_WEBHOOK_TEST_RESTORE_MISSING");
            std::env::remove_var("GITTREE_WEBHOOK_TEST_REMOVED_MISSING");
        }
        with_env_var("GITTREE_WEBHOOK_TEST_RESTORE_MISSING", "during", || {
            assert_eq!(
                std::env::var("GITTREE_WEBHOOK_TEST_RESTORE_MISSING").expect("transient value"),
                "during"
            );
        });
        assert!(std::env::var("GITTREE_WEBHOOK_TEST_RESTORE_MISSING").is_err());

        with_removed_env_var("GITTREE_WEBHOOK_TEST_REMOVED_MISSING", || {
            assert!(std::env::var("GITTREE_WEBHOOK_TEST_REMOVED_MISSING").is_err());
        });
        assert!(std::env::var("GITTREE_WEBHOOK_TEST_REMOVED_MISSING").is_err());
    }

    #[test]
    fn init_observability_maps_config_error() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var("GITTREE_METRICS_ENABLED", "invalid-bool", || {
            let err = super::init_observability().expect_err("invalid observability env");
            assert!(matches!(err, WebhookError::ObservabilityConfig(_)));
        });
    }

    #[test]
    fn init_observability_succeeds_once() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var("GITTREE_LOG_STDOUT", "false", || {
            with_env_var("GITTREE_METRICS_ENABLED", "false", || {
                let _ = OBSERVABILITY.get_or_init(|| {
                    super::init_observability().expect("observability should initialize once")
                });
            });
        });
    }

    #[test]
    fn serve_maps_observability_config_error() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        with_env_var("GITTREE_METRICS_ENABLED", "invalid-bool", || {
            let config = WebhookConfig {
                bind: "127.0.0.1:0".to_string(),
                storage: StorageConfig {
                    read_connection: "postgres://user:pass@localhost:5432/gittree".to_string(),
                    write_connection: None,
                    max_connections: 10,
                    min_connections: 1,
                    idle_timeout_secs: None,
                    max_lifetime_secs: None,
                    application_name: None,
                },
                sync_url: "http://localhost:8084".to_string(),
                forgejo_secret: "secret".to_string(),
            };
            let err = runtime
                .block_on(super::serve(config))
                .expect_err("observability config error");
            assert!(matches!(err, WebhookError::ObservabilityConfig(_)));
        });
    }

    #[tokio::test]
    async fn health_endpoint_returns_ok() {
        let repositories = Arc::new(InMemoryRepositories::new());
        let notifier = MockNotifier::default();
        let state = WebhookAppState {
            repositories,
            notifier,
            forgejo_secret: "secret".to_string(),
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
    async fn forgejo_webhook_forwards_payload() {
        let repositories = Arc::new(InMemoryRepositories::new());
        let mapping = RepoMapping::new("owner", "repo", "11".repeat(32), "repo").expect("mapping");
        let record = RepoMappingRecord::new(&mapping).expect("record");
        repositories
            .upsert_mapping(record)
            .await
            .expect("insert mapping");

        let notifier = MockNotifier::default();
        let state = WebhookAppState {
            repositories: repositories.clone(),
            notifier: notifier.clone(),
            forgejo_secret: "secret".to_string(),
        };
        let app = build_router(state);

        let payload = forgejo_push_payload();
        let signature = sign_payload(b"secret", payload.as_bytes());

        let response = app
            .oneshot(signed_request(
                payload.as_bytes(),
                "x-gitea-signature",
                &signature,
            ))
            .await
            .expect("response");
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let ack: WebhookAckPayload = serde_json::from_slice(&body).expect("ack");
        assert_eq!(ack.repo, "owner/repo");
        assert_eq!(ack.updates, 1);

        let payloads = notifier.payloads();
        assert_eq!(payloads.len(), 1);
        assert_eq!(payloads[0].identifier, "repo");
        assert_eq!(payloads[0].pubkey, "11".repeat(32));
    }

    #[tokio::test]
    async fn forgejo_webhook_rejects_missing_mapping() {
        let repositories = Arc::new(InMemoryRepositories::new());
        let notifier = MockNotifier::default();
        let state = WebhookAppState {
            repositories,
            notifier,
            forgejo_secret: "secret".to_string(),
        };
        let app = build_router(state);

        let payload = forgejo_push_payload();
        let signature = sign_payload(b"secret", payload.as_bytes());

        let response = app
            .oneshot(signed_request(
                payload.as_bytes(),
                "x-gitea-signature",
                &signature,
            ))
            .await
            .expect("response");
        assert_eq!(response.status(), axum::http::StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn forgejo_webhook_accepts_forgejo_signature_header() {
        let repositories = Arc::new(InMemoryRepositories::new());
        let mapping = RepoMapping::new("owner", "repo", "11".repeat(32), "repo").expect("mapping");
        let record = RepoMappingRecord::new(&mapping).expect("record");
        repositories
            .upsert_mapping(record)
            .await
            .expect("insert mapping");
        let notifier = MockNotifier::default();
        let state = WebhookAppState {
            repositories,
            notifier,
            forgejo_secret: "secret".to_string(),
        };
        let app = build_router(state);
        let payload = forgejo_push_payload();
        let signature = sign_payload(b"secret", payload.as_bytes());
        let response = app
            .oneshot(signed_request(
                payload.as_bytes(),
                "x-forgejo-signature",
                &signature,
            ))
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn forgejo_webhook_rejects_missing_signature_header() {
        let repositories = Arc::new(InMemoryRepositories::new());
        let state = WebhookAppState {
            repositories,
            notifier: MockNotifier::default(),
            forgejo_secret: "secret".to_string(),
        };
        let app = build_router(state);
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/")
                    .header("content-type", "application/json")
                    .body(Body::from(forgejo_push_payload()))
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn forgejo_webhook_rejects_invalid_signature() {
        let repositories = Arc::new(InMemoryRepositories::new());
        let state = WebhookAppState {
            repositories,
            notifier: MockNotifier::default(),
            forgejo_secret: "secret".to_string(),
        };
        let app = build_router(state);
        let response = app
            .oneshot(signed_request(
                forgejo_push_payload().as_bytes(),
                "x-gitea-signature",
                &"00".repeat(32),
            ))
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn forgejo_webhook_rejects_invalid_utf8_payload() {
        let repositories = Arc::new(InMemoryRepositories::new());
        let state = WebhookAppState {
            repositories,
            notifier: MockNotifier::default(),
            forgejo_secret: "secret".to_string(),
        };
        let app = build_router(state);
        let payload = [0xff, 0xfe, 0xfd];
        let signature = sign_payload(b"secret", &payload);
        let response = app
            .oneshot(signed_request(&payload, "x-gitea-signature", &signature))
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn forgejo_webhook_rejects_invalid_payload_json() {
        let repositories = Arc::new(InMemoryRepositories::new());
        let state = WebhookAppState {
            repositories,
            notifier: MockNotifier::default(),
            forgejo_secret: "secret".to_string(),
        };
        let app = build_router(state);
        let payload = b"not-json";
        let signature = sign_payload(b"secret", payload);
        let response = app
            .oneshot(signed_request(payload, "x-gitea-signature", &signature))
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn forgejo_webhook_returns_bad_gateway_when_notifier_fails() {
        let repositories = Arc::new(InMemoryRepositories::new());
        let mapping = RepoMapping::new("owner", "repo", "11".repeat(32), "repo").expect("mapping");
        let record = RepoMappingRecord::new(&mapping).expect("record");
        repositories
            .upsert_mapping(record)
            .await
            .expect("insert mapping");
        let state = WebhookAppState {
            repositories,
            notifier: FailingNotifier,
            forgejo_secret: "secret".to_string(),
        };
        let app = build_router(state);
        let payload = forgejo_push_payload();
        let signature = sign_payload(b"secret", payload.as_bytes());
        let response = app
            .oneshot(signed_request(
                payload.as_bytes(),
                "x-gitea-signature",
                &signature,
            ))
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    }

    #[tokio::test]
    async fn forgejo_webhook_returns_internal_on_mapping_error() {
        let state = WebhookAppState {
            repositories: Arc::new(ErrorRepoMappingRepository),
            notifier: MockNotifier::default(),
            forgejo_secret: "secret".to_string(),
        };
        let app = build_router(state);
        let payload = forgejo_push_payload();
        let signature = sign_payload(b"secret", payload.as_bytes());
        let response = app
            .oneshot(signed_request(
                payload.as_bytes(),
                "x-gitea-signature",
                &signature,
            ))
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[test]
    fn extract_signature_validates_missing_and_non_utf8_headers() {
        let mut headers = HeaderMap::new();
        let missing = super::extract_signature(&headers).expect_err("missing signature");
        assert_eq!(missing.into_response().status(), StatusCode::UNAUTHORIZED);

        headers.insert(
            "x-gitea-signature",
            HeaderValue::from_bytes(&[0xff, 0xfe]).expect("header"),
        );
        let invalid = super::extract_signature(&headers).expect_err("invalid signature");
        assert_eq!(invalid.into_response().status(), StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn webhook_error_display_and_source_cover_variants() {
        let config_variant = WebhookError::Config(WebhookConfigError::MissingEnv("MISSING"));
        assert!(format!("{config_variant}").contains("webhook error"));
        assert!(config_variant.source().is_some());

        let observability_config_variant =
            WebhookError::ObservabilityConfig(ObservabilityConfigError::InvalidEnv {
                key: "KEY",
                value: "bad".to_string(),
            });
        assert!(format!("{observability_config_variant}").contains("observability config error"));
        assert!(observability_config_variant.source().is_some());

        let observability_variant =
            WebhookError::Observability(ObservabilityError::MetricsInit("failed".to_string()));
        assert!(format!("{observability_variant}").contains("observability error"));
        assert!(observability_variant.source().is_some());

        let storage_variant = WebhookError::Storage(StorageError::Internal {
            message: "db".to_string(),
        });
        assert!(format!("{storage_variant}").contains("webhook storage error"));
        assert!(storage_variant.source().is_some());

        let notify_variant = WebhookError::Notify("sync".to_string());
        assert_eq!(format!("{notify_variant}"), "webhook notify error: sync");
        assert!(notify_variant.source().is_none());

        let serve_variant = WebhookError::Serve("bind".to_string());
        assert_eq!(format!("{serve_variant}"), "webhook serve error: bind");
        assert!(serve_variant.source().is_none());
    }

    #[test]
    fn webhook_config_and_storage_error_display_paths_are_stable() {
        let config_error = WebhookConfigError::Config(ConfigError::InvalidConfig {
            field: "field",
            value: "value".to_string(),
        });
        assert!(format!("{config_error}").contains("webhook config error"));
        assert!(config_error.source().is_some());

        let storage_error = WebhookConfigError::Storage(StorageConfigError::MissingEnv("READ_URL"));
        assert!(format!("{storage_error}").contains("webhook storage config error"));
        assert!(storage_error.source().is_some());

        let missing_env = WebhookConfigError::MissingEnv("ENV_KEY");
        assert_eq!(format!("{missing_env}"), "missing env ENV_KEY");
        assert!(missing_env.source().is_none());

        let invalid_env = WebhookConfigError::InvalidEnv {
            key: "ENV_KEY",
            value: "bad".to_string(),
        };
        assert_eq!(format!("{invalid_env}"), "invalid env ENV_KEY: bad");
        assert!(invalid_env.source().is_none());

        assert_eq!(
            format!("{}", StorageConfigError::MissingEnv("READ_URL")),
            "missing env READ_URL"
        );
        assert_eq!(
            format!(
                "{}",
                StorageConfigError::InvalidEnv {
                    key: "MAX",
                    value: "bad".to_string()
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
    fn build_repositories_returns_storage_error_for_invalid_pool_settings() {
        let config = WebhookConfig {
            bind: "127.0.0.1:8087".to_string(),
            storage: StorageConfig {
                read_connection: "postgres://user:pass@localhost:5432/gittree".to_string(),
                write_connection: None,
                max_connections: 0,
                min_connections: 0,
                idle_timeout_secs: None,
                max_lifetime_secs: None,
                application_name: None,
            },
            sync_url: "http://localhost:8084".to_string(),
            forgejo_secret: "secret".to_string(),
        };
        let err = super::build_repositories(&config).expect_err("expected error");
        assert!(matches!(err, WebhookError::Storage(_)));
    }

    #[tokio::test]
    async fn build_repositories_accepts_valid_storage_config() {
        let config = WebhookConfig {
            bind: "127.0.0.1:8087".to_string(),
            storage: StorageConfig {
                read_connection: "postgres://user:pass@localhost:5432/gittree".to_string(),
                write_connection: None,
                max_connections: 10,
                min_connections: 1,
                idle_timeout_secs: None,
                max_lifetime_secs: None,
                application_name: None,
            },
            sync_url: "http://localhost:8084".to_string(),
            forgejo_secret: "secret".to_string(),
        };
        let _repos = super::build_repositories(&config).expect("repositories");
    }

    #[tokio::test]
    async fn http_sync_notifier_accepts_success_status() {
        let (endpoint, handle) = start_mock_http_server("200 OK", "application/json", "{}");
        let notifier = HttpSyncNotifier::new(endpoint).expect("notifier");
        notifier
            .notify(sample_sync_payload())
            .await
            .expect("notify");
        handle.join().expect("server join");
    }

    #[tokio::test]
    async fn http_sync_notifier_reports_non_success_status() {
        let (endpoint, handle) =
            start_mock_http_server("500 Internal Server Error", "text/plain", "failed");
        let notifier = HttpSyncNotifier::new(endpoint).expect("notifier");
        let err = notifier
            .notify(sample_sync_payload())
            .await
            .expect_err("should fail");
        assert!(err.contains("sync error"));
        handle.join().expect("server join");
    }

    #[tokio::test]
    async fn http_sync_notifier_reports_transport_error() {
        let notifier = HttpSyncNotifier::new("http://127.0.0.1:1").expect("notifier");
        let err = notifier
            .notify(sample_sync_payload())
            .await
            .expect_err("transport error");
        assert!(!err.is_empty());
    }

    #[test]
    fn http_sync_notifier_new_maps_builder_errors() {
        let result = HttpSyncNotifier::new_with_result(
            "http://localhost:8084",
            Err::<reqwest::Client, _>("builder failed"),
        );
        assert!(matches!(result, Err(message) if message.contains("builder failed")));
    }

    #[tokio::test]
    async fn error_repo_mapping_repository_methods_are_callable() {
        let repo = ErrorRepoMappingRepository;
        let mapping = RepoMapping::new("owner", "repo", "11".repeat(32), "repo").expect("mapping");
        let record = RepoMappingRecord::new(&mapping).expect("record");
        repo.upsert_mapping(record).await.expect("upsert");
        assert!(
            repo.mapping_by_repo(&[1u8; 32], "repo")
                .await
                .expect("mapping by repo")
                .is_none()
        );
        assert!(repo.list_mappings().await.expect("list").is_empty());
    }

    #[test]
    fn webhook_app_state_clone_copies_fields() {
        let repositories = Arc::new(InMemoryRepositories::new());
        let state = WebhookAppState {
            repositories: Arc::clone(&repositories),
            notifier: MockNotifier::default(),
            forgejo_secret: "secret".to_string(),
        };
        let cloned = state.clone();
        assert_eq!(cloned.forgejo_secret, "secret");
        assert!(Arc::ptr_eq(&cloned.repositories, &repositories));
    }

    #[tokio::test]
    async fn serve_returns_serve_error_for_invalid_bind() {
        let config = WebhookConfig {
            bind: "invalid-bind".to_string(),
            storage: StorageConfig {
                read_connection: "postgres://user:pass@localhost:5432/gittree".to_string(),
                write_connection: None,
                max_connections: 10,
                min_connections: 1,
                idle_timeout_secs: None,
                max_lifetime_secs: None,
                application_name: None,
            },
            sync_url: "http://localhost:8084".to_string(),
            forgejo_secret: "secret".to_string(),
        };
        let err = super::serve_with_init(config, || Ok(()))
            .await
            .expect_err("serve error");
        assert!(matches!(err, WebhookError::Serve(_)));
    }

    #[tokio::test]
    async fn serve_with_init_runs_until_cancelled() {
        let config = WebhookConfig {
            bind: "127.0.0.1:0".to_string(),
            storage: StorageConfig {
                read_connection: "postgres://user:pass@localhost:5432/gittree".to_string(),
                write_connection: None,
                max_connections: 10,
                min_connections: 1,
                idle_timeout_secs: None,
                max_lifetime_secs: None,
                application_name: None,
            },
            sync_url: "http://localhost:8084".to_string(),
            forgejo_secret: "secret".to_string(),
        };
        let task = tokio::spawn(super::serve_with_init(config, || Ok(())));
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        task.abort();
        let _ = task.await;
    }

    #[tokio::test]
    async fn run_http_server_with_shutdown_returns_ok() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind listener");
        let result = super::run_http_server_with_shutdown(listener, Router::new(), async {}).await;
        assert!(result.is_ok());
    }

    #[test]
    fn map_serve_result_maps_io_errors() {
        let err = super::map_serve_result(Err(std::io::Error::other("boom")))
            .expect_err("serve error");
        assert!(matches!(err, WebhookError::Serve(message) if message.contains("boom")));
    }
}
