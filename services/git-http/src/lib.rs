use async_trait::async_trait;
use axum::Router;
use axum::body::{Body, Bytes, to_bytes};
use axum::extract::State;
use axum::http::{HeaderMap, Method, Request, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use gittree_config::{AuthConfig, ConfigError, ServicesConfig};
use gittree_nostr_auth::{Nip98Event, Nip98Request, validate_nip98};
use gittree_observability::{ObservabilityConfigError, ObservabilityError, ObservabilityHandle};
use gittree_storage::{
    AnnouncementRepository, PostgresRepositories, RepoMappingRecord, RepoMappingRepository,
    StorageConfig, StorageError,
};
use opentelemetry::KeyValue;
use opentelemetry::metrics::{Counter, Histogram};
use sha2::Digest;
use std::collections::HashSet;
use std::future::Future;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const ENV_UPSTREAM_URL: &str = "GITTREE_GIT_HTTP_UPSTREAM_URL";
const ENV_TIMEOUT_SECS: &str = "GITTREE_GIT_HTTP_TIMEOUT_SECS";
const DEFAULT_TIMEOUT_SECS: u64 = 10;
const AUTH_HEADER: &str = "authorization";
const ENV_STORAGE_READ_URL: &str = "GITTREE_STORAGE_READ_URL";
const ENV_STORAGE_WRITE_URL: &str = "GITTREE_STORAGE_WRITE_URL";
const ENV_STORAGE_MAX_CONNECTIONS: &str = "GITTREE_STORAGE_MAX_CONNECTIONS";
const ENV_STORAGE_MIN_CONNECTIONS: &str = "GITTREE_STORAGE_MIN_CONNECTIONS";
const ENV_STORAGE_IDLE_TIMEOUT_SECS: &str = "GITTREE_STORAGE_IDLE_TIMEOUT_SECS";
const ENV_STORAGE_MAX_LIFETIME_SECS: &str = "GITTREE_STORAGE_MAX_LIFETIME_SECS";
const ENV_STORAGE_APP_NAME: &str = "GITTREE_STORAGE_APP_NAME";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitHttpConfig {
    pub bind: String,
    pub upstream_url: String,
    pub timeout: Duration,
    pub auth: AuthConfig,
    pub storage: StorageConfig,
}

impl GitHttpConfig {
    pub fn from_env() -> Result<Self, GitHttpConfigError> {
        let services = ServicesConfig::from_env_validated().map_err(GitHttpConfigError::Config)?;
        let auth = AuthConfig::from_env().map_err(GitHttpConfigError::Config)?;
        let upstream_url = match std::env::var(ENV_UPSTREAM_URL) {
            Ok(value) => value,
            Err(_) => return Err(GitHttpConfigError::MissingEnv(ENV_UPSTREAM_URL)),
        };
        if url::Url::parse(&upstream_url).is_err() {
            return Err(GitHttpConfigError::InvalidEnv {
                key: ENV_UPSTREAM_URL,
                value: upstream_url,
            });
        }
        let timeout_secs = env_u64(ENV_TIMEOUT_SECS)?.unwrap_or(DEFAULT_TIMEOUT_SECS);
        let storage = storage_from_env()?;
        Ok(Self {
            bind: services.git_http.bind,
            upstream_url,
            timeout: Duration::from_secs(timeout_secs),
            auth,
            storage,
        })
    }
}

#[derive(Debug)]
pub enum GitHttpConfigError {
    Config(ConfigError),
    MissingEnv(&'static str),
    InvalidEnv { key: &'static str, value: String },
    Storage(StorageConfigError),
}

impl std::fmt::Display for GitHttpConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GitHttpConfigError::Config(err) => write!(f, "git-http config error: {err}"),
            GitHttpConfigError::MissingEnv(key) => write!(f, "missing env {key}"),
            GitHttpConfigError::InvalidEnv { key, value } => {
                write!(f, "invalid env {key}: {value}")
            }
            GitHttpConfigError::Storage(err) => write!(f, "git-http storage config error: {err}"),
        }
    }
}

impl std::error::Error for GitHttpConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            GitHttpConfigError::Config(err) => Some(err),
            GitHttpConfigError::MissingEnv(_) => None,
            GitHttpConfigError::InvalidEnv { .. } => None,
            GitHttpConfigError::Storage(err) => Some(err),
        }
    }
}

fn env_u64(key: &'static str) -> Result<Option<u64>, GitHttpConfigError> {
    match std::env::var(key) {
        Ok(value) => {
            if value.trim().is_empty() {
                return Ok(None);
            }
            match value.parse::<u64>() {
                Ok(parsed) => Ok(Some(parsed)),
                Err(_) => Err(GitHttpConfigError::InvalidEnv { key, value }),
            }
        }
        Err(_) => Ok(None),
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

fn storage_from_env() -> Result<StorageConfig, GitHttpConfigError> {
    let read_connection = match std::env::var(ENV_STORAGE_READ_URL) {
        Ok(value) => value,
        Err(_) => {
            return Err(GitHttpConfigError::Storage(StorageConfigError::MissingEnv(
                ENV_STORAGE_READ_URL,
            )));
        }
    };
    let write_connection = std::env::var(ENV_STORAGE_WRITE_URL).ok();
    let max_connections = storage_env_u32(ENV_STORAGE_MAX_CONNECTIONS)?.unwrap_or(10);
    let min_connections = storage_env_u32(ENV_STORAGE_MIN_CONNECTIONS)?.unwrap_or(2);
    let idle_timeout_secs = storage_env_u64(ENV_STORAGE_IDLE_TIMEOUT_SECS)?;
    let max_lifetime_secs = storage_env_u64(ENV_STORAGE_MAX_LIFETIME_SECS)?;
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

    if let Err(err) = config.validate() {
        return Err(GitHttpConfigError::Storage(
            StorageConfigError::InvalidConfig(err.to_string()),
        ));
    }

    Ok(config)
}

fn storage_env_u32(key: &'static str) -> Result<Option<u32>, GitHttpConfigError> {
    match std::env::var(key) {
        Ok(value) => {
            if value.trim().is_empty() {
                return Ok(None);
            }
            match value.parse::<u32>() {
                Ok(parsed) => Ok(Some(parsed)),
                Err(_) => Err(GitHttpConfigError::Storage(
                    StorageConfigError::InvalidEnv { key, value },
                )),
            }
        }
        Err(_) => Ok(None),
    }
}

fn storage_env_u64(key: &'static str) -> Result<Option<u64>, GitHttpConfigError> {
    match std::env::var(key) {
        Ok(value) => {
            if value.trim().is_empty() {
                return Ok(None);
            }
            match value.parse::<u64>() {
                Ok(parsed) => Ok(Some(parsed)),
                Err(_) => Err(GitHttpConfigError::Storage(
                    StorageConfigError::InvalidEnv { key, value },
                )),
            }
        }
        Err(_) => Ok(None),
    }
}

#[derive(Debug)]
pub enum GitHttpError {
    Config(GitHttpConfigError),
    ObservabilityConfig(ObservabilityConfigError),
    Observability(ObservabilityError),
    Storage(StorageError),
    Upstream(String),
    Serve(String),
}

impl std::fmt::Display for GitHttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GitHttpError::Config(err) => write!(f, "git-http error: {err}"),
            GitHttpError::ObservabilityConfig(err) => {
                write!(f, "git-http observability config error: {err}")
            }
            GitHttpError::Observability(err) => write!(f, "git-http observability error: {err}"),
            GitHttpError::Storage(err) => write!(f, "git-http storage error: {err}"),
            GitHttpError::Upstream(err) => write!(f, "git-http upstream error: {err}"),
            GitHttpError::Serve(err) => write!(f, "git-http serve error: {err}"),
        }
    }
}

impl std::error::Error for GitHttpError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            GitHttpError::Config(err) => Some(err),
            GitHttpError::ObservabilityConfig(err) => Some(err),
            GitHttpError::Observability(err) => Some(err),
            GitHttpError::Storage(err) => Some(err),
            GitHttpError::Upstream(_) => None,
            GitHttpError::Serve(_) => None,
        }
    }
}

pub fn init_observability() -> Result<ObservabilityHandle, GitHttpError> {
    let config = gittree_observability::ObservabilityConfig::from_env("gittree-git-http")
        .map_err(GitHttpError::ObservabilityConfig)?;
    let handle = gittree_observability::init(&config).map_err(GitHttpError::Observability)?;
    Ok(handle)
}

#[derive(Debug, Clone)]
pub struct GitHttpMetrics {
    request_duration: Histogram<f64>,
    request_total: Counter<u64>,
}

impl GitHttpMetrics {
    pub fn new() -> Self {
        let meter = opentelemetry::global::meter("gittree-git-http");
        let request_duration = meter
            .f64_histogram("gittree_git_http_request_duration_seconds")
            .with_description("Duration of git-http requests in seconds")
            .init();
        let request_total = meter
            .u64_counter("gittree_git_http_request_total")
            .with_description("Total number of git-http requests")
            .init();
        Self {
            request_duration,
            request_total,
        }
    }

    pub fn record(&self, route: &GitHttpRoute, status: u16, duration: Duration) {
        let labels = [
            KeyValue::new("route", route_label(route)),
            KeyValue::new("status", status.to_string()),
        ];
        self.request_duration
            .record(duration.as_secs_f64(), &labels);
        self.request_total.add(1, &labels);
        let route_name = route_label(route);
        let duration_ms = duration.as_millis();
        tracing::info!(
            route = route_name,
            status,
            duration_ms,
            event = "git-http request handled"
        );
    }
}

pub async fn serve(config: GitHttpConfig) -> Result<(), GitHttpError> {
    serve_with(config, init_observability, run_axum_server).await
}

fn run_axum_server(
    listener: tokio::net::TcpListener,
    router: Router,
) -> impl Future<Output = Result<(), std::io::Error>> + Send + 'static {
    async move { axum::serve(listener, router).await }
}

async fn serve_with<Obs, InitObs, ServeFn, ServeFut>(
    config: GitHttpConfig,
    init_observability_fn: InitObs,
    serve_fn: ServeFn,
) -> Result<(), GitHttpError>
where
    InitObs: FnOnce() -> Result<Obs, GitHttpError>,
    ServeFn: FnOnce(tokio::net::TcpListener, Router) -> ServeFut,
    ServeFut: Future<Output = Result<(), std::io::Error>>,
{
    let _observability = init_observability_fn()?;
    let metrics = Arc::new(GitHttpMetrics::new());
    let repositories = build_repositories(&config)?;
    let upstream = ReqwestUpstreamClient::new(config.timeout)?;
    let auth = config.auth;
    let state = GitHttpAppState {
        repositories: Arc::new(repositories),
        upstream: Arc::new(upstream),
        metrics,
        upstream_url: config.upstream_url.trim_end_matches('/').to_string(),
        auth,
    };
    let router = build_router(state);
    let listener = match tokio::net::TcpListener::bind(&config.bind).await {
        Ok(listener) => listener,
        Err(err) => return Err(GitHttpError::Serve(err.to_string())),
    };
    match serve_fn(listener, router).await {
        Ok(()) => Ok(()),
        Err(err) => Err(GitHttpError::Serve(err.to_string())),
    }?;
    Ok(())
}

fn build_repositories(config: &GitHttpConfig) -> Result<PostgresRepositories, GitHttpError> {
    let pool_options = config
        .storage
        .pool_options()
        .map_err(GitHttpError::Storage)?;
    let connect_options = config
        .storage
        .read_connect_options()
        .map_err(GitHttpError::Storage)?;
    let pool = pool_options.connect_lazy_with(connect_options);
    Ok(PostgresRepositories::new(pool))
}

struct GitHttpAppState<R, U> {
    auth: AuthConfig,
    repositories: Arc<R>,
    upstream: Arc<U>,
    metrics: Arc<GitHttpMetrics>,
    upstream_url: String,
}

