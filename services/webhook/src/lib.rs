use async_trait::async_trait;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use gittree_config::{ConfigError, ServicesConfig};
use gittree_git_hook::{PostReceivePayload, RefUpdatePayload, parse_forgejo_push, verify_forgejo_signature};
use gittree_observability::{ObservabilityConfigError, ObservabilityError, ObservabilityHandle};
use gittree_storage::{PostgresRepositories, RepoMappingRepository, StorageConfig, StorageError};
use serde::{Deserialize, Serialize};
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
        let services =
            ServicesConfig::from_env_validated().map_err(WebhookConfigError::Config)?;
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
        let client = reqwest::Client::builder()
            .build()
            .map_err(|err| err.to_string())?;
        Ok(Self {
            endpoint: endpoint.into(),
            client,
        })
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
    let _observability = init_observability()?;
    let repositories = build_repositories(&config)?;
    let notifier = HttpSyncNotifier::new(config.sync_url.clone())
        .map_err(WebhookError::Notify)?;
    let state = WebhookAppState {
        repositories: Arc::new(repositories),
        notifier,
        forgejo_secret: config.forgejo_secret,
    };
    let router = build_router(state);
    let listener = tokio::net::TcpListener::bind(&config.bind)
        .await
        .map_err(|err| WebhookError::Serve(err.to_string()))?;
    axum::serve(listener, router)
        .await
        .map_err(|err| WebhookError::Serve(err.to_string()))?;
    Ok(())
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
            WebhookHttpError::Internal(message) => {
                (StatusCode::INTERNAL_SERVER_ERROR, message)
            }
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
    let event = parse_forgejo_push(payload)
        .map_err(|err| WebhookHttpError::BadRequest(err.to_string()))?;
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
    use super::WebhookAckPayload;
    use super::WebhookAppState;
    use super::WebhookConfig;
    use super::SyncNotifier;
    use super::build_router;
    use async_trait::async_trait;
    use axum::body::Body;
    use axum::http::Request;
    use gittree_core::RepoMapping;
    use gittree_storage::{InMemoryRepositories, RepoMappingRecord, RepoMappingRepository};
    use hmac::Mac;
    use std::sync::{Arc, Mutex};
    use tower::ServiceExt;

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

    fn with_env_var<F: FnOnce()>(key: &str, value: &str, f: F) {
        let previous = std::env::var_os(key);
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
            .oneshot(Request::builder().uri("/health").body(Body::empty()).unwrap())
            .await
            .expect("response");
        assert_eq!(response.status(), axum::http::StatusCode::OK);
    }

    #[tokio::test]
    async fn forgejo_webhook_forwards_payload() {
        let repositories = Arc::new(InMemoryRepositories::new());
        let mapping = RepoMapping::new(
            "owner",
            "repo",
            "11".repeat(32),
            "repo",
        )
        .expect("mapping");
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

        let payload = r#"
        {
            "ref": "refs/heads/main",
            "before": "0000000000000000000000000000000000000000",
            "after": "1111111111111111111111111111111111111111",
            "repository": {
                "name": "repo",
                "full_name": "owner/repo",
                "owner": { "username": "owner" }
            }
        }"#;

        let mut mac =
            hmac::Hmac::<sha2::Sha256>::new_from_slice(b"secret").expect("mac");
        mac.update(payload.as_bytes());
        let signature = hex::encode(mac.finalize().into_bytes());

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/")
                    .header("content-type", "application/json")
                    .header("x-gitea-signature", format!("sha256={signature}"))
                    .body(Body::from(payload))
                    .unwrap(),
            )
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

        let payload = r#"
        {
            "ref": "refs/heads/main",
            "before": "0000000000000000000000000000000000000000",
            "after": "1111111111111111111111111111111111111111",
            "repository": {
                "name": "repo",
                "full_name": "owner/repo",
                "owner": { "username": "owner" }
            }
        }"#;

        let mut mac =
            hmac::Hmac::<sha2::Sha256>::new_from_slice(b"secret").expect("mac");
        mac.update(payload.as_bytes());
        let signature = hex::encode(mac.finalize().into_bytes());

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/")
                    .header("content-type", "application/json")
                    .header("x-gitea-signature", format!("sha256={signature}"))
                    .body(Body::from(payload))
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), axum::http::StatusCode::NOT_FOUND);
    }
}
