use async_trait::async_trait;
use axum::body::{Body, Bytes, to_bytes};
use axum::extract::State;
use axum::http::{HeaderMap, Method, Request, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine;
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
        let upstream_url = std::env::var(ENV_UPSTREAM_URL)
            .map_err(|_| GitHttpConfigError::MissingEnv(ENV_UPSTREAM_URL))?;
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
            value
                .parse::<u64>()
                .map(Some)
                .map_err(|_| GitHttpConfigError::InvalidEnv { key, value })
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
    let read_connection = std::env::var(ENV_STORAGE_READ_URL).map_err(|_| {
        GitHttpConfigError::Storage(StorageConfigError::MissingEnv(ENV_STORAGE_READ_URL))
    })?;
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

    config.validate().map_err(|err| {
        GitHttpConfigError::Storage(StorageConfigError::InvalidConfig(err.to_string()))
    })?;

    Ok(config)
}

fn storage_env_u32(key: &'static str) -> Result<Option<u32>, GitHttpConfigError> {
    match std::env::var(key) {
        Ok(value) => {
            if value.trim().is_empty() {
                return Ok(None);
            }
            value.parse::<u32>().map(Some).map_err(|_| {
                GitHttpConfigError::Storage(StorageConfigError::InvalidEnv { key, value })
            })
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
            value.parse::<u64>().map(Some).map_err(|_| {
                GitHttpConfigError::Storage(StorageConfigError::InvalidEnv { key, value })
            })
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
        tracing::info!(
            route = route_label(route),
            status,
            duration_ms = duration.as_millis(),
            "git-http request handled"
        );
    }
}

pub async fn serve(config: GitHttpConfig) -> Result<(), GitHttpError> {
    let _observability = init_observability()?;
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
    let listener = tokio::net::TcpListener::bind(&config.bind)
        .await
        .map_err(|err| GitHttpError::Serve(err.to_string()))?;
    axum::serve(listener, router)
        .await
        .map_err(|err| GitHttpError::Serve(err.to_string()))?;
    Ok(())
}

fn build_repositories(config: &GitHttpConfig) -> Result<PostgresRepositories, GitHttpError> {
    let pool_options = config.storage.pool_options().map_err(GitHttpError::Storage)?;
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
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .map_err(|err| GitHttpError::Upstream(err.to_string()))?;
        Ok(Self { client })
    }
}

#[async_trait]
impl UpstreamClient for ReqwestUpstreamClient {
    async fn send(&self, request: UpstreamRequest) -> Result<UpstreamResponse, UpstreamError> {
        let method = reqwest::Method::from_bytes(request.method.as_str().as_bytes())
            .map_err(|err| UpstreamError::Request(err.to_string()))?;
        let mut builder = self.client.request(method, request.url);
        builder = builder.headers(request.headers);
        builder = builder.body(request.body);
        let response = builder
            .send()
            .await
            .map_err(|err| UpstreamError::Request(err.to_string()))?;
        let status = StatusCode::from_u16(response.status().as_u16())
            .unwrap_or(StatusCode::BAD_GATEWAY);
        let headers = response.headers().clone();
        let body = response
            .bytes()
            .await
            .map_err(|err| UpstreamError::Request(err.to_string()))?;
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
    state.metrics.record(&route, response.status().as_u16(), start.elapsed());
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
        state.upstream_url,
        mapping.forgejo_owner,
        mapping.forgejo_repo,
        suffix
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
    let body = to_bytes(body, usize::MAX)
        .await
        .map_err(|err| GitHttpHttpError::Internal(err.to_string()))?;

    if matches!(route, GitHttpRoute::ReceivePack { .. }) {
        authorize_receive_pack(
            state,
            repo,
            &auth_headers,
            &parts.method,
            &parts.uri,
            &body,
        )
        .await?;
    }

    let upstream_request = UpstreamRequest {
        method: parts.method,
        url,
        headers,
        body,
    };
    let upstream_response = state
        .upstream
        .send(upstream_request)
        .await
        .map_err(|err| GitHttpHttpError::Upstream(err.to_string()))?;

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
    let pubkey = hex::decode(&repo.pubkey)
        .map_err(|_| GitHttpHttpError::Internal("invalid repo pubkey".to_string()))?;
    let record = repositories
        .mapping_by_repo(&pubkey, &repo.identifier)
        .await
        .map_err(|err| GitHttpHttpError::Storage(err.to_string()))?;
    record.ok_or_else(|| GitHttpHttpError::NotFound("missing repo mapping".to_string()))
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
    let auth = validate_nip98(&event, &request)
        .map_err(|err| GitHttpHttpError::Unauthorized(err.to_string()))?;
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
        let pubkey_bytes = hex::decode(&pubkey)
            .map_err(|_| GitHttpHttpError::Internal("invalid maintainer pubkey".to_string()))?;
        let announcement = repositories
            .latest_announcement(&pubkey_bytes, &repo.identifier)
            .await
            .map_err(|err| GitHttpHttpError::Storage(err.to_string()))?;
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
    let host = headers
        .get("host")
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| GitHttpHttpError::BadRequest("missing host header".to_string()))?;
    let scheme = headers
        .get("x-forwarded-proto")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("http");
    let path = uri
        .path_and_query()
        .map(|value| value.as_str())
        .unwrap_or_else(|| uri.path());
    Ok(format!("{scheme}://{host}{path}"))
}