impl<R, U> Clone for GitHttpAppState<R, U> {
    fn clone(&self) -> Self {
        Self {
            auth: self.auth.clone(),
            repositories: Arc::clone(&self.repositories),
            upstream: Arc::clone(&self.upstream),
            metrics: Arc::clone(&self.metrics),
            upstream_url: self.upstream_url.clone(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct UpstreamRequest {
    pub method: Method,
    pub url: String,
    pub headers: HeaderMap,
    pub body: Bytes,
}

#[derive(Debug, Clone)]
pub struct UpstreamResponse {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: Bytes,
}

#[derive(Debug)]
pub enum UpstreamError {
    Request(String),
}

impl std::fmt::Display for UpstreamError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UpstreamError::Request(message) => write!(f, "{message}"),
        }
    }
}

#[async_trait]
pub trait UpstreamClient: Send + Sync {
    async fn send(&self, request: UpstreamRequest) -> Result<UpstreamResponse, UpstreamError>;
}

pub struct ReqwestUpstreamClient {
    client: reqwest::Client,
}

impl ReqwestUpstreamClient {
    pub fn new(timeout: Duration) -> Result<Self, GitHttpError> {
        Self::from_builder_result(
            reqwest::Client::builder()
                .timeout(timeout)
                .build()
                .map_err(|err| err.to_string()),
        )
    }

    fn from_builder_result(result: Result<reqwest::Client, String>) -> Result<Self, GitHttpError> {
        let client = result.map_err(GitHttpError::Upstream)?;
        Ok(Self { client })
    }

    #[cfg(test)]
    fn new_with<F>(timeout: Duration, build_client: F) -> Result<Self, GitHttpError>
    where
        F: FnOnce(Duration) -> Result<reqwest::Client, String>,
    {
        Self::from_builder_result(build_client(timeout))
    }
}

#[async_trait]
impl UpstreamClient for ReqwestUpstreamClient {
    async fn send(&self, request: UpstreamRequest) -> Result<UpstreamResponse, UpstreamError> {
        let mut builder = self.client.request(request.method, request.url);
        builder = builder.headers(request.headers);
        builder = builder.body(request.body);
        let response = match builder.send().await {
            Ok(response) => response,
            Err(err) => return Err(UpstreamError::Request(err.to_string())),
        };
        let status =
            StatusCode::from_u16(response.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
        let headers = response.headers().clone();
        let body = match response.bytes().await {
            Ok(body) => body,
            Err(err) => return Err(UpstreamError::Request(err.to_string())),
        };
        Ok(UpstreamResponse {
            status,
            headers,
            body,
        })
    }
}

#[derive(Debug)]
enum GitHttpHttpError {
    NotFound(String),
    BadRequest(String),
    Unauthorized(String),
    Storage(String),
    Upstream(String),
    Internal(String),
}

impl IntoResponse for GitHttpHttpError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            GitHttpHttpError::NotFound(message) => (StatusCode::NOT_FOUND, message),
            GitHttpHttpError::BadRequest(message) => (StatusCode::BAD_REQUEST, message),
            GitHttpHttpError::Unauthorized(message) => (StatusCode::UNAUTHORIZED, message),
            GitHttpHttpError::Storage(message) => (StatusCode::INTERNAL_SERVER_ERROR, message),
            GitHttpHttpError::Upstream(message) => (StatusCode::BAD_GATEWAY, message),
            GitHttpHttpError::Internal(message) => (StatusCode::INTERNAL_SERVER_ERROR, message),
        };
        (status, message).into_response()
    }
}

fn build_router<R, U>(state: GitHttpAppState<R, U>) -> Router
where
    R: RepoMappingRepository + AnnouncementRepository + Send + Sync + 'static,
    U: UpstreamClient + Send + Sync + 'static,
{
    Router::new()
        .route("/health", get(health_handler))
        .fallback(git_handler)
        .with_state(state)
}

async fn health_handler() -> &'static str {
    "ok"
}

async fn git_handler<R, U>(
    State(state): State<GitHttpAppState<R, U>>,
    request: Request<Body>,
) -> Result<Response, GitHttpHttpError>
where
    R: RepoMappingRepository + AnnouncementRepository + Send + Sync,
    U: UpstreamClient + Send + Sync,
{
    let method = request.method().clone();
    let uri = request.uri().clone();
    let route = route_request(&GitHttpRequest::new(
        method.as_str(),
        uri.path(),
        uri.query(),
    ));
    let start = Instant::now();
    let response = match handle_git_route(&state, &route, request).await {
        Ok(response) => response,
        Err(err) => err.into_response(),
    };
    state
        .metrics
        .record(&route, response.status().as_u16(), start.elapsed());
    Ok(response)
}

async fn handle_git_route<R, U>(
    state: &GitHttpAppState<R, U>,
    route: &GitHttpRoute,
    request: Request<Body>,
) -> Result<Response, GitHttpHttpError>
where
    R: RepoMappingRepository + AnnouncementRepository + Send + Sync,
    U: UpstreamClient + Send + Sync,
{
    let (repo, suffix, query) = match route {
        GitHttpRoute::InfoRefs { repo, .. } => (repo, "/info/refs", request.uri().query()),
        GitHttpRoute::UploadPack { repo } => (repo, "/git-upload-pack", None),
        GitHttpRoute::ReceivePack { repo } => (repo, "/git-receive-pack", None),
        GitHttpRoute::NotFound => {
            return Err(GitHttpHttpError::NotFound("not found".to_string()));
        }
    };

    let mapping = resolve_mapping(state.repositories.as_ref(), repo).await?;
    let mut url = format!(
        "{}/{}/{}.git{}",
        state.upstream_url, mapping.forgejo_owner, mapping.forgejo_repo, suffix
    );
    if let Some(query) = query {
        url.push('?');
        url.push_str(query);
    }

    let (parts, body) = request.into_parts();
    let mut headers = parts.headers;
    let auth_headers = headers.clone();
    headers.remove(axum::http::header::HOST);
    headers.remove(AUTH_HEADER);
    let body = match to_bytes(body, usize::MAX).await {
        Ok(body) => body,
        Err(err) => return Err(GitHttpHttpError::Internal(err.to_string())),
    };

    if matches!(route, GitHttpRoute::ReceivePack { .. }) {
        authorize_receive_pack(state, repo, &auth_headers, &parts.method, &parts.uri, &body)
            .await?;
    }

    let upstream_request = UpstreamRequest {
        method: parts.method,
        url,
        headers,
        body,
    };
    let upstream_response = match state.upstream.send(upstream_request).await {
        Ok(response) => response,
        Err(err) => return Err(GitHttpHttpError::Upstream(err.to_string())),
    };

    let mut response = Response::new(Body::from(upstream_response.body));
    *response.status_mut() = upstream_response.status;
    *response.headers_mut() = upstream_response.headers;
    Ok(response)
}

async fn resolve_mapping<R>(
    repositories: &R,
    repo: &NormalizedRepo,
) -> Result<RepoMappingRecord, GitHttpHttpError>
where
    R: RepoMappingRepository + Send + Sync,
{
    let pubkey = match hex::decode(&repo.pubkey) {
        Ok(pubkey) => pubkey,
        Err(_) => {
            return Err(GitHttpHttpError::Internal(
                "invalid repo pubkey".to_string(),
            ));
        }
    };
    let record = match repositories
        .mapping_by_repo(&pubkey, &repo.identifier)
        .await
    {
        Ok(record) => record,
        Err(err) => return Err(GitHttpHttpError::Storage(err.to_string())),
    };
    match record {
        Some(record) => Ok(record),
        None => Err(GitHttpHttpError::NotFound(
            "missing repo mapping".to_string(),
        )),
    }
}

async fn authorize_receive_pack<R, U>(
    state: &GitHttpAppState<R, U>,
    repo: &NormalizedRepo,
    headers: &HeaderMap,
    method: &Method,
    uri: &axum::http::Uri,
    body: &Bytes,
) -> Result<(), GitHttpHttpError>
where
    R: RepoMappingRepository + AnnouncementRepository + Send + Sync,
    U: UpstreamClient + Send + Sync,
{
    let event = parse_nostr_auth(headers)?;
    let request_url = build_request_url(headers, uri)?;
    let payload_hash = payload_hash(body);
    let request = Nip98Request {
        method: method.as_str(),
        url: &request_url,
        payload_sha256: payload_hash.as_deref(),
        now: unix_timestamp(),
        max_skew_seconds: state.auth.max_skew_seconds as i64,
    };
    let auth = match validate_nip98(&event, &request) {
        Ok(auth) => auth,
        Err(err) => return Err(GitHttpHttpError::Unauthorized(err.to_string())),
    };
    let maintainers = resolve_maintainers(state.repositories.as_ref(), repo).await?;
    if !maintainers
        .iter()
        .any(|pubkey| pubkey.eq_ignore_ascii_case(&auth.pubkey))
    {
        return Err(GitHttpHttpError::Unauthorized(
            "pubkey not authorized".to_string(),
        ));
    }
    Ok(())
}

async fn resolve_maintainers<R>(
    repositories: &R,
    repo: &NormalizedRepo,
) -> Result<Vec<String>, GitHttpHttpError>
where
    R: AnnouncementRepository + Send + Sync,
{
    let mut pending = vec![repo.pubkey.to_lowercase()];
    let mut seen = HashSet::new();

    while let Some(pubkey) = pending.pop() {
        if !seen.insert(pubkey.clone()) {
            continue;
        }
        let pubkey_bytes = match hex::decode(&pubkey) {
            Ok(pubkey_bytes) => pubkey_bytes,
            Err(_) => {
                return Err(GitHttpHttpError::Internal(
                    "invalid maintainer pubkey".to_string(),
                ));
            }
        };
        let announcement = match repositories
            .latest_announcement(&pubkey_bytes, &repo.identifier)
            .await
        {
            Ok(announcement) => announcement,
            Err(err) => return Err(GitHttpHttpError::Storage(err.to_string())),
        };
        let Some(announcement) = announcement else {
            continue;
        };
        for maintainer in announcement.maintainers {
            let maintainer = maintainer.to_lowercase();
            if !seen.contains(&maintainer) {
                pending.push(maintainer);
            }
        }
    }

    let mut maintainers: Vec<String> = seen.into_iter().collect();
    maintainers.sort();
    Ok(maintainers)
}

fn build_request_url(
    headers: &HeaderMap,
    uri: &axum::http::Uri,
) -> Result<String, GitHttpHttpError> {
    let host = if let Some(value) = headers.get("host") {
        match value.to_str() {
            Ok(host) => host,
            Err(_) => {
                return Err(GitHttpHttpError::BadRequest(
                    "missing host header".to_string(),
                ));
            }
        }
    } else {
        return Err(GitHttpHttpError::BadRequest(
            "missing host header".to_string(),
        ));
    };
    let scheme = if let Some(value) = headers.get("x-forwarded-proto") {
        match value.to_str() {
            Ok(scheme) => scheme,
            Err(_) => "http",
        }
    } else {
        "http"
    };
    let path = if let Some(query) = uri.query() {
        format!("{}?{query}", uri.path())
    } else {
        uri.path().to_string()
    };
    Ok(format!("{scheme}://{host}{path}"))
}

fn parse_nostr_auth(headers: &HeaderMap) -> Result<Nip98Event, GitHttpHttpError> {
    let value = if let Some(header) = headers.get(AUTH_HEADER) {
        match header.to_str() {
            Ok(value) => value,
            Err(_) => {
                return Err(GitHttpHttpError::Unauthorized(
                    "missing authorization".to_string(),
                ));
            }
        }
    } else {
        return Err(GitHttpHttpError::Unauthorized(
            "missing authorization".to_string(),
        ));
    };
    let value = value.trim();
    let Some(token) = value.strip_prefix("Nostr ") else {
        return Err(GitHttpHttpError::Unauthorized(
            "invalid authorization header".to_string(),
        ));
    };
    let decoded = match BASE64_STANDARD.decode(token.as_bytes()) {
        Ok(decoded) => decoded,
        Err(_) => {
            return Err(GitHttpHttpError::Unauthorized(
                "invalid nostr authorization".to_string(),
            ));
        }
    };
    match serde_json::from_slice::<Nip98Event>(&decoded) {
        Ok(event) => Ok(event),
        Err(_) => Err(GitHttpHttpError::Unauthorized(
            "invalid nostr event".to_string(),
        )),
    }
}

fn payload_hash(body: &Bytes) -> Option<String> {
    if body.is_empty() {
        return None;
    }
    let mut hasher = sha2::Sha256::new();
    hasher.update(body);
    let digest = hasher.finalize();
    Some(hex::encode(digest))
}

fn unix_timestamp() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitHttpRequest<'a> {
    pub method: &'a str,
    pub path: &'a str,
    pub query: Option<&'a str>,
}

impl<'a> GitHttpRequest<'a> {
    pub fn new(method: &'a str, path: &'a str, query: Option<&'a str>) -> Self {
        Self {
            method,
            path,
            query,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitHttpRoute {
    InfoRefs {
        repo: NormalizedRepo,
        service: GitHttpService,
    },
    UploadPack {
        repo: NormalizedRepo,
    },
    ReceivePack {
        repo: NormalizedRepo,
    },
    NotFound,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitHttpService {
    UploadPack,
    ReceivePack,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedRepo {
    pub npub: String,
    pub identifier: String,
    pub canonical_path: String,
    pub pubkey: String,
}

#[derive(Debug, Default)]
pub struct GitHttpRouter;

impl GitHttpRouter {
    pub fn new() -> Self {
        Self
    }

    pub fn route(&self, request: &GitHttpRequest<'_>) -> GitHttpRoute {
        route_request(request)
    }
}

fn route_label(route: &GitHttpRoute) -> &'static str {
    match route {
        GitHttpRoute::InfoRefs { .. } => "info_refs",
        GitHttpRoute::UploadPack { .. } => "upload_pack",
        GitHttpRoute::ReceivePack { .. } => "receive_pack",
        GitHttpRoute::NotFound => "not_found",
    }
}

pub fn route_request(request: &GitHttpRequest<'_>) -> GitHttpRoute {
    let (npub, repo_segment, rest) = match split_repo_segments(request.path) {
        Some(parts) => parts,
        None => return GitHttpRoute::NotFound,
    };
    if !repo_segment.ends_with(".git") {
        return GitHttpRoute::NotFound;
    }
    let repo = match normalize_repo_path(&npub, &repo_segment) {
        Ok(repo) => repo,
        Err(_) => return GitHttpRoute::NotFound,
    };
    if rest.len() == 2 && rest[0] == "info" && rest[1] == "refs" && is_get(request.method) {
        let service = match parse_service(request.query) {
            Ok(service) => service,
            Err(_) => return GitHttpRoute::NotFound,
        };
        return GitHttpRoute::InfoRefs { repo, service };
    }
    if rest.len() == 1 && rest[0] == "git-upload-pack" && is_post(request.method) {
        return GitHttpRoute::UploadPack { repo };
    }
    if rest.len() == 1 && rest[0] == "git-receive-pack" && is_post(request.method) {
        return GitHttpRoute::ReceivePack { repo };
    }
    GitHttpRoute::NotFound
}

fn split_repo_segments(path: &str) -> Option<(String, String, Vec<String>)> {
    let trimmed = path.trim_start_matches('/');
    let mut segments = Vec::new();
    for segment in trimmed.split('/') {
        if !segment.is_empty() {
            segments.push(segment);
        }
    }
    let mut parts = segments.into_iter();
    let npub = parts.next()?.to_string();
    let repo = parts.next()?.to_string();
    let mut rest = Vec::new();
    for segment in parts {
        rest.push(segment.to_string());
    }
    if rest.is_empty() {
        return None;
    }
    Some((npub, repo, rest))
}

fn normalize_repo_path(
    npub: &str,
    repo_segment: &str,
) -> Result<NormalizedRepo, GitHttpRouteError> {
    let path = Path::new("/").join(npub).join(repo_segment);
    let parsed = match gittree_core::parse_repo_path(&path) {
        Ok(parsed) => parsed,
        Err(err) => return Err(GitHttpRouteError::InvalidRepo(err.to_string())),
    };
    Ok(NormalizedRepo {
        canonical_path: format!("/{}/{}.git", parsed.npub, parsed.identifier),
        identifier: parsed.identifier,
        npub: parsed.npub,
        pubkey: parsed.pubkey,
    })
}

fn parse_service(query: Option<&str>) -> Result<GitHttpService, GitHttpRouteError> {
    let query = query.ok_or(GitHttpRouteError::MissingService)?;
    for pair in query.split('&') {
        let mut parts = pair.splitn(2, '=');
        if parts.next() != Some("service") {
            continue;
        }
        let value = parts.next().unwrap_or("");
        return match value {
            "git-upload-pack" => Ok(GitHttpService::UploadPack),
            "git-receive-pack" => Ok(GitHttpService::ReceivePack),
            _ => Err(GitHttpRouteError::InvalidService(value.to_string())),
        };
    }
    Err(GitHttpRouteError::MissingService)
}

fn is_get(method: &str) -> bool {
    method.eq_ignore_ascii_case("GET")
}

fn is_post(method: &str) -> bool {
    method.eq_ignore_ascii_case("POST")
}

#[derive(Debug)]
pub enum GitHttpRouteError {
    InvalidRepo(String),
    MissingService,
    InvalidService(String),
}

#[cfg(test)]
mod tests {
    use super::AUTH_HEADER;
    use super::AuthConfig;
    use super::BASE64_STANDARD;
    use super::ENV_STORAGE_IDLE_TIMEOUT_SECS;
    use super::ENV_STORAGE_MAX_CONNECTIONS;
    use super::ENV_STORAGE_MAX_LIFETIME_SECS;
    use super::ENV_STORAGE_MIN_CONNECTIONS;
    use super::ENV_STORAGE_READ_URL;
    use super::ENV_TIMEOUT_SECS;
    use super::ENV_UPSTREAM_URL;
    use super::GitHttpAppState;
    use super::GitHttpConfig;
    use super::GitHttpConfigError;
    use super::GitHttpError;
    use super::GitHttpMetrics;
    use super::GitHttpRequest;
    use super::GitHttpRoute;
    use super::GitHttpRouter;
    use super::GitHttpService;
    use super::ObservabilityHandle;
    use super::ReqwestUpstreamClient;
    use super::StorageConfigError;
    use super::UpstreamClient;
    use super::UpstreamError;
    use super::UpstreamRequest;
    use super::UpstreamResponse;
    use super::init_observability;
    use super::payload_hash;
    use super::route_request;
    use async_trait::async_trait;
    use axum::Router;
    use axum::body::{Body, Bytes, HttpBody, to_bytes};
    use axum::http::{HeaderMap, Method, Request, StatusCode};
    use axum::response::IntoResponse;
    use axum::routing::get;
    use base64::Engine;
    use gittree_config::ConfigError;
    use gittree_core::{RepoAnnouncement, RepoMapping};
    use gittree_nostr_auth::{NIP98_KIND, Nip98Event};
    use gittree_observability::{ObservabilityConfigError, ObservabilityError};
    use gittree_storage::{
        AnnouncementRepository, InMemoryRepositories, RepoAnnouncementRecord, RepoMappingRecord,
        RepoMappingRepository, StorageError,
    };
    use secp256k1::{Keypair, Message, Secp256k1, SecretKey, XOnlyPublicKey};
    use serde_json::json;
    use sha2::Digest;
    use std::error::Error;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::path::Path;
    use std::pin::Pin;
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::sync::OnceLock;
    use std::task::{Context, Poll};
    use std::time::Duration;
    use tower::ServiceExt;

    static ENV_LOCK: Mutex<()> = Mutex::new(());
    static OBSERVABILITY: OnceLock<ObservabilityHandle> = OnceLock::new();

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

    fn with_env_value(key: &str, value: Option<&str>, f: &mut dyn FnMut()) {
        let previous = std::env::var_os(key);
        match value {
            Some(value) => unsafe { std::env::set_var(key, value) },
            None => unsafe { std::env::remove_var(key) },
        }
        f();
        match previous {
            Some(old) => unsafe { std::env::set_var(key, old) },
            None => unsafe { std::env::remove_var(key) },
        }
    }

    async fn noop_server(
        _listener: tokio::net::TcpListener,
        _router: Router,
    ) -> Result<(), std::io::Error> {
        Ok(())
    }

    fn init_ok_handle() -> Result<(), GitHttpError> {
        Ok(())
    }

    async fn fail_server(
        _listener: tokio::net::TcpListener,
        _router: Router,
    ) -> Result<(), std::io::Error> {
        Err(std::io::Error::other("boom"))
    }

    fn init_observability_for_test() -> ObservabilityHandle {
        init_observability().expect("init")
    }

    fn start_mock_http_server(
        status: &str,
        content_type: &str,
        body: &str,
    ) -> (String, std::thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        let status = status.to_string();
        let content_type = content_type.to_string();
        let body = body.to_string();
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut request = [0u8; 1024];
            let _ = stream.read(&mut request);
            let response = format!(
                "HTTP/1.1 {status}\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.flush();
        });
        (format!("http://{addr}"), handle)
    }

    fn start_raw_http_server(response: &[u8]) -> (String, std::thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        let response = response.to_vec();
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut request = [0u8; 1024];
            let _ = stream.read(&mut request);
            let _ = stream.write_all(&response);
            let _ = stream.flush();
        });
        (format!("http://{addr}"), handle)
    }

    fn route_kind(route: &GitHttpRoute) -> &'static str {
        match route {
            GitHttpRoute::InfoRefs { .. } => "info_refs",
            GitHttpRoute::UploadPack { .. } => "upload_pack",
            GitHttpRoute::ReceivePack { .. } => "receive_pack",
            GitHttpRoute::NotFound => "not_found",
        }
    }

    fn info_refs_service(route: &GitHttpRoute) -> Option<GitHttpService> {
        match route {
            GitHttpRoute::InfoRefs { service, .. } => Some(service.clone()),
            _ => None,
        }
    }