fn parse_nostr_auth(headers: &HeaderMap) -> Result<Nip98Event, GitHttpHttpError> {
    let value = headers
        .get(AUTH_HEADER)
        .and_then(|header| header.to_str().ok())
        .ok_or_else(|| GitHttpHttpError::Unauthorized("missing authorization".to_string()))?;
    let value = value.trim();
    let Some(token) = value.strip_prefix("Nostr ") else {
        return Err(GitHttpHttpError::Unauthorized(
            "invalid authorization header".to_string(),
        ));
    };
    let decoded = BASE64_STANDARD
        .decode(token.as_bytes())
        .map_err(|_| GitHttpHttpError::Unauthorized("invalid nostr authorization".to_string()))?;
    serde_json::from_slice::<Nip98Event>(&decoded)
        .map_err(|_| GitHttpHttpError::Unauthorized("invalid nostr event".to_string()))
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
    let mut parts = trimmed.split('/').filter(|segment| !segment.is_empty());
    let npub = parts.next()?.to_string();
    let repo = parts.next()?.to_string();
    let rest = parts.map(|segment| segment.to_string()).collect::<Vec<_>>();
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
    let parsed = gittree_core::parse_repo_path(&path)
        .map_err(|err| GitHttpRouteError::InvalidRepo(err.to_string()))?;
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
    use super::ENV_STORAGE_MAX_CONNECTIONS;
    use super::ENV_STORAGE_MIN_CONNECTIONS;
    use super::ENV_STORAGE_READ_URL;
    use super::ENV_TIMEOUT_SECS;
    use super::ENV_UPSTREAM_URL;
    use super::GitHttpConfigError;
    use super::GitHttpError;
    use super::GitHttpAppState;
    use super::GitHttpConfig;
    use super::GitHttpMetrics;
    use super::GitHttpRequest;
    use super::GitHttpRoute;
    use super::GitHttpService;
    use super::ObservabilityHandle;
    use super::StorageConfigError;
    use super::UpstreamClient;
    use super::UpstreamError;
    use super::UpstreamRequest;
    use super::UpstreamResponse;
    use super::ReqwestUpstreamClient;
    use super::init_observability;
    use super::payload_hash;
    use super::route_request;
    use async_trait::async_trait;
    use axum::body::{Body, Bytes, to_bytes};
    use axum::http::{HeaderMap, Method, Request, StatusCode};
    use axum::response::IntoResponse;
    use base64::Engine;
    use gittree_config::ConfigError;
    use gittree_core::{RepoAnnouncement, RepoMapping};
    use gittree_nostr_auth::{Nip98Event, NIP98_KIND};
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
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::sync::OnceLock;
    use std::time::Duration;
    use tower::ServiceExt;

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

    #[test]
    fn config_loads_from_env() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var(ENV_STORAGE_READ_URL, "postgres://user:pass@localhost:5432/gittree", || {
            with_env_var(ENV_UPSTREAM_URL, "https://git.example", || {
                with_env_var("GITTREE_GIT_HTTP_BIND", "127.0.0.1:9090", || {
                    with_env_var(ENV_TIMEOUT_SECS, "15", || {
                        let config = GitHttpConfig::from_env().expect("config");
                        assert_eq!(config.bind, "127.0.0.1:9090");
                        assert_eq!(config.upstream_url, "https://git.example");
                        assert_eq!(config.timeout, Duration::from_secs(15));
                    });
                });
            });
        });
    }

    #[test]
    fn config_ignores_empty_timeout_override() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var(ENV_STORAGE_READ_URL, "postgres://user:pass@localhost:5432/gittree", || {
            with_env_var(ENV_UPSTREAM_URL, "https://git.example", || {
                with_env_var(ENV_TIMEOUT_SECS, "", || {
                    let config = GitHttpConfig::from_env().expect("config");
                    assert_eq!(
                        config.timeout,
                        Duration::from_secs(super::DEFAULT_TIMEOUT_SECS)
                    );
                });
            });
        });
    }

    #[test]
    fn config_rejects_invalid_upstream_url() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var(ENV_STORAGE_READ_URL, "postgres://user:pass@localhost:5432/gittree", || {
            with_env_var(ENV_UPSTREAM_URL, "not-a-url", || {
                let err = GitHttpConfig::from_env().expect_err("invalid upstream");
                assert!(matches!(
                    err,
                    GitHttpConfigError::InvalidEnv {
                        key: ENV_UPSTREAM_URL,
                        ..
                    }
                ));
            });
        });
    }

    #[test]
    fn config_rejects_invalid_timeout_value() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var(ENV_STORAGE_READ_URL, "postgres://user:pass@localhost:5432/gittree", || {
            with_env_var(ENV_UPSTREAM_URL, "https://git.example", || {
                with_env_var(ENV_TIMEOUT_SECS, "bad-timeout", || {
                    let err = GitHttpConfig::from_env().expect_err("invalid timeout");
                    assert!(matches!(
                        err,
                        GitHttpConfigError::InvalidEnv {
                            key: ENV_TIMEOUT_SECS,
                            ..
                        }
                    ));
                });
            });
        });
    }

    #[test]
    fn config_rejects_invalid_storage_numeric_values() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var(ENV_STORAGE_READ_URL, "postgres://user:pass@localhost:5432/gittree", || {
            with_env_var(ENV_UPSTREAM_URL, "https://git.example", || {
                with_env_var(ENV_STORAGE_MAX_CONNECTIONS, "oops", || {
                    let err = GitHttpConfig::from_env().expect_err("invalid storage value");
                    assert!(matches!(
                        err,
                        GitHttpConfigError::Storage(StorageConfigError::InvalidEnv {
                            key: ENV_STORAGE_MAX_CONNECTIONS,
                            ..
                        })
                    ));
                });
            });
        });
    }

    #[test]
    fn config_rejects_invalid_storage_bounds() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var(ENV_STORAGE_READ_URL, "postgres://user:pass@localhost:5432/gittree", || {
            with_env_var(ENV_UPSTREAM_URL, "https://git.example", || {
                with_env_var(ENV_STORAGE_MAX_CONNECTIONS, "1", || {
                    with_env_var(ENV_STORAGE_MIN_CONNECTIONS, "2", || {
                        let err = GitHttpConfig::from_env().expect_err("invalid bounds");
                        assert!(matches!(
                            err,
                            GitHttpConfigError::Storage(StorageConfigError::InvalidConfig(_))
                        ));
                    });
                });
            });
        });
    }

    #[test]
    fn route_request_handles_info_refs() {
        let request = GitHttpRequest::new(
            "GET",
            "/npub1gjttreegkzys8jlhdnfm3qe39h2gka79cpndd0jsms5fk7tuhcnsdw56jq/repo.git/info/refs",
            Some("service=git-upload-pack"),
        );
        let route = route_request(&request);
        assert!(matches!(
            route,
            GitHttpRoute::InfoRefs {
                service: GitHttpService::UploadPack,
                ..
            }
        ));
    }

    #[test]
    fn route_request_handles_receive_pack() {
        let request = GitHttpRequest::new(
            "POST",
            "/npub1gjttreegkzys8jlhdnfm3qe39h2gka79cpndd0jsms5fk7tuhcnsdw56jq/repo.git/git-receive-pack",
            None,
        );
        let route = route_request(&request);
        assert!(matches!(route, GitHttpRoute::ReceivePack { .. }));
    }

    #[test]
    fn route_request_rejects_missing_git_suffix() {
        let request = GitHttpRequest::new(
            "GET",
            "/npub1gjttreegkzys8jlhdnfm3qe39h2gka79cpndd0jsms5fk7tuhcnsdw56jq/repo/info/refs",
            Some("service=git-upload-pack"),
        );
        let route = route_request(&request);
        assert!(matches!(route, GitHttpRoute::NotFound));
    }

    #[test]
    fn route_request_rejects_missing_service_param() {
        let request = GitHttpRequest::new(
            "GET",
            "/npub1gjttreegkzys8jlhdnfm3qe39h2gka79cpndd0jsms5fk7tuhcnsdw56jq/repo.git/info/refs",
            None,
        );
        assert!(matches!(route_request(&request), GitHttpRoute::NotFound));
    }

    #[test]
    fn route_request_rejects_invalid_service_param() {
        let request = GitHttpRequest::new(
            "GET",
            "/npub1gjttreegkzys8jlhdnfm3qe39h2gka79cpndd0jsms5fk7tuhcnsdw56jq/repo.git/info/refs",
            Some("service=git-bad"),
        );
        assert!(matches!(route_request(&request), GitHttpRoute::NotFound));
    }

    #[test]
    fn route_request_rejects_wrong_method_for_receive_pack() {
        let request = GitHttpRequest::new(
            "GET",
            "/npub1gjttreegkzys8jlhdnfm3qe39h2gka79cpndd0jsms5fk7tuhcnsdw56jq/repo.git/git-receive-pack",
            None,
        );
        assert!(matches!(route_request(&request), GitHttpRoute::NotFound));
    }

    fn test_auth() -> AuthConfig {
        AuthConfig {
            email_domain: "example.com".to_string(),
            max_skew_seconds: 60,
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
        response: UpstreamResponse,
    }

    impl MockUpstreamClient {
        fn new(response: UpstreamResponse) -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
                response,
            }
        }
    }

    #[async_trait]
    impl UpstreamClient for MockUpstreamClient {
        async fn send(&self, request: UpstreamRequest) -> Result<UpstreamResponse, UpstreamError> {
            let mut calls = self.calls.lock().expect("calls");
            calls.push(request);
            Ok(self.response.clone())
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
            .oneshot(Request::builder().uri("/health").body(Body::empty()).unwrap())
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
        let mapping =
            RepoMapping::new("owner", "repo", parsed.pubkey, "repo").expect("mapping");
        let record = RepoMappingRecord::new(&mapping).expect("record");
        repositories
            .upsert_mapping(record)
            .await
            .expect("mapping");

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
        let body = to_bytes(response.into_body(), usize::MAX).await.expect("body");
        assert_eq!(body, Bytes::from_static(b"upstream"));

        let calls = upstream.calls.lock().expect("calls");
        assert_eq!(calls.len(), 1);
        assert_eq!(
            calls[0].url,
            "https://git.example/owner/repo.git/info/refs?service=git-upload-pack"
        );
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
        repositories
            .upsert_mapping(record)
            .await
            .expect("mapping");

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
        repositories
            .upsert_mapping(record)
            .await
            .expect("mapping");

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

    #[test]
    fn observability_init_returns_registry() {
        let handle = OBSERVABILITY.get_or_init(|| init_observability().expect("init"));
        assert!(handle.prometheus_registry().is_some());
    }

    #[test]
    fn metrics_record_accepts_requests() {
        let _handle = OBSERVABILITY.get_or_init(|| init_observability().expect("init"));
        let metrics = GitHttpMetrics::new();
        let route = GitHttpRoute::NotFound;
        metrics.record(&route, 200, Duration::from_millis(5));
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

        let missing_auth = super::parse_nostr_auth(&HeaderMap::new()).expect_err("missing auth");
        assert_eq!(missing_auth.into_response().status(), StatusCode::UNAUTHORIZED);

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
        invalid_event.insert(
            AUTH_HEADER,
            format!("Nostr {token}").parse().expect("auth"),
        );
        let err = super::parse_nostr_auth(&invalid_event).expect_err("invalid event");
        assert_eq!(err.into_response().status(), StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn payload_hash_handles_empty_and_non_empty_inputs() {
        assert!(super::payload_hash(&Bytes::new()).is_none());
        assert!(super::payload_hash(&Bytes::from_static(b"payload")).is_some());
    }

    #[test]
    fn git_http_error_display_and_source_cover_variants() {
        let config = GitHttpError::Config(GitHttpConfigError::MissingEnv("MISSING"));
        assert!(format!("{config}").contains("git-http error"));
        assert!(config.source().is_some());

        let observability_config = GitHttpError::ObservabilityConfig(
            ObservabilityConfigError::InvalidEnv {
                key: "KEY",
                value: "bad".to_string(),
            },
        );
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
        assert!(matches!(err, UpstreamError::Request(_)));
    }
}