    fn config_error_label(err: &GitHttpConfigError) -> &'static str {
        match err {
            GitHttpConfigError::Config(_) => "config",
            GitHttpConfigError::MissingEnv(_) => "missing_env",
            GitHttpConfigError::InvalidEnv { .. } => "invalid_env",
            GitHttpConfigError::Storage(_) => "storage",
        }
    }

    fn git_http_error_label(err: &GitHttpError) -> &'static str {
        match err {
            GitHttpError::Config(_) => "config",
            GitHttpError::ObservabilityConfig(_) => "observability_config",
            GitHttpError::Observability(_) => "observability",
            GitHttpError::Storage(_) => "storage",
            GitHttpError::Upstream(_) => "upstream",
            GitHttpError::Serve(_) => "serve",
        }
    }

    fn upstream_error_label(err: &UpstreamError) -> &'static str {
        match err {
            UpstreamError::Request(_) => "request",
        }
    }

    #[test]
    fn helper_label_functions_cover_all_variants() {
        assert_eq!(route_kind(&GitHttpRoute::NotFound), "not_found");
        assert_eq!(info_refs_service(&GitHttpRoute::NotFound), None);

        let cfg_missing = GitHttpConfigError::MissingEnv("MISSING");
        assert_eq!(config_error_label(&cfg_missing), "missing_env");
        let cfg_config = GitHttpConfigError::Config(ConfigError::MissingEnv("CONFIG_MISSING"));
        assert_eq!(config_error_label(&cfg_config), "config");

        let git_cfg = GitHttpError::Config(GitHttpConfigError::MissingEnv("X"));
        assert_eq!(git_http_error_label(&git_cfg), "config");
        let git_obs = GitHttpError::Observability(ObservabilityError::TraceInit("x".to_string()));
        assert_eq!(git_http_error_label(&git_obs), "observability");
        let git_upstream = GitHttpError::Upstream("upstream".to_string());
        assert_eq!(git_http_error_label(&git_upstream), "upstream");
    }

    #[test]
    fn config_loads_from_env() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var(
            ENV_STORAGE_READ_URL,
            "postgres://user:pass@localhost:5432/gittree",
            &mut || {
                with_env_var(ENV_UPSTREAM_URL, "https://git.example", &mut || {
                    with_env_var("GITTREE_GIT_HTTP_BIND", "127.0.0.1:9090", &mut || {
                        with_env_var(ENV_TIMEOUT_SECS, "15", &mut || {
                            let config = GitHttpConfig::from_env().expect("config");
                            assert_eq!(config.bind, "127.0.0.1:9090");
                            assert_eq!(config.upstream_url, "https://git.example");
                            assert_eq!(config.timeout, Duration::from_secs(15));
                        });
                    });
                });
            },
        );
    }

    #[test]
    fn config_ignores_empty_timeout_override() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var(
            ENV_STORAGE_READ_URL,
            "postgres://user:pass@localhost:5432/gittree",
            &mut || {
                with_env_var(ENV_UPSTREAM_URL, "https://git.example", &mut || {
                    with_env_var(ENV_TIMEOUT_SECS, "", &mut || {
                        let config = GitHttpConfig::from_env().expect("config");
                        assert_eq!(
                            config.timeout,
                            Duration::from_secs(super::DEFAULT_TIMEOUT_SECS)
                        );
                    });
                });
            },
        );
    }

    #[test]
    fn config_rejects_invalid_upstream_url() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var(
            ENV_STORAGE_READ_URL,
            "postgres://user:pass@localhost:5432/gittree",
            &mut || {
                with_env_var(ENV_UPSTREAM_URL, "not-a-url", &mut || {
                    let err = GitHttpConfig::from_env().expect_err("invalid upstream");
                    assert_eq!(config_error_label(&err), "invalid_env");
                });
            },
        );
    }

    #[test]
    fn config_rejects_invalid_timeout_value() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var(
            ENV_STORAGE_READ_URL,
            "postgres://user:pass@localhost:5432/gittree",
            &mut || {
                with_env_var(ENV_UPSTREAM_URL, "https://git.example", &mut || {
                    with_env_var(ENV_TIMEOUT_SECS, "bad-timeout", &mut || {
                        let err = GitHttpConfig::from_env().expect_err("invalid timeout");
                        assert_eq!(config_error_label(&err), "invalid_env");
                    });
                });
            },
        );
    }

    #[test]
    fn config_requires_upstream_url() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var(
            ENV_STORAGE_READ_URL,
            "postgres://user:pass@localhost:5432/gittree",
            &mut || {
                with_env_value(ENV_UPSTREAM_URL, None, &mut || {
                    let err = GitHttpConfig::from_env().expect_err("missing upstream");
                    assert!(matches!(
                        err,
                        GitHttpConfigError::MissingEnv(ENV_UPSTREAM_URL)
                    ));
                });
            },
        );
    }

    #[test]
    fn config_maps_auth_config_errors() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var(
            ENV_STORAGE_READ_URL,
            "postgres://user:pass@localhost:5432/gittree",
            &mut || {
                with_env_var(ENV_UPSTREAM_URL, "https://git.example", &mut || {
                    with_env_var("GITTREE_AUTH_MAX_SKEW_SECONDS", "0", &mut || {
                        let err = GitHttpConfig::from_env().expect_err("auth config error");
                        assert_eq!(config_error_label(&err), "config");
                    });
                });
            },
        );
    }

    #[test]
    fn config_rejects_invalid_storage_numeric_values() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var(
            ENV_STORAGE_READ_URL,
            "postgres://user:pass@localhost:5432/gittree",
            &mut || {
                with_env_var(ENV_UPSTREAM_URL, "https://git.example", &mut || {
                    with_env_var(ENV_STORAGE_MAX_CONNECTIONS, "oops", &mut || {
                        let err = GitHttpConfig::from_env().expect_err("invalid storage value");
                        assert_eq!(config_error_label(&err), "storage");
                    });
                });
            },
        );
    }

    #[test]
    fn config_rejects_invalid_storage_min_connections_and_max_lifetime_values() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var(
            ENV_STORAGE_READ_URL,
            "postgres://user:pass@localhost:5432/gittree",
            &mut || {
                with_env_var(ENV_UPSTREAM_URL, "https://git.example", &mut || {
                    with_env_var(ENV_STORAGE_MIN_CONNECTIONS, "oops", &mut || {
                        let err = GitHttpConfig::from_env().expect_err("invalid min connections");
                        assert_eq!(config_error_label(&err), "storage");
                    });
                });
            },
        );

        with_env_var(
            ENV_STORAGE_READ_URL,
            "postgres://user:pass@localhost:5432/gittree",
            &mut || {
                with_env_var(ENV_UPSTREAM_URL, "https://git.example", &mut || {
                    with_env_var(ENV_STORAGE_MAX_LIFETIME_SECS, "oops", &mut || {
                        let err = GitHttpConfig::from_env().expect_err("invalid max lifetime");
                        assert_eq!(config_error_label(&err), "storage");
                    });
                });
            },
        );
    }

    #[test]
    fn config_rejects_invalid_storage_timeout_values() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var(
            ENV_STORAGE_READ_URL,
            "postgres://user:pass@localhost:5432/gittree",
            &mut || {
                with_env_var(ENV_UPSTREAM_URL, "https://git.example", &mut || {
                    with_env_var(ENV_STORAGE_IDLE_TIMEOUT_SECS, "oops", &mut || {
                        let err = GitHttpConfig::from_env().expect_err("invalid storage timeout");
                        assert_eq!(config_error_label(&err), "storage");
                    });
                });
            },
        );
    }

    #[test]
    fn config_rejects_invalid_storage_bounds() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var(
            ENV_STORAGE_READ_URL,
            "postgres://user:pass@localhost:5432/gittree",
            &mut || {
                with_env_var(ENV_UPSTREAM_URL, "https://git.example", &mut || {
                    with_env_var(ENV_STORAGE_MAX_CONNECTIONS, "1", &mut || {
                        with_env_var(ENV_STORAGE_MIN_CONNECTIONS, "2", &mut || {
                            let err = GitHttpConfig::from_env().expect_err("invalid bounds");
                            assert_eq!(config_error_label(&err), "storage");
                        });
                    });
                });
            },
        );
    }

    #[test]
    fn config_requires_storage_read_url() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_value(ENV_STORAGE_READ_URL, None, &mut || {
            with_env_var(ENV_UPSTREAM_URL, "https://git.example", &mut || {
                let err = GitHttpConfig::from_env().expect_err("missing read url");
                assert_eq!(config_error_label(&err), "storage");
            });
        });
    }

    #[test]
    fn config_handles_empty_storage_optional_env_values() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var(
            ENV_STORAGE_READ_URL,
            "postgres://user:pass@localhost:5432/gittree",
            &mut || {
                with_env_var(ENV_UPSTREAM_URL, "https://git.example", &mut || {
                    with_env_var(ENV_STORAGE_MAX_CONNECTIONS, "", &mut || {
                        with_env_var(ENV_STORAGE_MIN_CONNECTIONS, "", &mut || {
                            with_env_var(ENV_STORAGE_IDLE_TIMEOUT_SECS, "", &mut || {
                                with_env_var(ENV_STORAGE_MAX_LIFETIME_SECS, "", &mut || {
                                    let config = GitHttpConfig::from_env().expect("config");
                                    assert_eq!(config.storage.max_connections, 10);
                                    assert_eq!(config.storage.min_connections, 2);
                                    assert_eq!(config.storage.idle_timeout_secs, None);
                                    assert_eq!(config.storage.max_lifetime_secs, None);
                                });
                            });
                        });
                    });
                });
            },
        );
    }

    #[test]
    fn route_request_handles_info_refs() {
        let request = GitHttpRequest::new(
            "GET",
            "/npub1gjttreegkzys8jlhdnfm3qe39h2gka79cpndd0jsms5fk7tuhcnsdw56jq/repo.git/info/refs",
            Some("service=git-upload-pack"),
        );
        let route = route_request(&request);
        assert_eq!(route_kind(&route), "info_refs");
        assert_eq!(info_refs_service(&route), Some(GitHttpService::UploadPack));
    }

    #[test]
    fn route_request_handles_receive_pack() {
        let request = GitHttpRequest::new(
            "POST",
            "/npub1gjttreegkzys8jlhdnfm3qe39h2gka79cpndd0jsms5fk7tuhcnsdw56jq/repo.git/git-receive-pack",
            None,
        );
        let route = route_request(&request);
        assert_eq!(route_kind(&route), "receive_pack");
    }

    #[test]
    fn route_request_rejects_missing_git_suffix() {
        let request = GitHttpRequest::new(
            "GET",
            "/npub1gjttreegkzys8jlhdnfm3qe39h2gka79cpndd0jsms5fk7tuhcnsdw56jq/repo/info/refs",
            Some("service=git-upload-pack"),
        );
        let route = route_request(&request);
        assert_eq!(route_kind(&route), "not_found");
    }

    #[test]
    fn route_request_rejects_missing_service_param() {
        let request = GitHttpRequest::new(
            "GET",
            "/npub1gjttreegkzys8jlhdnfm3qe39h2gka79cpndd0jsms5fk7tuhcnsdw56jq/repo.git/info/refs",
            None,
        );
        assert_eq!(route_kind(&route_request(&request)), "not_found");
    }

    #[test]
    fn route_request_rejects_query_without_service_pair() {
        let request = GitHttpRequest::new(
            "GET",
            "/npub1gjttreegkzys8jlhdnfm3qe39h2gka79cpndd0jsms5fk7tuhcnsdw56jq/repo.git/info/refs",
            Some("foo=bar"),
        );
        assert_eq!(route_kind(&route_request(&request)), "not_found");
    }

    #[test]
    fn route_request_rejects_invalid_service_param() {
        let request = GitHttpRequest::new(
            "GET",
            "/npub1gjttreegkzys8jlhdnfm3qe39h2gka79cpndd0jsms5fk7tuhcnsdw56jq/repo.git/info/refs",
            Some("service=git-bad"),
        );
        assert_eq!(route_kind(&route_request(&request)), "not_found");
    }

    #[test]
    fn route_request_rejects_wrong_method_for_receive_pack() {
        let request = GitHttpRequest::new(
            "GET",
            "/npub1gjttreegkzys8jlhdnfm3qe39h2gka79cpndd0jsms5fk7tuhcnsdw56jq/repo.git/git-receive-pack",
            None,
        );
        assert_eq!(route_kind(&route_request(&request)), "not_found");
    }

    #[test]
    fn route_request_rejects_invalid_npub_segment() {
        let request = GitHttpRequest::new(
            "GET",
            "/not-npub/repo.git/info/refs",
            Some("service=git-upload-pack"),
        );
        assert_eq!(route_kind(&route_request(&request)), "not_found");
    }

    #[test]
    fn route_request_rejects_empty_path_segments() {
        let request = GitHttpRequest::new("GET", "/", Some("service=git-upload-pack"));
        assert_eq!(route_kind(&route_request(&request)), "not_found");
    }

    #[test]
    fn route_request_handles_info_refs_receive_pack_service() {
        let request = GitHttpRequest::new(
            "GET",
            "/npub1gjttreegkzys8jlhdnfm3qe39h2gka79cpndd0jsms5fk7tuhcnsdw56jq/repo.git/info/refs",
            Some("service=git-receive-pack"),
        );
        let route = route_request(&request);
        assert_eq!(route_kind(&route), "info_refs");
        assert_eq!(info_refs_service(&route), Some(GitHttpService::ReceivePack));
    }

    #[test]
    fn router_and_route_helpers_cover_all_labels_and_edge_paths() {
        let router = GitHttpRouter::new();
        let request = GitHttpRequest::new(
            "POST",
            "/npub1gjttreegkzys8jlhdnfm3qe39h2gka79cpndd0jsms5fk7tuhcnsdw56jq/repo.git/git-upload-pack",
            None,
        );
        let upload_route = router.route(&request);
        assert_eq!(route_kind(&upload_route), "upload_pack");
        assert_eq!(super::route_label(&upload_route), "upload_pack");

        let receive_request = GitHttpRequest::new(
            "POST",
            "/npub1gjttreegkzys8jlhdnfm3qe39h2gka79cpndd0jsms5fk7tuhcnsdw56jq/repo.git/git-receive-pack",
            None,
        );
        let receive_route = router.route(&receive_request);
        assert_eq!(super::route_label(&receive_route), "receive_pack");

        let info_request = GitHttpRequest::new(
            "GET",
            "/npub1gjttreegkzys8jlhdnfm3qe39h2gka79cpndd0jsms5fk7tuhcnsdw56jq/repo.git/info/refs",
            Some("other=1&service=git-upload-pack"),
        );
        let info_route = router.route(&info_request);
        assert_eq!(super::route_label(&info_route), "info_refs");

        let not_found_request = GitHttpRequest::new(
            "GET",
            "/npub1gjttreegkzys8jlhdnfm3qe39h2gka79cpndd0jsms5fk7tuhcnsdw56jq/repo.git",
            None,
        );
        let not_found_route = router.route(&not_found_request);
        assert_eq!(route_kind(&not_found_route), "not_found");
        assert_eq!(super::route_label(&not_found_route), "not_found");
    }

    fn test_auth() -> AuthConfig {
        AuthConfig {
            email_domain: "example.com".to_string(),
            max_skew_seconds: 60,
        }
    }

    fn postgres_test_config() -> GitHttpConfig {
        GitHttpConfig {
            bind: "127.0.0.1:0".to_string(),
            upstream_url: "https://git.example".to_string(),
            timeout: Duration::from_secs(1),
            auth: test_auth(),
            storage: super::StorageConfig {
                read_connection: "postgres://gittree:gittree@127.0.0.1:5432/gittree".to_string(),
                write_connection: None,
                max_connections: 10,
                min_connections: 2,
                idle_timeout_secs: None,
                max_lifetime_secs: None,
                application_name: Some("gittree-git-http-test".to_string()),
            },
        }
    }

    fn postgres_state() -> GitHttpAppState<super::PostgresRepositories, ReqwestUpstreamClient> {
        let config = postgres_test_config();
        let repositories = Arc::new(super::build_repositories(&config).expect("repositories"));
        let upstream =
            Arc::new(ReqwestUpstreamClient::new(Duration::from_secs(1)).expect("client"));
        GitHttpAppState {
            auth: config.auth,
            repositories,
            upstream,
            metrics: Arc::new(GitHttpMetrics::new()),
            upstream_url: config.upstream_url,
        }
    }

    fn unavailable_postgres_repositories() -> super::PostgresRepositories {
        let mut config = postgres_test_config();
        config.storage.read_connection =
            "postgres://gittree:gittree@127.0.0.1:1/gittree".to_string();
        super::build_repositories(&config).expect("repositories")
    }

    fn sample_normalized_repo(pubkey: &str) -> super::NormalizedRepo {
        super::NormalizedRepo {
            npub: "npub1test".to_string(),
            identifier: "repo".to_string(),
            canonical_path: "/npub1test/repo.git".to_string(),
            pubkey: pubkey.to_string(),
        }
    }

    fn signed_event(url: &str, method: &str, body: &Bytes, created_at: i64) -> Nip98Event {
        let secp = Secp256k1::new();
        let secret_key = SecretKey::from_slice(&[4u8; 32]).expect("secret");
        let keypair = Keypair::from_secret_key(&secp, &secret_key);
        let (pubkey, _) = XOnlyPublicKey::from_keypair(&keypair);
        let pubkey_hex = hex::encode(pubkey.serialize());
        let mut tags = vec![
            vec!["u".to_string(), url.to_string()],
            vec!["method".to_string(), method.to_string()],
        ];
        if let Some(hash) = payload_hash(body) {
            tags.push(vec!["payload".to_string(), hash]);
        }
        let mut event = Nip98Event {
            id: String::new(),
            pubkey: pubkey_hex,
            created_at,
            kind: NIP98_KIND,
            tags,
            content: String::new(),
            sig: String::new(),
        };
        let event_id = build_event_id(&event);
        let sig = sign_event_id(&event_id, &keypair, &secp);
        event.id = event_id;
        event.sig = sig;
        event
    }

    fn build_event_id(event: &Nip98Event) -> String {
        let payload = json!([
            0,
            event.pubkey,
            event.created_at,
            event.kind,
            event.tags,
            event.content
        ]);
        let serialized = serde_json::to_string(&payload).expect("serialize");
        let mut hasher = sha2::Sha256::new();
        hasher.update(serialized.as_bytes());
        let digest = hasher.finalize();
        hex::encode(digest)
    }

    fn sign_event_id(
        event_id: &str,
        keypair: &Keypair,
        secp: &Secp256k1<secp256k1::All>,
    ) -> String {
        let bytes = hex::decode(event_id).expect("decode");
        let msg = Message::from_digest_slice(&bytes).expect("msg");
        let sig = secp.sign_schnorr_no_aux_rand(&msg, keypair);
        hex::encode(sig.as_ref())
    }

    struct MockUpstreamClient {
        calls: Mutex<Vec<UpstreamRequest>>,
        response: Option<UpstreamResponse>,
        error: Option<String>,
    }

    impl MockUpstreamClient {
        fn new(response: UpstreamResponse) -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
                response: Some(response),
                error: None,
            }
        }

        fn with_error(message: &str) -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
                response: None,
                error: Some(message.to_string()),
            }
        }
    }

    #[async_trait]
    impl UpstreamClient for MockUpstreamClient {
        async fn send(&self, request: UpstreamRequest) -> Result<UpstreamResponse, UpstreamError> {
            let mut calls = self.calls.lock().expect("calls");
            calls.push(request);
            if let Some(message) = &self.error {
                return Err(UpstreamError::Request(message.clone()));
            }
            Ok(self.response.clone().expect("mock response"))
        }
    }

    struct FailingBody;

    impl HttpBody for FailingBody {
        type Data = Bytes;
        type Error = std::io::Error;

        fn poll_frame(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
        ) -> Poll<Option<Result<http_body::Frame<Self::Data>, Self::Error>>> {
            Poll::Ready(Some(Err(std::io::Error::other("body read failed"))))
        }
    }

    #[tokio::test]
    async fn health_endpoint_returns_ok() {
        let repositories = Arc::new(InMemoryRepositories::new());
        let upstream = Arc::new(MockUpstreamClient::new(UpstreamResponse {
            status: StatusCode::OK,
            headers: HeaderMap::new(),
            body: Bytes::from_static(b"ok"),
        }));
        let app = super::build_router(GitHttpAppState {
            auth: test_auth(),
            repositories,
            upstream,
            metrics: Arc::new(GitHttpMetrics::new()),
            upstream_url: "https://git.example".to_string(),
        });

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
    async fn proxy_forwards_to_upstream() {
        let repositories = Arc::new(InMemoryRepositories::new());
        let npub = "npub1gjttreegkzys8jlhdnfm3qe39h2gka79cpndd0jsms5fk7tuhcnsdw56jq";
        let parsed = gittree_core::parse_repo_path(Path::new("/").join(npub).join("repo.git"))
            .expect("parse");
        let mapping = RepoMapping::new("owner", "repo", parsed.pubkey, "repo").expect("mapping");
        let record = RepoMappingRecord::new(&mapping).expect("record");
        repositories.upsert_mapping(record).await.expect("mapping");

        let upstream = Arc::new(MockUpstreamClient::new(UpstreamResponse {
            status: StatusCode::OK,
            headers: HeaderMap::new(),
            body: Bytes::from_static(b"upstream"),
        }));
        let app = super::build_router(GitHttpAppState {
            auth: test_auth(),
            repositories: Arc::clone(&repositories),
            upstream: Arc::clone(&upstream),
            metrics: Arc::new(GitHttpMetrics::new()),
            upstream_url: "https://git.example".to_string(),
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!(
                        "/{npub}/repo.git/info/refs?service=git-upload-pack"
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        assert_eq!(body, Bytes::from_static(b"upstream"));

        let calls = upstream.calls.lock().expect("calls");
        assert_eq!(calls.len(), 1);
        assert_eq!(
            calls[0].url,
            "https://git.example/owner/repo.git/info/refs?service=git-upload-pack"
        );
    }

    #[tokio::test]
    async fn proxy_upload_pack_forwards_to_upstream() {
        let repositories = Arc::new(InMemoryRepositories::new());
        let npub = "npub1gjttreegkzys8jlhdnfm3qe39h2gka79cpndd0jsms5fk7tuhcnsdw56jq";
        let parsed = gittree_core::parse_repo_path(Path::new("/").join(npub).join("repo.git"))
            .expect("parse");
        let mapping = RepoMapping::new("owner", "repo", parsed.pubkey, "repo").expect("mapping");
        let record = RepoMappingRecord::new(&mapping).expect("record");
        repositories.upsert_mapping(record).await.expect("mapping");

        let upstream = Arc::new(MockUpstreamClient::new(UpstreamResponse {
            status: StatusCode::OK,
            headers: HeaderMap::new(),
            body: Bytes::from_static(b"upstream"),
        }));
        let app = super::build_router(GitHttpAppState {
            auth: test_auth(),
            repositories,
            upstream: Arc::clone(&upstream),
            metrics: Arc::new(GitHttpMetrics::new()),
            upstream_url: "https://git.example".to_string(),
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/{npub}/repo.git/git-upload-pack"))
                    .body(Body::from(Bytes::from_static(b"pkt-line")))
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::OK);

        let calls = upstream.calls.lock().expect("calls");
        assert_eq!(calls.len(), 1);
        assert_eq!(
            calls[0].url,
            "https://git.example/owner/repo.git/git-upload-pack"
        );
    }

    #[tokio::test]
    async fn proxy_returns_not_found_for_unmatched_route() {
        let app = super::build_router(GitHttpAppState {
            auth: test_auth(),
            repositories: Arc::new(InMemoryRepositories::new()),
            upstream: Arc::new(MockUpstreamClient::new(UpstreamResponse {
                status: StatusCode::OK,
                headers: HeaderMap::new(),
                body: Bytes::new(),
            })),
            metrics: Arc::new(GitHttpMetrics::new()),
            upstream_url: "https://git.example".to_string(),
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/bad/path")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn resolve_maintainers_handles_duplicate_queue_entries() {
        let repositories = InMemoryRepositories::new();
        let repo_npub = "npub1gjttreegkzys8jlhdnfm3qe39h2gka79cpndd0jsms5fk7tuhcnsdw56jq";
        let maintainer_pubkey = "58e318557257f2ab58a415d21bb57082b4824cf667a1d64e72bcbc5acc018c62";
        let repo_path = Path::new("/").join(repo_npub).join("repo.git");
        let parsed = gittree_core::parse_repo_path(&repo_path).expect("repo parse");

        let announcement = RepoAnnouncement {
            identifier: "repo".to_string(),
            name: None,
            description: None,
            root_commit: None,
            clone: vec!["https://git.example/repo.git".to_string()],
            web: Vec::new(),
            relays: vec!["wss://relay.example".to_string()],
            blossoms: Vec::new(),
            hashtags: Vec::new(),
            maintainers: vec![maintainer_pubkey.to_string(), maintainer_pubkey.to_string()],
        };
        let announcement_record =
            RepoAnnouncementRecord::new(&"aa".repeat(32), &parsed.pubkey, 1, &announcement)
                .expect("announcement");
        repositories
            .insert_announcement(announcement_record)
            .await
            .expect("insert announcement");

        let repo = super::normalize_repo_path(repo_npub, "repo.git").expect("normalized");
        let maintainers = super::resolve_maintainers(&repositories, &repo)
            .await
            .expect("maintainers");
        assert!(maintainers.contains(&parsed.pubkey));
        assert!(maintainers.contains(&maintainer_pubkey.to_string()));
    }

    #[tokio::test]
    async fn resolve_maintainers_enqueues_unique_maintainers() {
        let repositories = InMemoryRepositories::new();
        let repo_npub = "npub1gjttreegkzys8jlhdnfm3qe39h2gka79cpndd0jsms5fk7tuhcnsdw56jq";
        let maintainer_pubkey = "466d7fcae563e5cb09a0d1870bb580344804617879a14949cf22285f1bae3f27";
        let repo_path = Path::new("/").join(repo_npub).join("repo.git");
        let parsed = gittree_core::parse_repo_path(&repo_path).expect("repo parse");

        let announcement = RepoAnnouncement {
            identifier: "repo".to_string(),
            name: None,
            description: None,
            root_commit: None,
            clone: vec!["https://git.example/repo.git".to_string()],
            web: Vec::new(),
            relays: vec!["wss://relay.example".to_string()],
            blossoms: Vec::new(),
            hashtags: Vec::new(),
            maintainers: vec![maintainer_pubkey.to_string()],
        };
        let announcement_record =
            RepoAnnouncementRecord::new(&"cc".repeat(32), &parsed.pubkey, 1, &announcement)
                .expect("announcement");
        repositories
            .insert_announcement(announcement_record)
            .await
            .expect("insert root announcement");

        let maintainer_announcement = RepoAnnouncement {
            identifier: "repo".to_string(),
            name: None,
            description: None,
            root_commit: None,
            clone: vec!["https://git.example/repo.git".to_string()],
            web: Vec::new(),
            relays: vec!["wss://relay.example".to_string()],
            blossoms: Vec::new(),
            hashtags: Vec::new(),
            maintainers: Vec::new(),
        };
        let maintainer_record = RepoAnnouncementRecord::new(
            &"dd".repeat(32),
            maintainer_pubkey,
            2,
            &maintainer_announcement,
        )
        .expect("maintainer announcement");
        repositories
            .insert_announcement(maintainer_record)
            .await
            .expect("insert maintainer announcement");

        let repo = super::normalize_repo_path(repo_npub, "repo.git").expect("normalized");
        let maintainers = super::resolve_maintainers(&repositories, &repo)
            .await
            .expect("maintainers");
        assert!(maintainers.contains(&parsed.pubkey));
        assert!(maintainers.contains(&maintainer_pubkey.to_string()));
    }

    #[test]
    fn env_helpers_restore_preexisting_values() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        let key = "GITTREE_GIT_HTTP_ENV_HELPER";
        let missing_key = "GITTREE_GIT_HTTP_ENV_HELPER_MISSING";
        // SAFETY: protected by ENV_LOCK and restored below.
        unsafe {
            std::env::set_var(key, "before");
            std::env::remove_var(missing_key);
        }
        with_env_var(key, "during", &mut || {
            assert_eq!(std::env::var(key).ok().as_deref(), Some("during"));
        });
        assert_eq!(std::env::var(key).ok().as_deref(), Some("before"));

        with_env_value(key, Some("value"), &mut || {
            assert_eq!(std::env::var(key).ok().as_deref(), Some("value"));
        });
        assert_eq!(std::env::var(key).ok().as_deref(), Some("before"));

        with_env_var(missing_key, "during", &mut || {
            assert_eq!(std::env::var(missing_key).ok().as_deref(), Some("during"));
        });
        assert!(std::env::var(missing_key).is_err());

        with_env_value(missing_key, Some("value"), &mut || {
            assert_eq!(std::env::var(missing_key).ok().as_deref(), Some("value"));
        });
        assert!(std::env::var(missing_key).is_err());
        // SAFETY: clean test env key.
        unsafe {
            std::env::remove_var(key);
            std::env::remove_var(missing_key);
        }
    }

    #[test]
    fn env_parsers_return_none_for_missing_values() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        // SAFETY: protected by ENV_LOCK and restored in this test.
        unsafe {
            std::env::remove_var(ENV_TIMEOUT_SECS);
            std::env::remove_var(ENV_STORAGE_MAX_CONNECTIONS);
            std::env::remove_var(ENV_STORAGE_IDLE_TIMEOUT_SECS);
        }
        assert_eq!(super::env_u64(ENV_TIMEOUT_SECS).expect("timeout"), None);
        assert_eq!(
            super::storage_env_u32(ENV_STORAGE_MAX_CONNECTIONS).expect("max connections"),
            None
        );
        assert_eq!(
            super::storage_env_u64(ENV_STORAGE_IDLE_TIMEOUT_SECS).expect("idle timeout"),
            None
        );
    }

    #[test]
    fn env_parsers_return_none_for_unique_missing_key_without_env_mutation() {
        let missing_key: &'static str = Box::leak(
            (0..)
                .map(|index| format!("GITTREE_GIT_HTTP_MISSING_KEY_{index}"))
                .find(|key| std::env::var_os(key).is_none())
                .expect("find missing env key")
                .into_boxed_str(),
        );
        assert_eq!(super::env_u64(missing_key).expect("timeout"), None);
        assert_eq!(super::storage_env_u32(missing_key).expect("u32"), None);
        assert_eq!(super::storage_env_u64(missing_key).expect("u64"), None);
    }

    #[test]
    fn env_parsers_parse_numeric_values() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        // SAFETY: protected by ENV_LOCK and restored in this test.
        unsafe {
            std::env::set_var(ENV_TIMEOUT_SECS, "15");
            std::env::set_var(ENV_STORAGE_MAX_CONNECTIONS, "42");
            std::env::set_var(ENV_STORAGE_IDLE_TIMEOUT_SECS, "90");
            std::env::set_var(ENV_STORAGE_MAX_LIFETIME_SECS, "120");
        }
        assert_eq!(super::env_u64(ENV_TIMEOUT_SECS).expect("timeout"), Some(15));
        assert_eq!(
            super::storage_env_u32(ENV_STORAGE_MAX_CONNECTIONS).expect("max connections"),
            Some(42)
        );
        assert_eq!(
            super::storage_env_u64(ENV_STORAGE_IDLE_TIMEOUT_SECS).expect("idle timeout"),
            Some(90)
        );
        assert_eq!(
            super::storage_env_u64(ENV_STORAGE_MAX_LIFETIME_SECS).expect("max lifetime"),
            Some(120)
        );
        // SAFETY: test cleanup for unique keys above.
        unsafe {
            std::env::remove_var(ENV_TIMEOUT_SECS);
            std::env::remove_var(ENV_STORAGE_MAX_CONNECTIONS);
            std::env::remove_var(ENV_STORAGE_IDLE_TIMEOUT_SECS);
            std::env::remove_var(ENV_STORAGE_MAX_LIFETIME_SECS);
        }
    }

    #[tokio::test]
    async fn receive_pack_rejects_missing_auth() {
        let repositories = Arc::new(InMemoryRepositories::new());
        let npub = "npub1gjttreegkzys8jlhdnfm3qe39h2gka79cpndd0jsms5fk7tuhcnsdw56jq";
        let parsed = gittree_core::parse_repo_path(Path::new("/").join(npub).join("repo.git"))
            .expect("parse");
        let mapping =
            RepoMapping::new("owner", "repo", parsed.pubkey.clone(), "repo").expect("mapping");
        let record = RepoMappingRecord::new(&mapping).expect("record");
        repositories.upsert_mapping(record).await.expect("mapping");

        let maintainer_pubkey = "11".repeat(32);
        let announcement = RepoAnnouncement {
            identifier: "repo".to_string(),
            name: None,
            description: None,
            root_commit: None,
            clone: vec!["https://git.example/repo.git".to_string()],
            web: Vec::new(),
            relays: vec!["wss://relay.example".to_string()],
            blossoms: Vec::new(),
            hashtags: Vec::new(),
            maintainers: vec![maintainer_pubkey],
        };
        let announcement_record =
            RepoAnnouncementRecord::new(&"aa".repeat(32), &parsed.pubkey, 1, &announcement)
                .expect("announcement");
        repositories
            .insert_announcement(announcement_record)
            .await
            .expect("announcement");

        let upstream = Arc::new(MockUpstreamClient::new(UpstreamResponse {
            status: StatusCode::OK,
            headers: HeaderMap::new(),
            body: Bytes::from_static(b"upstream"),
        }));
        let app = super::build_router(GitHttpAppState {
            auth: test_auth(),
            repositories: Arc::clone(&repositories),
            upstream: Arc::clone(&upstream),
            metrics: Arc::new(GitHttpMetrics::new()),
            upstream_url: "https://git.example".to_string(),
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/{npub}/repo.git/git-receive-pack"))
                    .header("host", "localhost")
                    .body(Body::from(Bytes::from_static(b"payload")))
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let calls = upstream.calls.lock().expect("calls");
        assert!(calls.is_empty());
    }

    #[tokio::test]
    async fn receive_pack_accepts_authorized_pubkey() {
        let repositories = Arc::new(InMemoryRepositories::new());
        let npub = "npub1gjttreegkzys8jlhdnfm3qe39h2gka79cpndd0jsms5fk7tuhcnsdw56jq";
        let parsed = gittree_core::parse_repo_path(Path::new("/").join(npub).join("repo.git"))
            .expect("parse");
        let mapping =
            RepoMapping::new("owner", "repo", parsed.pubkey.clone(), "repo").expect("mapping");
        let record = RepoMappingRecord::new(&mapping).expect("record");
        repositories.upsert_mapping(record).await.expect("mapping");

        let body = Bytes::from_static(b"payload");
        let url = format!("http://localhost/{npub}/repo.git/git-receive-pack");
        let event = signed_event(&url, "POST", &body, super::unix_timestamp());
        let announcement = RepoAnnouncement {
            identifier: "repo".to_string(),
            name: None,
            description: None,
            root_commit: None,
            clone: vec!["https://git.example/repo.git".to_string()],
            web: Vec::new(),
            relays: vec!["wss://relay.example".to_string()],
            blossoms: Vec::new(),
            hashtags: Vec::new(),
            maintainers: vec![event.pubkey.clone()],
        };
        let announcement_record =
            RepoAnnouncementRecord::new(&"aa".repeat(32), &parsed.pubkey, 1, &announcement)
                .expect("announcement");
        repositories
            .insert_announcement(announcement_record)
            .await
            .expect("announcement");

        let upstream = Arc::new(MockUpstreamClient::new(UpstreamResponse {
            status: StatusCode::OK,
            headers: HeaderMap::new(),
            body: Bytes::from_static(b"upstream"),
        }));
        let app = super::build_router(GitHttpAppState {
            auth: test_auth(),
            repositories: Arc::clone(&repositories),
            upstream: Arc::clone(&upstream),
            metrics: Arc::new(GitHttpMetrics::new()),
            upstream_url: "https://git.example".to_string(),
        });

        let token = BASE64_STANDARD.encode(serde_json::to_vec(&event).expect("event json"));
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/{npub}/repo.git/git-receive-pack"))
                    .header("host", "localhost")
                    .header(AUTH_HEADER, format!("Nostr {token}"))
                    .body(Body::from(body.clone()))
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::OK);
        let calls = upstream.calls.lock().expect("calls");
        assert_eq!(calls.len(), 1);
    }

    #[tokio::test]
    async fn receive_pack_rejects_unauthorized_pubkey() {
        let repositories = Arc::new(InMemoryRepositories::new());
        let npub = "npub1gjttreegkzys8jlhdnfm3qe39h2gka79cpndd0jsms5fk7tuhcnsdw56jq";
        let parsed = gittree_core::parse_repo_path(Path::new("/").join(npub).join("repo.git"))
            .expect("parse");
        let mapping =
            RepoMapping::new("owner", "repo", parsed.pubkey.clone(), "repo").expect("mapping");
        let record = RepoMappingRecord::new(&mapping).expect("record");
        repositories.upsert_mapping(record).await.expect("mapping");

        let body = Bytes::from_static(b"payload");
        let url = format!("http://localhost/{npub}/repo.git/git-receive-pack");
        let event = signed_event(&url, "POST", &body, super::unix_timestamp());
        let announcement = RepoAnnouncement {
            identifier: "repo".to_string(),
            name: None,
            description: None,
            root_commit: None,
            clone: vec!["https://git.example/repo.git".to_string()],
            web: Vec::new(),
            relays: vec!["wss://relay.example".to_string()],
            blossoms: Vec::new(),
            hashtags: Vec::new(),
            maintainers: vec!["11".repeat(32)],
        };
        let announcement_record =
            RepoAnnouncementRecord::new(&"aa".repeat(32), &parsed.pubkey, 1, &announcement)
                .expect("announcement");
        repositories
            .insert_announcement(announcement_record)
            .await
            .expect("announcement");

        let upstream = Arc::new(MockUpstreamClient::new(UpstreamResponse {
            status: StatusCode::OK,
            headers: HeaderMap::new(),
            body: Bytes::from_static(b"upstream"),
        }));
        let app = super::build_router(GitHttpAppState {
            auth: test_auth(),
            repositories: Arc::clone(&repositories),
            upstream: Arc::clone(&upstream),
            metrics: Arc::new(GitHttpMetrics::new()),
            upstream_url: "https://git.example".to_string(),
        });

        let token = BASE64_STANDARD.encode(serde_json::to_vec(&event).expect("event json"));
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/{npub}/repo.git/git-receive-pack"))
                    .header("host", "localhost")
                    .header(AUTH_HEADER, format!("Nostr {token}"))
                    .body(Body::from(body.clone()))
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn postgres_handlers_cover_not_found_and_pre_storage_validation_paths() {
        let state = postgres_state();
        let app = super::build_router(state.clone());

        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/missing")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::NOT_FOUND);

        let repo = sample_normalized_repo("not-hex");
        let mapping_error = super::resolve_mapping(state.repositories.as_ref(), &repo)
            .await
            .expect_err("invalid pubkey");
        assert_eq!(
            mapping_error.into_response().status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );

        let maintainer_error = super::resolve_maintainers(state.repositories.as_ref(), &repo)
            .await
            .expect_err("invalid maintainer pubkey");
        assert_eq!(
            maintainer_error.into_response().status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );

        let auth_error = super::authorize_receive_pack(
            &state,
            &sample_normalized_repo(&"11".repeat(32)),
            &HeaderMap::new(),
            &Method::POST,
            &"/npub1test/repo.git/git-receive-pack".parse().expect("uri"),
            &Bytes::from_static(b"payload"),
        )
        .await
        .expect_err("missing auth");
        assert_eq!(
            auth_error.into_response().status(),
            StatusCode::UNAUTHORIZED
        );
    }

    #[tokio::test]
    async fn handle_git_route_returns_internal_when_request_body_fails() {
        let repositories = Arc::new(InMemoryRepositories::new());
        let npub = "npub1gjttreegkzys8jlhdnfm3qe39h2gka79cpndd0jsms5fk7tuhcnsdw56jq";
        let parsed = gittree_core::parse_repo_path(Path::new("/").join(npub).join("repo.git"))
            .expect("parse");
        let mapping =
            RepoMapping::new("owner", "repo", parsed.pubkey.clone(), "repo").expect("mapping");
        repositories
            .upsert_mapping(RepoMappingRecord::new(&mapping).expect("record"))
            .await
            .expect("mapping");
        let upstream = Arc::new(MockUpstreamClient::new(UpstreamResponse {
            status: StatusCode::OK,
            headers: HeaderMap::new(),
            body: Bytes::from_static(b"upstream"),
        }));
        let state = GitHttpAppState {
            auth: test_auth(),
            repositories,
            upstream,
            metrics: Arc::new(GitHttpMetrics::new()),
            upstream_url: "https://git.example".to_string(),
        };
        let route = super::GitHttpRoute::UploadPack {
            repo: super::normalize_repo_path(npub, "repo.git").expect("normalized"),
        };
        let request = Request::builder()
            .method("POST")
            .uri(format!("/{npub}/repo.git/git-upload-pack"))
            .body(Body::new(FailingBody))
            .expect("request");
        let err = super::handle_git_route(&state, &route, request)
            .await
            .expect_err("body read error");
        assert_eq!(
            err.into_response().status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }

    #[tokio::test]
    async fn handle_git_route_returns_bad_gateway_when_upstream_send_fails() {
        let repositories = Arc::new(InMemoryRepositories::new());
        let npub = "npub1gjttreegkzys8jlhdnfm3qe39h2gka79cpndd0jsms5fk7tuhcnsdw56jq";
        let parsed = gittree_core::parse_repo_path(Path::new("/").join(npub).join("repo.git"))
            .expect("parse");
        let mapping =
            RepoMapping::new("owner", "repo", parsed.pubkey.clone(), "repo").expect("mapping");
        repositories
            .upsert_mapping(RepoMappingRecord::new(&mapping).expect("record"))
            .await
            .expect("mapping");
        let state = GitHttpAppState {
            auth: test_auth(),
            repositories,
            upstream: Arc::new(MockUpstreamClient::with_error("upstream failed")),
            metrics: Arc::new(GitHttpMetrics::new()),
            upstream_url: "https://git.example".to_string(),
        };
        let route = super::GitHttpRoute::UploadPack {
            repo: super::normalize_repo_path(npub, "repo.git").expect("normalized"),
        };
        let request = Request::builder()
            .method("POST")
            .uri(format!("/{npub}/repo.git/git-upload-pack"))
            .body(Body::from(Bytes::from_static(b"pkt-line")))
            .expect("request");
        let err = super::handle_git_route(&state, &route, request)
            .await
            .expect_err("upstream error");
        assert_eq!(err.into_response().status(), StatusCode::BAD_GATEWAY);
    }

    #[tokio::test]
    async fn resolve_mapping_maps_storage_errors_and_missing_records() {
        let valid_pubkey = "11".repeat(32);
        let repo = sample_normalized_repo(&valid_pubkey);

        let unavailable = unavailable_postgres_repositories();
        let storage_err = super::resolve_mapping(&unavailable, &repo)
            .await
            .expect_err("storage error");
        assert_eq!(
            storage_err.into_response().status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );

        let missing_err = super::resolve_mapping(&InMemoryRepositories::new(), &repo)
            .await
            .expect_err("missing mapping");
        assert_eq!(missing_err.into_response().status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn authorize_receive_pack_rejects_invalid_nip98_event() {
        let repositories = Arc::new(InMemoryRepositories::new());
        let upstream = Arc::new(MockUpstreamClient::new(UpstreamResponse {
            status: StatusCode::OK,
            headers: HeaderMap::new(),
            body: Bytes::from_static(b"upstream"),
        }));
        let state = GitHttpAppState {
            auth: test_auth(),
            repositories,
            upstream,
            metrics: Arc::new(GitHttpMetrics::new()),
            upstream_url: "https://git.example".to_string(),
        };
        let body = Bytes::from_static(b"payload");
        let uri: axum::http::Uri = "/npub1test/repo.git/git-receive-pack".parse().expect("uri");
        let url = format!("http://localhost{uri}");
        let event = signed_event(&url, "GET", &body, super::unix_timestamp());
        let token = BASE64_STANDARD.encode(serde_json::to_vec(&event).expect("event"));
        let mut headers = HeaderMap::new();
        headers.insert("host", "localhost".parse().expect("host"));
        headers.insert(AUTH_HEADER, format!("Nostr {token}").parse().expect("auth"));
        let err = super::authorize_receive_pack(
            &state,
            &sample_normalized_repo(&"11".repeat(32)),
            &headers,
            &Method::POST,
            &uri,
            &body,
        )
        .await
        .expect_err("invalid nip98");
        assert_eq!(err.into_response().status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn resolve_maintainers_maps_storage_errors() {
        let repo = sample_normalized_repo(&"11".repeat(32));
        let unavailable = unavailable_postgres_repositories();
        let err = super::resolve_maintainers(&unavailable, &repo)
            .await
            .expect_err("storage error");
        assert_eq!(
            err.into_response().status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }

    #[test]
    fn observability_init_returns_registry() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_value("GITTREE_METRICS_ENABLED", None, &mut || {
            let handle = OBSERVABILITY.get_or_init(init_observability_for_test);
            assert!(handle.prometheus_registry().is_some());
        });
    }

    #[test]
    fn observability_init_second_call_reports_error_variant() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_value("GITTREE_METRICS_ENABLED", None, &mut || {
            let _ = OBSERVABILITY.get_or_init(init_observability_for_test);
            let err = init_observability().expect_err("second init should fail");
            assert_eq!(git_http_error_label(&err), "observability");
        });
    }

    #[test]
    fn metrics_record_accepts_requests() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_value("GITTREE_METRICS_ENABLED", None, &mut || {
            let _handle = OBSERVABILITY.get_or_init(init_observability_for_test);
            let metrics = GitHttpMetrics::new();
            let route = GitHttpRoute::NotFound;
            metrics.record(&route, 200, Duration::from_millis(5));
        });
    }

    #[tokio::test]
    async fn upstream_error_and_service_build_paths_are_stable() {
        let upstream = UpstreamError::Request("failed".to_string());
        assert_eq!(upstream.to_string(), "failed");

        let config = GitHttpConfig {
            bind: "127.0.0.1:8085".to_string(),
            upstream_url: "https://git.example".to_string(),
            timeout: Duration::from_secs(1),
            auth: test_auth(),
            storage: super::StorageConfig {
                read_connection: "postgres://user:pass@localhost:5432/gittree".to_string(),
                write_connection: None,
                max_connections: 0,
                min_connections: 0,
                idle_timeout_secs: None,
                max_lifetime_secs: None,
                application_name: None,
            },
        };
        let err = super::build_repositories(&config).expect_err("invalid pool");
        assert_eq!(git_http_error_label(&err), "storage");

        let invalid_read_connection = GitHttpConfig {
            storage: super::StorageConfig {
                read_connection: "this is not a url".to_string(),
                max_connections: 10,
                min_connections: 2,
                ..config.storage.clone()
            },
            ..config
        };
        let err = super::build_repositories(&invalid_read_connection)
            .expect_err("invalid read connection");
        assert_eq!(git_http_error_label(&err), "storage");
    }

    #[tokio::test]
    async fn serve_with_covers_bind_and_server_error_paths() {
        let occupied = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("occupied listener");
        let bind = occupied.local_addr().expect("occupied addr");
        let config = GitHttpConfig {
            bind: bind.to_string(),
            upstream_url: "https://git.example".to_string(),
            timeout: Duration::from_secs(1),
            auth: test_auth(),
            storage: super::StorageConfig {
                read_connection: "postgres://user:pass@localhost:5432/gittree".to_string(),
                write_connection: None,
                max_connections: 10,
                min_connections: 2,
                idle_timeout_secs: None,
                max_lifetime_secs: None,
                application_name: None,
            },
        };
        let bind_err = super::serve_with(config, init_ok_handle, noop_server)
            .await
            .expect_err("bind error");
        assert_eq!(git_http_error_label(&bind_err), "serve");

        let config = GitHttpConfig {
            bind: "127.0.0.1:0".to_string(),
            upstream_url: "https://git.example".to_string(),
            timeout: Duration::from_secs(1),
            auth: test_auth(),
            storage: super::StorageConfig {
                read_connection: "postgres://user:pass@localhost:5432/gittree".to_string(),
                write_connection: None,
                max_connections: 10,
                min_connections: 2,
                idle_timeout_secs: None,
                max_lifetime_secs: None,
                application_name: None,
            },
        };
        let serve_err = super::serve_with(config, init_ok_handle, fail_server)
            .await
            .expect_err("serve error");
        assert_eq!(git_http_error_label(&serve_err), "serve");
        assert!(serve_err.to_string().contains("boom"));
    }

    #[tokio::test]
    async fn serve_with_returns_storage_error_before_bind() {
        let config = GitHttpConfig {
            bind: "127.0.0.1:0".to_string(),
            upstream_url: "https://git.example".to_string(),
            timeout: Duration::from_secs(1),
            auth: test_auth(),
            storage: super::StorageConfig {
                read_connection: "postgres://user:pass@localhost:5432/gittree".to_string(),
                write_connection: None,
                max_connections: 0,
                min_connections: 0,
                idle_timeout_secs: None,
                max_lifetime_secs: None,
                application_name: None,
            },
        };
        let err = super::serve_with(config, init_ok_handle, noop_server)
            .await
            .expect_err("storage error");
        assert_eq!(git_http_error_label(&err), "storage");
    }

    #[tokio::test]
    async fn serve_with_returns_ok_when_server_finishes_cleanly() {
        let config = GitHttpConfig {
            bind: "127.0.0.1:0".to_string(),
            upstream_url: "https://git.example".to_string(),
            timeout: Duration::from_secs(1),
            auth: test_auth(),
            storage: super::StorageConfig {
                read_connection: "postgres://user:pass@localhost:5432/gittree".to_string(),
                write_connection: None,
                max_connections: 10,
                min_connections: 2,
                idle_timeout_secs: None,
                max_lifetime_secs: None,
                application_name: None,
            },
        };
        let result = super::serve_with(config, init_ok_handle, noop_server).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn run_axum_server_can_start_and_be_aborted() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let router = Router::new().route("/health", get(super::health_handler));
        let task = tokio::spawn(super::run_axum_server(listener, router));
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        task.abort();
        let _ = task.await;
    }

    #[test]
    fn serve_maps_observability_config_error() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        with_env_var("GITTREE_METRICS_ENABLED", "invalid-bool", &mut || {
            let config = GitHttpConfig {
                bind: "127.0.0.1:0".to_string(),
                upstream_url: "https://git.example".to_string(),
                timeout: Duration::from_secs(1),
                auth: test_auth(),
                storage: super::StorageConfig {
                    read_connection: "postgres://user:pass@localhost:5432/gittree".to_string(),
                    write_connection: None,
                    max_connections: 10,
                    min_connections: 2,
                    idle_timeout_secs: None,
                    max_lifetime_secs: None,
                    application_name: None,
                },
            };
            let err = runtime
                .block_on(super::serve(config))
                .expect_err("observability config error");
            assert_eq!(git_http_error_label(&err), "observability_config");
        });
    }

    #[test]
    fn build_request_url_and_auth_parsing_cover_error_paths() {
        let headers = HeaderMap::new();
        let uri: axum::http::Uri = "/owner/repo.git/git-receive-pack?service=git-receive-pack"
            .parse()
            .expect("uri");
        let err = super::build_request_url(&headers, &uri).expect_err("missing host");
        assert_eq!(err.into_response().status(), StatusCode::BAD_REQUEST);

        let mut headers = HeaderMap::new();
        headers.insert("host", "git.example".parse().expect("host"));
        headers.insert("x-forwarded-proto", "https".parse().expect("proto"));
        let url = super::build_request_url(&headers, &uri).expect("url");
        assert!(url.starts_with("https://git.example/"));

        let mut invalid_host = HeaderMap::new();
        invalid_host.insert(
            "host",
            axum::http::HeaderValue::from_bytes(b"\xff").expect("host value"),
        );
        let err = super::build_request_url(&invalid_host, &uri).expect_err("invalid host");
        assert_eq!(err.into_response().status(), StatusCode::BAD_REQUEST);

        let mut invalid_proto = HeaderMap::new();
        invalid_proto.insert("host", "git.example".parse().expect("host"));
        invalid_proto.insert(
            "x-forwarded-proto",
            axum::http::HeaderValue::from_bytes(b"\xff").expect("proto value"),
        );
        let url = super::build_request_url(&invalid_proto, &uri).expect("fallback proto url");
        assert!(url.starts_with("http://git.example/"));

        let mut no_query_headers = HeaderMap::new();
        no_query_headers.insert("host", "git.example".parse().expect("host"));
        let uri_without_path_query: axum::http::Uri =
            "/owner/repo.git/git-receive-pack".parse().expect("uri");
        let url = super::build_request_url(&no_query_headers, &uri_without_path_query)
            .expect("uri path fallback");
        assert_eq!(
            url,
            format!("http://git.example{}", uri_without_path_query.path())
        );

        let missing_auth = super::parse_nostr_auth(&HeaderMap::new()).expect_err("missing auth");
        assert_eq!(
            missing_auth.into_response().status(),
            StatusCode::UNAUTHORIZED
        );

        let mut invalid_prefix = HeaderMap::new();
        invalid_prefix.insert(AUTH_HEADER, "Bearer token".parse().expect("auth"));
        let err = super::parse_nostr_auth(&invalid_prefix).expect_err("invalid prefix");
        assert_eq!(err.into_response().status(), StatusCode::UNAUTHORIZED);

        let mut invalid_base64 = HeaderMap::new();
        invalid_base64.insert(AUTH_HEADER, "Nostr !!!".parse().expect("auth"));
        let err = super::parse_nostr_auth(&invalid_base64).expect_err("invalid base64");
        assert_eq!(err.into_response().status(), StatusCode::UNAUTHORIZED);

        let token = BASE64_STANDARD.encode(b"{\"invalid\":true}");
        let mut invalid_event = HeaderMap::new();
        invalid_event.insert(AUTH_HEADER, format!("Nostr {token}").parse().expect("auth"));
        let err = super::parse_nostr_auth(&invalid_event).expect_err("invalid event");
        assert_eq!(err.into_response().status(), StatusCode::UNAUTHORIZED);

        let mut invalid_header_bytes = HeaderMap::new();
        invalid_header_bytes.insert(
            AUTH_HEADER,
            axum::http::HeaderValue::from_bytes(b"\xff").expect("auth value"),
        );
        let err = super::parse_nostr_auth(&invalid_header_bytes).expect_err("invalid header bytes");
        assert_eq!(err.into_response().status(), StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn payload_hash_handles_empty_and_non_empty_inputs() {
        assert!(super::payload_hash(&Bytes::new()).is_none());
        assert!(super::payload_hash(&Bytes::from_static(b"payload")).is_some());
    }

    #[test]
    fn signed_event_skips_payload_tag_for_empty_body() {
        let event = signed_event(
            "http://localhost/npub1test/repo.git/git-receive-pack",
            "POST",
            &Bytes::new(),
            super::unix_timestamp(),
        );
        assert!(
            !event
                .tags
                .iter()
                .any(|tag| tag.first().map(String::as_str) == Some("payload"))
        );
    }

    #[test]
    fn git_http_error_display_and_source_cover_variants() {
        let config = GitHttpError::Config(GitHttpConfigError::MissingEnv("MISSING"));
        assert!(format!("{config}").contains("git-http error"));
        assert!(config.source().is_some());

        let observability_config =
            GitHttpError::ObservabilityConfig(ObservabilityConfigError::InvalidEnv {
                key: "KEY",
                value: "bad".to_string(),
            });
        assert!(format!("{observability_config}").contains("observability config error"));
        assert!(observability_config.source().is_some());

        let observability =
            GitHttpError::Observability(ObservabilityError::MetricsInit("failed".to_string()));
        assert!(format!("{observability}").contains("observability error"));
        assert!(observability.source().is_some());

        let storage = GitHttpError::Storage(StorageError::Internal {
            message: "db".to_string(),
        });
        assert!(format!("{storage}").contains("git-http storage error"));
        assert!(storage.source().is_some());

        let upstream = GitHttpError::Upstream("upstream".to_string());
        assert_eq!(format!("{upstream}"), "git-http upstream error: upstream");
        assert!(upstream.source().is_none());

        let serve = GitHttpError::Serve("bind".to_string());
        assert_eq!(format!("{serve}"), "git-http serve error: bind");
        assert!(serve.source().is_none());
    }

    #[test]
    fn git_http_config_and_storage_error_display_paths_are_stable() {
        let config = GitHttpConfigError::Config(ConfigError::InvalidConfig {
            field: "field",
            value: "value".to_string(),
        });
        assert!(format!("{config}").contains("git-http config error"));
        assert!(config.source().is_some());

        let missing_env = GitHttpConfigError::MissingEnv("ENV");
        assert_eq!(format!("{missing_env}"), "missing env ENV");
        assert!(missing_env.source().is_none());

        let invalid_env = GitHttpConfigError::InvalidEnv {
            key: "ENV",
            value: "bad".to_string(),
        };
        assert_eq!(format!("{invalid_env}"), "invalid env ENV: bad");
        assert!(invalid_env.source().is_none());

        let storage = GitHttpConfigError::Storage(StorageConfigError::InvalidConfig(
            "invalid storage".to_string(),
        ));
        assert!(format!("{storage}").contains("storage config error"));
        assert!(storage.source().is_some());

        assert_eq!(
            format!("{}", StorageConfigError::MissingEnv("READ")),
            "missing env READ"
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
    }

    #[test]
    fn git_http_http_error_into_response_maps_status_codes() {
        assert_eq!(
            super::GitHttpHttpError::NotFound("missing".to_string())
                .into_response()
                .status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            super::GitHttpHttpError::BadRequest("bad".to_string())
                .into_response()
                .status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            super::GitHttpHttpError::Unauthorized("unauthorized".to_string())
                .into_response()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            super::GitHttpHttpError::Storage("storage".to_string())
                .into_response()
                .status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
        assert_eq!(
            super::GitHttpHttpError::Upstream("upstream".to_string())
                .into_response()
                .status(),
            StatusCode::BAD_GATEWAY
        );
        assert_eq!(
            super::GitHttpHttpError::Internal("internal".to_string())
                .into_response()
                .status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }

    #[tokio::test]
    async fn reqwest_upstream_client_sends_request_and_reads_response() {
        let (base_url, handle) = start_mock_http_server("200 OK", "text/plain", "upstream");
        let client = ReqwestUpstreamClient::new(Duration::from_secs(1)).expect("client");
        let response = client
            .send(UpstreamRequest {
                method: Method::GET,
                url: base_url,
                headers: HeaderMap::new(),
                body: Bytes::new(),
            })
            .await
            .expect("upstream response");
        assert_eq!(response.status, StatusCode::OK);
        assert_eq!(response.body, Bytes::from_static(b"upstream"));
        handle.join().expect("server join");
    }

    #[test]
    fn reqwest_upstream_client_new_maps_builder_errors() {
        let result = super::ReqwestUpstreamClient::new_with(Duration::from_secs(1), |_| {
            Err("builder failed".to_string())
        });
        assert!(result.is_err());
        let err = result.err().expect("builder error expected");
        assert_eq!(git_http_error_label(&err), "upstream");
        assert!(err.to_string().contains("builder failed"));
    }

    #[test]
    fn reqwest_upstream_client_new_with_supports_success_path() {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(1))
            .build()
            .expect("client");
        let result = super::ReqwestUpstreamClient::new_with(Duration::from_secs(1), |_| Ok(client));
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn reqwest_upstream_client_maps_transport_errors() {
        let client = ReqwestUpstreamClient::new(Duration::from_millis(100)).expect("client");
        let err = client
            .send(UpstreamRequest {
                method: Method::GET,
                url: "http://127.0.0.1:1".to_string(),
                headers: HeaderMap::new(),
                body: Bytes::new(),
            })
            .await
            .expect_err("expected transport error");
        assert_eq!(upstream_error_label(&err), "request");
    }

    #[tokio::test]
    async fn reqwest_upstream_client_maps_body_read_errors() {
        let raw = b"HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\nconnection: close\r\n\r\nzz\r\nbody\r\n0\r\n\r\n";
        let (base_url, handle) = start_raw_http_server(raw);
        let client = ReqwestUpstreamClient::new(Duration::from_secs(1)).expect("client");
        let err = client
            .send(UpstreamRequest {
                method: Method::GET,
                url: base_url,
                headers: HeaderMap::new(),
                body: Bytes::new(),
            })
            .await
            .expect_err("expected body read error");
        assert_eq!(upstream_error_label(&err), "request");
        handle.join().expect("server join");
    }
}
