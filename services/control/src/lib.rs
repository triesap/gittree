use axum::body::Bytes;
use axum::extract::{OriginalUri, State};
use axum::http::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE};
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine;
use bech32::{Bech32, Hrp};
use gittree_app_core::{
    nip98_payload_hash, normalize_identifier, RepoCreateRequest, RepoCreateResponse,
    SignedNostrEvent as ApiSignedNostrEvent,
};
use gittree_config::{
    ConfigError, ControlAuthConfig, ForgejoConfig, RelayTargetsConfig, ServicesConfig, UiConfig,
};
use gittree_core::kinds::{KIND_GIT_REPO_ANNOUNCEMENT, KIND_GITTREE_CONTROL};
use gittree_core::{format_grasp_server_url_as_clone_url, ControlAction, RepoAnnouncement};
use gittree_forgejo::{
    ForgejoClient, ForgejoCreateOrg, ForgejoCreatePullRequest, ForgejoCreateRepo,
    ForgejoCreateUser, ForgejoError, ForgejoOrg, ForgejoPullRequest, ForgejoRepo, ForgejoTransport,
    ForgejoUser, ReqwestTransport,
};
use gittree_nostr_auth::{validate_nip98, Nip98Event, Nip98Request};
use gittree_observability::{ObservabilityConfigError, ObservabilityError, ObservabilityHandle};
use gittree_relay_adapter::SignedNostrEvent as RelaySignedNostrEvent;
use gittree_storage::{
    AccountRepository, PostgresRepositories, RelayMembershipRecord, RelayMembershipRepository,
    RelayPublishRepository, RelayPublishRequest, RelayTenantRecord, RelayTenantRepository,
    StorageConfig, StorageError,
};
use secp256k1::rand::{rngs::OsRng, RngCore};
use secp256k1::{Keypair, Message, Secp256k1, SecretKey, XOnlyPublicKey};
use serde::{Deserialize, Serialize};
use sha2::Digest;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tower_http::cors::{Any, CorsLayer};

#[allow(dead_code)]
const AUTH_HEADER: &str = "authorization";
const ENV_STORAGE_READ_URL: &str = "GITTREE_STORAGE_READ_URL";
const ENV_STORAGE_WRITE_URL: &str = "GITTREE_STORAGE_WRITE_URL";
const ENV_STORAGE_MAX_CONNECTIONS: &str = "GITTREE_STORAGE_MAX_CONNECTIONS";
const ENV_STORAGE_MIN_CONNECTIONS: &str = "GITTREE_STORAGE_MIN_CONNECTIONS";
const ENV_STORAGE_IDLE_TIMEOUT_SECS: &str = "GITTREE_STORAGE_IDLE_TIMEOUT_SECS";
const ENV_STORAGE_MAX_LIFETIME_SECS: &str = "GITTREE_STORAGE_MAX_LIFETIME_SECS";
const ENV_STORAGE_APP_NAME: &str = "GITTREE_STORAGE_APP_NAME";
const DEFAULT_CONTROL_MAX_SKEW_SECONDS: i64 = 300;
const DEFAULT_RELAY_SECRET_KID: &str = "local";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControlConfig {
    pub bind: String,
    pub auth: ControlAuthConfig,
    pub forgejo: ForgejoConfig,
    pub storage: StorageConfig,
    pub relay_urls: Vec<String>,
    pub public_git_url: String,
}

impl ControlConfig {
    pub fn from_env() -> Result<Self, ControlConfigError> {
        let services = ServicesConfig::from_env_validated().map_err(ControlConfigError::Config)?;
        let auth = ControlAuthConfig::from_env().map_err(ControlConfigError::Config)?;
        let forgejo = ForgejoConfig::from_env().map_err(ControlConfigError::Config)?;
        let storage = storage_from_env()?;
        let relay_targets =
            RelayTargetsConfig::from_env_validated().map_err(ControlConfigError::Config)?;
        let ui = UiConfig::from_env().map_err(ControlConfigError::Config)?;
        Ok(Self {
            bind: services.control.bind,
            auth,
            forgejo,
            storage,
            relay_urls: relay_targets.relay_urls,
            public_git_url: ui.public_git_url,
        })
    }
}

#[derive(Debug)]
pub enum ControlConfigError {
    Config(ConfigError),
    Storage(StorageConfigError),
}

impl std::fmt::Display for ControlConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ControlConfigError::Config(err) => write!(f, "control config error: {err}"),
            ControlConfigError::Storage(err) => write!(f, "control storage config error: {err}"),
        }
    }
}

impl std::error::Error for ControlConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ControlConfigError::Config(err) => Some(err),
            ControlConfigError::Storage(err) => Some(err),
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

fn storage_from_env() -> Result<StorageConfig, ControlConfigError> {
    let read_connection = std::env::var(ENV_STORAGE_READ_URL).map_err(|_| {
        ControlConfigError::Storage(StorageConfigError::MissingEnv(ENV_STORAGE_READ_URL))
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
        ControlConfigError::Storage(StorageConfigError::InvalidConfig(err.to_string()))
    })?;

    Ok(config)
}

fn env_u32(key: &'static str) -> Result<Option<u32>, ControlConfigError> {
    match std::env::var(key) {
        Ok(value) => {
            if value.trim().is_empty() {
                return Ok(None);
            }
            value.parse::<u32>().map(Some).map_err(|_| {
                ControlConfigError::Storage(StorageConfigError::InvalidEnv { key, value })
            })
        }
        Err(_) => Ok(None),
    }
}

fn env_u64(key: &'static str) -> Result<Option<u64>, ControlConfigError> {
    match std::env::var(key) {
        Ok(value) => {
            if value.trim().is_empty() {
                return Ok(None);
            }
            value.parse::<u64>().map(Some).map_err(|_| {
                ControlConfigError::Storage(StorageConfigError::InvalidEnv { key, value })
            })
        }
        Err(_) => Ok(None),
    }
}

#[derive(Debug)]
pub enum ControlError {
    Config(ControlConfigError),
    Forgejo(ForgejoError),
    ObservabilityConfig(ObservabilityConfigError),
    Observability(ObservabilityError),
    Storage(StorageError),
    Serve(String),
}

impl std::fmt::Display for ControlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ControlError::Config(err) => write!(f, "control error: {err}"),
            ControlError::Forgejo(err) => write!(f, "control forgejo error: {err}"),
            ControlError::ObservabilityConfig(err) => {
                write!(f, "control observability config error: {err}")
            }
            ControlError::Observability(err) => write!(f, "control observability error: {err}"),
            ControlError::Storage(err) => write!(f, "control storage error: {err}"),
            ControlError::Serve(err) => write!(f, "control serve error: {err}"),
        }
    }
}

impl std::error::Error for ControlError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ControlError::Config(err) => Some(err),
            ControlError::Forgejo(err) => Some(err),
            ControlError::ObservabilityConfig(err) => Some(err),
            ControlError::Observability(err) => Some(err),
            ControlError::Storage(err) => Some(err),
            ControlError::Serve(_) => None,
        }
    }
}

pub fn init_observability() -> Result<ObservabilityHandle, ControlError> {
    let config = gittree_observability::ObservabilityConfig::from_env("gittree-control")
        .map_err(ControlError::ObservabilityConfig)?;
    let handle = gittree_observability::init(&config).map_err(ControlError::Observability)?;
    Ok(handle)
}

pub fn build_repositories(
    config: &ControlConfig,
) -> Result<PostgresRepositories, ControlError> {
    let pool_options = config.storage.pool_options().map_err(ControlError::Storage)?;
    let connect_options = config
        .storage
        .read_connect_options()
        .map_err(ControlError::Storage)?;
    let pool = pool_options.connect_lazy_with(connect_options);
    Ok(PostgresRepositories::new(pool))
}

#[derive(Clone)]
struct ControlAppState {
    auth: ControlAuthConfig,
    forgejo: ForgejoClient<Arc<dyn ForgejoTransport>>,
    forgejo_owner: String,
    repositories: Arc<dyn ControlRepositories>,
    relay_urls: Vec<String>,
    public_git_url: String,
    repo_private_default: bool,
}

trait ControlRepositories:
    AccountRepository + RelayPublishRepository + RelayTenantRepository + RelayMembershipRepository
{
}

impl<T> ControlRepositories for T where
    T: AccountRepository + RelayPublishRepository + RelayTenantRepository + RelayMembershipRepository
{
}

pub async fn serve(config: ControlConfig) -> Result<(), ControlError> {
    let _observability = init_observability()?;
    let repositories = build_repositories(&config)?;
    let bind = config.bind.clone();
    let relay_urls = config.relay_urls;
    let public_git_url = config.public_git_url;
    let auth = config.auth;
    let forgejo_config = config.forgejo;
    let forgejo_owner = forgejo_config.owner.clone();
    let repo_private_default = forgejo_config.repo_private;
    let transport =
        ReqwestTransport::new(forgejo_config.api_token.clone()).map_err(ControlError::Forgejo)?;
    let transport: Arc<dyn ForgejoTransport> = Arc::new(transport);
    let forgejo = ForgejoClient::with_transport(forgejo_config, transport);
    let state = ControlAppState {
        auth,
        forgejo,
        forgejo_owner,
        repositories: Arc::new(repositories),
        relay_urls,
        public_git_url,
        repo_private_default,
    };
    let router = build_router(state);
    let listener = tokio::net::TcpListener::bind(&bind)
        .await
        .map_err(|err| ControlError::Serve(err.to_string()))?;
    axum::serve(listener, router)
        .await
        .map_err(|err| ControlError::Serve(err.to_string()))?;
    Ok(())
}

fn build_router(state: ControlAppState) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods([Method::POST, Method::OPTIONS])
        .allow_headers([AUTHORIZATION, CONTENT_TYPE, ACCEPT]);
    Router::new()
        .route("/health", get(health_handler))
        .route("/v1/relay/tenants", post(create_tenant_handler))
        .route("/v1/repos", post(create_repo_nostr_handler))
        .route("/control/users", post(create_user_handler))
        .route("/control/orgs", post(create_org_handler))
        .route("/control/repos", post(create_repo_handler))
        .route("/control/pulls", post(create_pull_handler))
        .route("/control/events", post(control_event_handler))
        .layer(cors)
        .with_state(state)
}

async fn health_handler(State(state): State<ControlAppState>) -> &'static str {
    let _ = (&state.auth, &state.forgejo);
    "ok"
}

#[allow(dead_code)]
fn authorize(headers: &HeaderMap, token: &str) -> Result<(), ControlHttpError> {
    let value = headers
        .get(AUTH_HEADER)
        .and_then(|header| header.to_str().ok())
        .ok_or_else(|| ControlHttpError::Unauthorized("missing authorization".to_string()))?;
    let value = value.trim();
    let Some(value) = value.strip_prefix("Bearer ") else {
        return Err(ControlHttpError::Unauthorized(
            "invalid authorization header".to_string(),
        ));
    };
    if value != token {
        return Err(ControlHttpError::Unauthorized(
            "invalid control token".to_string(),
        ));
    }
    Ok(())
}

fn authorize_admin_pubkey(
    pubkey: &str,
    auth: &ControlAuthConfig,
) -> Result<(), ControlHttpError> {
    if auth.admin_keys.is_empty() {
        return Ok(());
    }
    if auth.admin_keys.iter().any(|key| key == pubkey) {
        return Ok(());
    }
    Err(ControlHttpError::Unauthorized(
        "control pubkey not authorized".to_string(),
    ))
}

#[derive(Debug, Deserialize)]
struct ControlCreateUserRequest {
    username: String,
    email: String,
    password: String,
    full_name: Option<String>,
    must_change_password: Option<bool>,
    send_notify: Option<bool>,
}

impl From<ControlCreateUserRequest> for ForgejoCreateUser {
    fn from(value: ControlCreateUserRequest) -> Self {
        ForgejoCreateUser {
            username: value.username,
            email: value.email,
            password: value.password,
            full_name: value.full_name,
            must_change_password: value.must_change_password,
            send_notify: value.send_notify,
        }
    }
}

#[derive(Debug, Deserialize)]
struct ControlCreateOrgRequest {
    owner: String,
    name: String,
    full_name: Option<String>,
    description: Option<String>,
    visibility: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ControlCreateRepoRequest {
    owner: String,
    name: String,
    identifier: Option<String>,
    description: Option<String>,
    private: Option<bool>,
    auto_init: Option<bool>,
    pubkey: String,
    privkey: String,
}

#[derive(Debug, Deserialize)]
struct ControlCreateTenantRequest {
    host: String,
    name: Option<String>,
    description: Option<String>,
    icon: Option<String>,
    banner: Option<String>,
    contact: Option<String>,
    auth_required: Option<bool>,
    public_read: Option<bool>,
    public_write: Option<bool>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ControlCreateTenantResponse {
    tenant_id: String,
    host: String,
    relay_pubkey: String,
    relay_url: String,
    owner_pubkey: String,
    status: String,
}

#[derive(Debug, Deserialize)]
struct ControlCreatePullRequest {
    owner: String,
    repo: String,
    head: String,
    base: String,
    title: String,
    body: Option<String>,
}

#[derive(Debug, Clone)]
struct CreateRepoInput {
    owner: Option<String>,
    name: String,
    identifier: Option<String>,
    description: Option<String>,
    private: Option<bool>,
    auto_init: Option<bool>,
    pubkey: String,
    privkey: String,
}

#[derive(Debug, Deserialize)]
struct ControlEventRequest {
    kind: u64,
    pubkey: String,
    content: String,
}

#[derive(Debug, Serialize)]
struct ControlUserResponse {
    username: String,
    email: Option<String>,
}

impl From<ForgejoUser> for ControlUserResponse {
    fn from(value: ForgejoUser) -> Self {
        Self {
            username: value.username,
            email: value.email,
        }
    }
}

#[derive(Debug, Serialize)]
struct ControlOrgResponse {
    name: String,
    full_name: Option<String>,
}

impl From<ForgejoOrg> for ControlOrgResponse {
    fn from(value: ForgejoOrg) -> Self {
        Self {
            name: value.name,
            full_name: value.full_name,
        }
    }
}

#[derive(Debug, Serialize)]
struct ControlRepoResponse {
    owner: String,
    name: String,
    full_name: String,
    html_url: Option<String>,
}

impl From<ForgejoRepo> for ControlRepoResponse {
    fn from(value: ForgejoRepo) -> Self {
        Self {
            owner: value.owner,
            name: value.name,
            full_name: value.full_name,
            html_url: value.html_url,
        }
    }
}

#[derive(Debug, Serialize)]
struct ControlPullResponse {
    number: u64,
    url: String,
    html_url: Option<String>,
}

impl From<ForgejoPullRequest> for ControlPullResponse {
    fn from(value: ForgejoPullRequest) -> Self {
        Self {
            number: value.number,
            url: value.url,
            html_url: value.html_url,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(tag = "action", rename_all = "snake_case")]
enum ControlEventResponse {
    CreateUser { user: ControlUserResponse },
    CreateOrg { org: ControlOrgResponse },
    CreateRepo { repo: ControlRepoResponse },
    CreatePullRequest { pull: ControlPullResponse },
}

async fn create_user_handler(
    State(state): State<ControlAppState>,
    headers: HeaderMap,
    Json(payload): Json<ControlCreateUserRequest>,
) -> Result<Json<ControlUserResponse>, ControlHttpError> {
    authorize(&headers, &state.auth.token)?;
    require_non_empty("username", &payload.username)?;
    require_non_empty("email", &payload.email)?;
    require_non_empty("password", &payload.password)?;
    let user = state
        .forgejo
        .create_user(payload.into())
        .await
        .map_err(map_forgejo_error)?;
    Ok(Json(user.into()))
}

async fn create_org_handler(
    State(state): State<ControlAppState>,
    headers: HeaderMap,
    Json(payload): Json<ControlCreateOrgRequest>,
) -> Result<Json<ControlOrgResponse>, ControlHttpError> {
    authorize(&headers, &state.auth.token)?;
    require_non_empty("owner", &payload.owner)?;
    require_non_empty("name", &payload.name)?;
    let org = state
        .forgejo
        .create_org(
            &payload.owner,
            ForgejoCreateOrg {
                username: payload.name,
                full_name: payload.full_name,
                description: payload.description,
                visibility: payload.visibility,
            },
        )
        .await
        .map_err(map_forgejo_error)?;
    Ok(Json(org.into()))
}

async fn create_repo_handler(
    State(state): State<ControlAppState>,
    headers: HeaderMap,
    Json(payload): Json<ControlCreateRepoRequest>,
) -> Result<Json<ControlRepoResponse>, ControlHttpError> {
    authorize(&headers, &state.auth.token)?;
    require_non_empty("owner", &payload.owner)?;
    require_non_empty("name", &payload.name)?;
    require_non_empty("pubkey", &payload.pubkey)?;
    require_non_empty("privkey", &payload.privkey)?;
    if let Some(identifier) = &payload.identifier {
        require_non_empty("identifier", identifier)?;
    }
    require_hex64("pubkey", &payload.pubkey)?;
    require_hex64("privkey", &payload.privkey)?;
    authorize_admin_pubkey(&payload.pubkey, &state.auth)?;

    let input = CreateRepoInput {
        owner: Some(payload.owner),
        name: payload.name,
        identifier: payload.identifier,
        description: payload.description,
        private: payload.private,
        auto_init: payload.auto_init,
        pubkey: payload.pubkey,
        privkey: payload.privkey,
    };
    let repo = create_repo_with_announcement(&state, input).await?;
    Ok(Json(repo))
}

async fn create_repo_nostr_handler(
    State(state): State<ControlAppState>,
    headers: HeaderMap,
    method: Method,
    OriginalUri(uri): OriginalUri,
    body: Bytes,
) -> Result<Json<RepoCreateResponse>, ControlHttpError> {
    let event = parse_nostr_auth(&headers)?;
    let request_url = build_request_url(&headers, &uri)?;
    let payload_hash = nip98_payload_hash(&body);
    let now = unix_timestamp();
    let request = Nip98Request {
        method: method.as_str(),
        url: &request_url,
        payload_sha256: payload_hash.as_deref(),
        now,
        max_skew_seconds: DEFAULT_CONTROL_MAX_SKEW_SECONDS,
    };
    let auth = validate_nip98(&event, &request)
        .map_err(|err| ControlHttpError::Unauthorized(err.to_string()))?;

    if body.is_empty() {
        return Err(ControlHttpError::BadRequest(
            "missing repo create body".to_string(),
        ));
    }
    let payload: RepoCreateRequest = serde_json::from_slice(&body).map_err(|err| {
        ControlHttpError::BadRequest(format!("invalid repo create request: {err}"))
    })?;
    let repo = create_repo_from_signed_event(&state, &auth.pubkey, payload).await?;
    Ok(Json(repo))
}

async fn create_tenant_handler(
    State(state): State<ControlAppState>,
    headers: HeaderMap,
    method: Method,
    OriginalUri(uri): OriginalUri,
    body: Bytes,
) -> Result<Json<ControlCreateTenantResponse>, ControlHttpError> {
    let event = parse_nostr_auth(&headers)?;
    let request_url = build_request_url(&headers, &uri)?;
    let payload_hash = nip98_payload_hash(&body);
    let now = unix_timestamp();
    let request = Nip98Request {
        method: method.as_str(),
        url: &request_url,
        payload_sha256: payload_hash.as_deref(),
        now,
        max_skew_seconds: DEFAULT_CONTROL_MAX_SKEW_SECONDS,
    };
    let auth = validate_nip98(&event, &request)
        .map_err(|err| ControlHttpError::Unauthorized(err.to_string()))?;

    if body.is_empty() {
        return Err(ControlHttpError::BadRequest(
            "missing tenant create body".to_string(),
        ));
    }
    let payload: ControlCreateTenantRequest = serde_json::from_slice(&body).map_err(|err| {
        ControlHttpError::BadRequest(format!("invalid tenant create request: {err}"))
    })?;

    let host = normalize_host(&payload.host)?;
    require_non_empty("host", &host)?;
    if let Some(existing) = state
        .repositories
        .tenant_by_host(&host)
        .await
        .map_err(map_storage_error)?
    {
        let _ = existing;
        return Err(ControlHttpError::BadRequest(
            "tenant host already exists".to_string(),
        ));
    }

    let auth_required = payload.auth_required.unwrap_or(true);
    let public_read = payload.public_read.unwrap_or(false);
    let public_write = payload.public_write.unwrap_or(false);
    let tenant_id = host.clone();

    let secp = Secp256k1::new();
    let mut rng = OsRng;
    let secret_key = SecretKey::new(&mut rng);
    let keypair = Keypair::from_secret_key(&secp, &secret_key);
    let (relay_pubkey, _) = XOnlyPublicKey::from_keypair(&keypair);
    let relay_pubkey_hex = hex::encode(relay_pubkey.serialize());
    let mut relay_secret_nonce = vec![0u8; 24];
    rng.fill_bytes(&mut relay_secret_nonce);

    let record = RelayTenantRecord::new(
        tenant_id.clone(),
        host.clone(),
        &relay_pubkey_hex,
        secret_key.secret_bytes().to_vec(),
        relay_secret_nonce,
        DEFAULT_RELAY_SECRET_KID,
        payload.name,
        payload.description,
        payload.icon,
        payload.banner,
        payload.contact,
        auth_required,
        public_read,
        public_write,
        now,
        now,
    )
    .map_err(|err| ControlHttpError::BadRequest(err.to_string()))?;
    state
        .repositories
        .upsert_tenant(record)
        .await
        .map_err(map_storage_error)?;

    let owner_pubkey = auth.pubkey.clone();
    let owner_pubkey_bytes = hex::decode(&owner_pubkey)
        .map_err(|_| ControlHttpError::BadRequest("invalid pubkey".to_string()))?;
    let membership = RelayMembershipRecord {
        tenant_id: tenant_id.clone(),
        pubkey: owner_pubkey_bytes,
        role: "owner".to_string(),
        status: "active".to_string(),
        created_at: now,
        updated_at: now,
    };
    state
        .repositories
        .upsert_membership(membership)
        .await
        .map_err(map_storage_error)?;

    let relay_url = format!("wss://{host}");
    Ok(Json(ControlCreateTenantResponse {
        tenant_id,
        host,
        relay_pubkey: relay_pubkey_hex,
        relay_url,
        owner_pubkey,
        status: "created".to_string(),
    }))
}

async fn create_pull_handler(
    State(state): State<ControlAppState>,
    headers: HeaderMap,
    Json(payload): Json<ControlCreatePullRequest>,
) -> Result<Json<ControlPullResponse>, ControlHttpError> {
    authorize(&headers, &state.auth.token)?;
    require_non_empty("owner", &payload.owner)?;
    require_non_empty("repo", &payload.repo)?;
    require_non_empty("head", &payload.head)?;
    require_non_empty("base", &payload.base)?;
    require_non_empty("title", &payload.title)?;
    let pr = state
        .forgejo
        .create_pull_request(
            &payload.owner,
            &payload.repo,
            ForgejoCreatePullRequest {
                head: payload.head,
                base: payload.base,
                title: payload.title,
                body: payload.body,
            },
        )
        .await
        .map_err(map_forgejo_error)?;
    Ok(Json(pr.into()))
}

async fn control_event_handler(
    State(state): State<ControlAppState>,
    headers: HeaderMap,
    Json(payload): Json<ControlEventRequest>,
) -> Result<Json<ControlEventResponse>, ControlHttpError> {
    authorize(&headers, &state.auth.token)?;
    require_non_empty("pubkey", &payload.pubkey)?;
    require_non_empty("content", &payload.content)?;
    authorize_admin_pubkey(&payload.pubkey, &state.auth)?;
    let kind = u32::try_from(payload.kind)
        .map_err(|_| ControlHttpError::BadRequest("invalid kind".to_string()))?;
    let action = ControlAction::parse(kind, &payload.content, KIND_GITTREE_CONTROL.0)
        .map_err(|err| ControlHttpError::BadRequest(err.to_string()))?;
    ensure_action_pubkey_matches(&payload.pubkey, &action)?;
    let response = apply_control_action(&state, action).await?;
    Ok(Json(response))
}

async fn apply_control_action(
    state: &ControlAppState,
    action: ControlAction,
) -> Result<ControlEventResponse, ControlHttpError> {
    match action {
        ControlAction::CreateUser {
            username,
            email,
            password,
            must_change_password,
            ..
        } => {
            let user = state
                .forgejo
                .create_user(ForgejoCreateUser {
                    username,
                    email,
                    password,
                    full_name: None,
                    must_change_password,
                    send_notify: None,
                })
                .await
                .map_err(map_forgejo_error)?;
            Ok(ControlEventResponse::CreateUser {
                user: user.into(),
            })
        }
        ControlAction::CreateOrg {
            name,
            full_name,
            description,
        } => {
            let org = state
                .forgejo
                .create_org(
                    &state.forgejo_owner,
                    ForgejoCreateOrg {
                        username: name,
                        full_name,
                        description,
                        visibility: None,
                    },
                )
                .await
                .map_err(map_forgejo_error)?;
            Ok(ControlEventResponse::CreateOrg { org: org.into() })
        }
        ControlAction::CreateRepo {
            name,
            owner,
            identifier,
            description,
            private,
            pubkey,
            privkey,
        } => {
            let input = CreateRepoInput {
                owner,
                name,
                identifier,
                description,
                private,
                auto_init: None,
                pubkey,
                privkey,
            };
            let repo = create_repo_with_announcement(state, input).await?;
            Ok(ControlEventResponse::CreateRepo { repo })
        }
        ControlAction::CreatePullRequest {
            owner,
            repo,
            head,
            base,
            title,
            body,
            ..
        } => {
            let pull = state
                .forgejo
                .create_pull_request(
                    &owner,
                    &repo,
                    ForgejoCreatePullRequest {
                        head,
                        base,
                        title,
                        body,
                    },
                )
                .await
                .map_err(map_forgejo_error)?;
            Ok(ControlEventResponse::CreatePullRequest {
                pull: pull.into(),
            })
        }
    }
}

async fn create_repo_with_announcement(
    state: &ControlAppState,
    input: CreateRepoInput,
) -> Result<ControlRepoResponse, ControlHttpError> {
    let owner = input.owner.unwrap_or_else(|| state.forgejo_owner.clone());
    let identifier = input.identifier.unwrap_or_else(|| input.name.clone());

    require_non_empty("owner", &owner)?;
    require_non_empty("name", &input.name)?;
    require_non_empty("identifier", &identifier)?;
    require_hex64("pubkey", &input.pubkey)?;
    require_hex64("privkey", &input.privkey)?;
    if state.relay_urls.is_empty() {
        return Err(ControlHttpError::BadRequest(
            "missing relay urls".to_string(),
        ));
    }

    let npub = npub_from_hex(&input.pubkey)?;
    let secret_key = parse_secret_key(&input.privkey)?;
    let clone_url = format_grasp_server_url_as_clone_url(
        &state.public_git_url,
        &npub,
        &identifier,
    )
    .map_err(|err| ControlHttpError::BadRequest(err.to_string()))?;

    let announcement = RepoAnnouncement {
        identifier: identifier.clone(),
        name: Some(input.name.clone()),
        description: input.description.clone(),
        root_commit: None,
        clone: vec![clone_url],
        web: Vec::new(),
        relays: state.relay_urls.clone(),
        blossoms: Vec::new(),
        hashtags: Vec::new(),
        maintainers: vec![input.pubkey.clone()],
    };

    let signed = RelaySignedNostrEvent::from_announcement(&announcement, &secret_key)
        .map_err(|err| ControlHttpError::BadRequest(err.to_string()))?;
    if signed.pubkey != input.pubkey {
        return Err(ControlHttpError::BadRequest(
            "pubkey does not match privkey".to_string(),
        ));
    }

    let repo = state
        .forgejo
        .create_repo_for_owner(
            &owner,
            ForgejoCreateRepo {
                name: input.name,
                description: input.description,
                private: input.private,
                auto_init: input.auto_init,
            },
        )
        .await
        .map_err(map_forgejo_error)?;

    for relay_url in &state.relay_urls {
        let request = RelayPublishRequest {
            relay_url: relay_url.clone(),
            event_id: signed.id.clone(),
            pubkey: signed.pubkey.clone(),
            created_at: signed.created_at,
            kind: signed.kind,
            tags: signed.tags.clone(),
            content: signed.content.clone(),
            sig: signed.sig.clone(),
            forgejo_owner: owner.clone(),
            forgejo_repo: repo.name.clone(),
            identifier: identifier.clone(),
        };
        state
            .repositories
            .enqueue_relay_publish(request)
            .await
            .map_err(map_storage_error)?;
    }

    Ok(repo.into())
}

async fn create_repo_from_signed_event(
    state: &ControlAppState,
    auth_pubkey: &str,
    request: RepoCreateRequest,
) -> Result<RepoCreateResponse, ControlHttpError> {
    let event = request.event;
    let (announcement, identifier, _npub) = validate_repo_announcement_event(
        &event,
        auth_pubkey,
        &state.relay_urls,
        &state.public_git_url,
    )?;

    let pubkey_bytes = hex::decode(auth_pubkey)
        .map_err(|_| ControlHttpError::BadRequest("invalid pubkey".to_string()))?;
    let account = state
        .repositories
        .account_by_pubkey(&pubkey_bytes)
        .await
        .map_err(map_storage_error)?
        .ok_or_else(|| ControlHttpError::BadRequest("account not found".to_string()))?;
    let owner = account.forgejo_username;
    let private = request.private.unwrap_or(state.repo_private_default);

    let repo = state
        .forgejo
        .create_repo_for_owner(
            &owner,
            ForgejoCreateRepo {
                name: identifier.clone(),
                description: announcement.description.clone(),
                private: Some(private),
                auto_init: None,
            },
        )
        .await
        .map_err(map_forgejo_error)?;

    for relay_url in &state.relay_urls {
        let request = RelayPublishRequest {
            relay_url: relay_url.clone(),
            event_id: event.id.clone(),
            pubkey: event.pubkey.clone(),
            created_at: event.created_at,
            kind: event.kind,
            tags: event.tags.clone(),
            content: event.content.clone(),
            sig: event.sig.clone(),
            forgejo_owner: owner.clone(),
            forgejo_repo: repo.name.clone(),
            identifier: identifier.clone(),
        };
        state
            .repositories
            .enqueue_relay_publish(request)
            .await
            .map_err(map_storage_error)?;
    }

    Ok(RepoCreateResponse {
        owner,
        name: repo.name,
        html_url: repo.html_url,
    })
}

fn validate_repo_announcement_event(
    event: &ApiSignedNostrEvent,
    auth_pubkey: &str,
    relay_urls: &[String],
    public_git_url: &str,
) -> Result<(RepoAnnouncement, String, String), ControlHttpError> {
    if event.kind != KIND_GIT_REPO_ANNOUNCEMENT.0 {
        return Err(ControlHttpError::BadRequest(
            "invalid repo announcement kind".to_string(),
        ));
    }
    if event.pubkey != auth_pubkey {
        return Err(ControlHttpError::Unauthorized(
            "repo event pubkey mismatch".to_string(),
        ));
    }
    verify_signed_event(event)?;
    let announcement = RepoAnnouncement::from_tags(&event.tags)
        .map_err(|err| ControlHttpError::BadRequest(err.to_string()))?;
    announcement
        .validate()
        .map_err(|err| ControlHttpError::BadRequest(err.to_string()))?;
    if !announcement
        .maintainers
        .iter()
        .any(|key| key == &event.pubkey)
    {
        return Err(ControlHttpError::BadRequest(
            "missing maintainer pubkey".to_string(),
        ));
    }

    let identifier = normalize_identifier(&announcement.identifier).to_string();
    if identifier.trim().is_empty() {
        return Err(ControlHttpError::BadRequest(
            "invalid repo identifier".to_string(),
        ));
    }

    let npub = npub_from_hex(&event.pubkey)?;
    let expected_clone = format_grasp_server_url_as_clone_url(
        public_git_url,
        &npub,
        &identifier,
    )
    .map_err(|err| ControlHttpError::BadRequest(err.to_string()))?;
    if !announcement.clone.iter().any(|url| url == &expected_clone) {
        return Err(ControlHttpError::BadRequest(
            "missing clone url".to_string(),
        ));
    }
    for relay_url in relay_urls {
        if !announcement.relays.iter().any(|url| url == relay_url) {
            return Err(ControlHttpError::BadRequest(
                "missing relay url".to_string(),
            ));
        }
    }

    Ok((announcement, identifier, npub))
}

fn ensure_action_pubkey_matches(
    request_pubkey: &str,
    action: &ControlAction,
) -> Result<(), ControlHttpError> {
    if let ControlAction::CreateRepo { pubkey, .. } = action {
        if pubkey != request_pubkey {
            return Err(ControlHttpError::BadRequest(
                "pubkey does not match control request".to_string(),
            ));
        }
    }
    Ok(())
}

fn build_request_url(headers: &HeaderMap, uri: &Uri) -> Result<String, ControlHttpError> {
    let host = headers
        .get("host")
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| ControlHttpError::BadRequest("missing host header".to_string()))?;
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

fn normalize_host(value: &str) -> Result<String, ControlHttpError> {
    let value = value.trim().trim_end_matches('.');
    let value = value
        .strip_prefix("https://")
        .or_else(|| value.strip_prefix("http://"))
        .unwrap_or(value);
    if value.contains('/') {
        return Err(ControlHttpError::BadRequest("invalid host".to_string()));
    }
    let normalized = if let Some(value) = value.strip_prefix('[') {
        if let Some(end) = value.find(']') {
            value[..end].to_ascii_lowercase()
        } else {
            return Err(ControlHttpError::BadRequest("invalid host".to_string()));
        }
    } else {
        value
            .split(':')
            .next()
            .unwrap_or(value)
            .to_ascii_lowercase()
    };
    if normalized.trim().is_empty() {
        return Err(ControlHttpError::BadRequest("invalid host".to_string()));
    }
    Ok(normalized)
}

fn parse_nostr_auth(headers: &HeaderMap) -> Result<Nip98Event, ControlHttpError> {
    let value = headers
        .get(AUTH_HEADER)
        .and_then(|header| header.to_str().ok())
        .ok_or_else(|| ControlHttpError::Unauthorized("missing authorization".to_string()))?;
    let value = value.trim();
    let Some(token) = value.strip_prefix("Nostr ") else {
        return Err(ControlHttpError::Unauthorized(
            "invalid authorization header".to_string(),
        ));
    };
    let decoded = BASE64_STANDARD
        .decode(token.as_bytes())
        .map_err(|_| ControlHttpError::Unauthorized("invalid nostr authorization".to_string()))?;
    serde_json::from_slice::<Nip98Event>(&decoded)
        .map_err(|_| ControlHttpError::Unauthorized("invalid nostr event".to_string()))
}

fn verify_signed_event(event: &ApiSignedNostrEvent) -> Result<(), ControlHttpError> {
    require_hex_len("event.id", &event.id, 64)?;
    require_hex_len("event.pubkey", &event.pubkey, 64)?;
    require_hex_len("event.sig", &event.sig, 128)?;

    let expected_id = build_event_id(event)?;
    if expected_id != event.id {
        return Err(ControlHttpError::BadRequest(
            "event id mismatch".to_string(),
        ));
    }

    let event_id = hex::decode(&event.id)
        .map_err(|_| ControlHttpError::BadRequest("invalid event id".to_string()))?;
    let msg = Message::from_digest_slice(&event_id)
        .map_err(|_| ControlHttpError::BadRequest("invalid event id".to_string()))?;
    let sig_bytes = hex::decode(&event.sig)
        .map_err(|_| ControlHttpError::BadRequest("invalid event sig".to_string()))?;
    let sig = secp256k1::schnorr::Signature::from_slice(&sig_bytes)
        .map_err(|_| ControlHttpError::BadRequest("invalid event sig".to_string()))?;
    let pubkey_bytes = hex::decode(&event.pubkey)
        .map_err(|_| ControlHttpError::BadRequest("invalid pubkey".to_string()))?;
    let pubkey = XOnlyPublicKey::from_slice(&pubkey_bytes)
        .map_err(|_| ControlHttpError::BadRequest("invalid pubkey".to_string()))?;
    let secp = Secp256k1::new();
    secp.verify_schnorr(&sig, &msg, &pubkey)
        .map_err(|_| ControlHttpError::BadRequest("invalid event sig".to_string()))?;
    Ok(())
}

fn build_event_id(event: &ApiSignedNostrEvent) -> Result<String, ControlHttpError> {
    let payload = serde_json::json!([
        0,
        event.pubkey,
        event.created_at,
        event.kind,
        event.tags,
        event.content
    ]);
    let serialized = serde_json::to_string(&payload)
        .map_err(|err| ControlHttpError::BadRequest(err.to_string()))?;
    let mut hasher = sha2::Sha256::new();
    hasher.update(serialized.as_bytes());
    let digest = hasher.finalize();
    Ok(hex::encode(digest))
}

fn unix_timestamp() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn require_non_empty(field: &'static str, value: &str) -> Result<(), ControlHttpError> {
    if value.trim().is_empty() {
        return Err(ControlHttpError::BadRequest(format!(
            "missing {field}"
        )));
    }
    Ok(())
}

fn require_hex_len(
    field: &'static str,
    value: &str,
    len: usize,
) -> Result<(), ControlHttpError> {
    if value.len() != len || !is_hex(value) {
        return Err(ControlHttpError::BadRequest(format!(
            "invalid {field}"
        )));
    }
    Ok(())
}

fn require_hex64(field: &'static str, value: &str) -> Result<(), ControlHttpError> {
    if value.len() != 64 || !is_hex(value) {
        return Err(ControlHttpError::BadRequest(format!(
            "invalid {field}"
        )));
    }
    Ok(())
}

fn is_hex(value: &str) -> bool {
    value.as_bytes().iter().all(|b| b.is_ascii_hexdigit())
}

fn parse_secret_key(value: &str) -> Result<SecretKey, ControlHttpError> {
    if value.len() != 64 {
        return Err(ControlHttpError::BadRequest(
            "invalid privkey".to_string(),
        ));
    }
    let bytes = hex::decode(value)
        .map_err(|_| ControlHttpError::BadRequest("invalid privkey".to_string()))?;
    SecretKey::from_slice(&bytes)
        .map_err(|_| ControlHttpError::BadRequest("invalid privkey".to_string()))
}

fn npub_from_hex(pubkey: &str) -> Result<String, ControlHttpError> {
    if pubkey.len() != 64 {
        return Err(ControlHttpError::BadRequest("invalid pubkey".to_string()));
    }
    let bytes = hex::decode(pubkey)
        .map_err(|_| ControlHttpError::BadRequest("invalid pubkey".to_string()))?;
    let hrp = Hrp::parse("npub")
        .map_err(|_| ControlHttpError::Internal("npub hrp parse failed".to_string()))?;
    bech32::encode::<Bech32>(hrp, &bytes)
        .map_err(|_| ControlHttpError::Internal("npub encode failed".to_string()))
}

fn map_forgejo_error(error: ForgejoError) -> ControlHttpError {
    match error {
        ForgejoError::Response { status, body } if status >= 400 && status < 500 => {
            ControlHttpError::BadRequest(format!("forgejo error {status}: {body}"))
        }
        err => ControlHttpError::Internal(err.to_string()),
    }
}

fn map_storage_error(error: StorageError) -> ControlHttpError {
    ControlHttpError::Internal(error.to_string())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "error", rename_all = "snake_case")]
pub enum ControlHttpError {
    Unauthorized(String),
    BadRequest(String),
    Internal(String),
}

impl IntoResponse for ControlHttpError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            ControlHttpError::Unauthorized(message) => (StatusCode::UNAUTHORIZED, message),
            ControlHttpError::BadRequest(message) => (StatusCode::BAD_REQUEST, message),
            ControlHttpError::Internal(message) => (StatusCode::INTERNAL_SERVER_ERROR, message),
        };
        (status, message).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ControlCreateTenantResponse, AUTH_HEADER, ControlConfig, ControlHttpError, authorize,
        authorize_admin_pubkey, build_request_url, build_router, normalize_host, npub_from_hex,
        parse_nostr_auth, parse_secret_key,
    };
    use async_trait::async_trait;
    use axum::body::{Body, to_bytes};
    use axum::http::{HeaderMap, Request, StatusCode};
    use axum::response::IntoResponse;
    use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
    use base64::Engine;
    use gittree_app_core::{
        nip98_payload_hash, nip98_sign_event, pubkey_bytes_from_npub, RepoCreateRequest,
        RepoCreateResponse,
        SignedNostrEvent as ApiSignedNostrEvent,
    };
    use gittree_config::{ControlAuthConfig, ForgejoConfig};
    use gittree_core::kinds::KIND_GITTREE_CONTROL;
    use gittree_core::{RepoAnnouncement, format_grasp_server_url_as_clone_url};
    use gittree_forgejo::{
        ForgejoClient, ForgejoError, ForgejoMethod, ForgejoRequest, ForgejoResponse,
        ForgejoTransport,
    };
    use gittree_relay_adapter::SignedNostrEvent as RelaySignedNostrEvent;
    use gittree_storage::{
        AccountRecord, AccountRepository, InMemoryRepositories, RelayMembershipRepository,
        RelayPublishRepository, RelayTenantRepository, StorageError,
    };
    use secp256k1::{Keypair, Secp256k1, SecretKey, XOnlyPublicKey};
    use serde_json::json;
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};
    use time::OffsetDateTime;
    use tower::ServiceExt;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[derive(Clone, Default)]
    struct MockTransport {
        requests: Arc<Mutex<Vec<ForgejoRequest>>>,
        responses: Arc<Mutex<VecDeque<ForgejoResponse>>>,
    }

    impl MockTransport {
        fn new(responses: Vec<ForgejoResponse>) -> Self {
            Self {
                requests: Arc::new(Mutex::new(Vec::new())),
                responses: Arc::new(Mutex::new(VecDeque::from(responses))),
            }
        }

        fn requests(&self) -> Vec<ForgejoRequest> {
            self.requests.lock().expect("requests").clone()
        }
    }

    #[async_trait]
    impl ForgejoTransport for MockTransport {
        async fn send(&self, request: ForgejoRequest) -> Result<ForgejoResponse, gittree_forgejo::ForgejoError> {
            self.requests.lock().expect("requests").push(request);
            self.responses
                .lock()
                .expect("responses")
                .pop_front()
                .ok_or_else(|| gittree_forgejo::ForgejoError::Request("missing mock response".to_string()))
        }
    }

    fn test_config() -> ForgejoConfig {
        ForgejoConfig {
            base_url: "http://localhost:3000".to_string(),
            api_token: "token".to_string(),
            owner: "gittree".to_string(),
            webhook_url: "http://localhost:8087/".to_string(),
            webhook_secret: "secret".to_string(),
            repo_private: true,
        }
    }

    fn test_state(
        responses: Vec<ForgejoResponse>,
    ) -> (
        super::ControlAppState,
        Arc<MockTransport>,
        Arc<InMemoryRepositories>,
    ) {
        test_state_with_auth(responses, Vec::new(), "gittree")
    }

    fn test_state_with_auth(
        responses: Vec<ForgejoResponse>,
        admin_keys: Vec<String>,
        owner: &str,
    ) -> (
        super::ControlAppState,
        Arc<MockTransport>,
        Arc<InMemoryRepositories>,
    ) {
        let transport = Arc::new(MockTransport::new(responses));
        let transport_dyn: Arc<dyn ForgejoTransport> = transport.clone();
        let client = ForgejoClient::with_transport(test_config(), transport_dyn);
        let repositories = Arc::new(InMemoryRepositories::new());
        let relay_urls = vec!["ws://relay.local".to_string()];
        (
            super::ControlAppState {
                auth: ControlAuthConfig {
                    token: "token".to_string(),
                    admin_keys,
                },
                forgejo: client,
                forgejo_owner: owner.to_string(),
                repositories: repositories.clone(),
                relay_urls,
                public_git_url: "http://localhost:8085".to_string(),
                repo_private_default: true,
            },
            transport,
            repositories,
        )
    }

    fn test_keys() -> (String, String) {
        let secret = SecretKey::from_slice(&[1u8; 32]).expect("secret");
        let secp = Secp256k1::new();
        let keypair = Keypair::from_secret_key(&secp, &secret);
        let (pubkey, _) = XOnlyPublicKey::from_keypair(&keypair);
        (
            hex::encode(pubkey.serialize()),
            hex::encode(secret.secret_bytes()),
        )
    }

    fn api_event_from_relay(event: RelaySignedNostrEvent) -> ApiSignedNostrEvent {
        ApiSignedNostrEvent {
            id: event.id,
            pubkey: event.pubkey,
            created_at: event.created_at,
            kind: event.kind,
            tags: event.tags,
            content: event.content,
            sig: event.sig,
        }
    }

    fn nostr_auth_header(event: &gittree_app_core::Nip98Event) -> String {
        let payload = serde_json::to_vec(event).expect("serialize");
        let token = BASE64_STANDARD.encode(payload);
        format!("Nostr {token}")
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

    fn without_env_var<F: FnOnce()>(key: &str, f: F) {
        let previous = std::env::var_os(key);
        unsafe {
            std::env::remove_var(key);
        }
        f();
        if let Some(old) = previous {
            unsafe {
                std::env::set_var(key, old);
            }
        }
    }

    #[test]
    fn config_loads_from_env() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var("GITTREE_CONTROL_TOKEN", "token", || {
            with_env_var("GITTREE_FORGEJO_BASE_URL", "http://localhost:3000", || {
                with_env_var("GITTREE_FORGEJO_API_TOKEN", "token", || {
                    with_env_var("GITTREE_FORGEJO_OWNER", "gittree", || {
                        with_env_var("GITTREE_FORGEJO_WEBHOOK_URL", "http://localhost:8087/", || {
                            with_env_var("GITTREE_FORGEJO_WEBHOOK_SECRET", "secret", || {
                                with_env_var(
                                    "GITTREE_STORAGE_READ_URL",
                                    "postgres://user:pass@localhost:5432/gittree",
                                    || {
                                        with_env_var("GITTREE_RELAY_URLS", "ws://relay.local", || {
                                            with_env_var("GITTREE_UI_REPO_ROOT", "/tmp/repos", || {
                                                with_env_var(
                                                    "GITTREE_UI_PUBLIC_GIT_URL",
                                                    "http://localhost:8085",
                                                    || {
                                                        let config =
                                                            ControlConfig::from_env().expect("config");
                                                        assert!(!config.bind.is_empty());
                                                    },
                                                );
                                            });
                                        });
                                    },
                                );
                            });
                        });
                    });
                });
            });
        });
    }

    #[test]
    fn config_rejects_missing_storage_read_url() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var("GITTREE_CONTROL_TOKEN", "token", || {
            with_env_var("GITTREE_FORGEJO_BASE_URL", "http://localhost:3000", || {
                with_env_var("GITTREE_FORGEJO_API_TOKEN", "token", || {
                    with_env_var("GITTREE_FORGEJO_OWNER", "gittree", || {
                        with_env_var("GITTREE_FORGEJO_WEBHOOK_URL", "http://localhost:8087/", || {
                            with_env_var("GITTREE_FORGEJO_WEBHOOK_SECRET", "secret", || {
                                with_env_var("GITTREE_RELAY_URLS", "ws://relay.local", || {
                                    with_env_var("GITTREE_UI_REPO_ROOT", "/tmp/repos", || {
                                        with_env_var(
                                            "GITTREE_UI_PUBLIC_GIT_URL",
                                            "http://localhost:8085",
                                            || {
                                                without_env_var("GITTREE_STORAGE_READ_URL", || {
                                                    let err =
                                                        ControlConfig::from_env().expect_err("config");
                                                    assert!(matches!(
                                                        err,
                                                        super::ControlConfigError::Storage(
                                                            super::StorageConfigError::MissingEnv(
                                                                "GITTREE_STORAGE_READ_URL"
                                                            )
                                                        )
                                                    ));
                                                });
                                            },
                                        );
                                    });
                                });
                            });
                        });
                    });
                });
            });
        });
    }

    #[test]
    fn config_rejects_invalid_storage_numeric_override() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var("GITTREE_CONTROL_TOKEN", "token", || {
            with_env_var("GITTREE_FORGEJO_BASE_URL", "http://localhost:3000", || {
                with_env_var("GITTREE_FORGEJO_API_TOKEN", "token", || {
                    with_env_var("GITTREE_FORGEJO_OWNER", "gittree", || {
                        with_env_var("GITTREE_FORGEJO_WEBHOOK_URL", "http://localhost:8087/", || {
                            with_env_var("GITTREE_FORGEJO_WEBHOOK_SECRET", "secret", || {
                                with_env_var(
                                    "GITTREE_STORAGE_READ_URL",
                                    "postgres://user:pass@localhost:5432/gittree",
                                    || {
                                        with_env_var(
                                            "GITTREE_STORAGE_MAX_CONNECTIONS",
                                            "bad",
                                            || {
                                                with_env_var("GITTREE_RELAY_URLS", "ws://relay.local", || {
                                                    with_env_var("GITTREE_UI_REPO_ROOT", "/tmp/repos", || {
                                                        with_env_var(
                                                            "GITTREE_UI_PUBLIC_GIT_URL",
                                                            "http://localhost:8085",
                                                            || {
                                                                let err =
                                                                    ControlConfig::from_env().expect_err("config");
                                                                assert!(matches!(
                                                                    err,
                                                                    super::ControlConfigError::Storage(
                                                                        super::StorageConfigError::InvalidEnv {
                                                                            key: "GITTREE_STORAGE_MAX_CONNECTIONS",
                                                                            ..
                                                                        }
                                                                    )
                                                                ));
                                                            },
                                                        );
                                                    });
                                                });
                                            },
                                        );
                                    },
                                );
                            });
                        });
                    });
                });
            });
        });
    }

    #[test]
    fn init_observability_reports_config_error_for_invalid_env() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var("GITTREE_LOG_JSON", "not-a-bool", || {
            let err = super::init_observability().expect_err("invalid observability env");
            assert!(matches!(err, super::ControlError::ObservabilityConfig(_)));
        });
    }

    #[test]
    fn serve_reports_bind_error_for_invalid_bind() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var("GITTREE_LOG_JSON", "false", || {
            let config = ControlConfig {
                bind: "127.0.0.1:99999".to_string(),
                auth: ControlAuthConfig {
                    token: "token".to_string(),
                    admin_keys: Vec::new(),
                },
                forgejo: test_config(),
                storage: gittree_storage::StorageConfig {
                    read_connection: "postgres://user:pass@localhost:5432/gittree".to_string(),
                    write_connection: None,
                    max_connections: 10,
                    min_connections: 2,
                    idle_timeout_secs: None,
                    max_lifetime_secs: None,
                    application_name: None,
                },
                relay_urls: vec!["ws://relay.local".to_string()],
                public_git_url: "http://localhost:8085".to_string(),
            };
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("runtime");
            let err = runtime
                .block_on(async { super::serve(config).await })
                .expect_err("invalid bind should fail");
            assert!(matches!(err, super::ControlError::Serve(_)));
        });
    }

    #[test]
    fn authorize_accepts_bearer_token() {
        let mut headers = HeaderMap::new();
        headers.insert(AUTH_HEADER, "Bearer token".parse().expect("header"));
        authorize(&headers, "token").expect("auth");
    }

    #[test]
    fn authorize_rejects_missing_header() {
        let headers = HeaderMap::new();
        let err = authorize(&headers, "token").unwrap_err();
        assert!(matches!(err, ControlHttpError::Unauthorized(_)));
    }

    #[test]
    fn authorize_rejects_invalid_header_prefix() {
        let mut headers = HeaderMap::new();
        headers.insert(AUTH_HEADER, "Nostr token".parse().expect("header"));
        let err = authorize(&headers, "token").unwrap_err();
        assert!(matches!(err, ControlHttpError::Unauthorized(_)));
    }

    #[test]
    fn authorize_rejects_wrong_token() {
        let mut headers = HeaderMap::new();
        headers.insert(AUTH_HEADER, "Bearer wrong".parse().expect("header"));
        let err = authorize(&headers, "token").unwrap_err();
        assert!(matches!(err, ControlHttpError::Unauthorized(_)));
    }

    #[test]
    fn authorize_admin_pubkey_handles_allowlist() {
        let auth = ControlAuthConfig {
            token: "token".to_string(),
            admin_keys: vec!["aa".repeat(32)],
        };
        let denied = authorize_admin_pubkey(&"bb".repeat(32), &auth).unwrap_err();
        assert!(matches!(denied, ControlHttpError::Unauthorized(_)));

        let open = ControlAuthConfig {
            token: "token".to_string(),
            admin_keys: Vec::new(),
        };
        authorize_admin_pubkey(&"bb".repeat(32), &open).expect("open allowlist");
    }

    #[test]
    fn build_request_url_and_host_normalization_cover_edge_cases() {
        let mut headers = HeaderMap::new();
        headers.insert("host", "gittr.ee".parse().expect("host"));
        headers.insert("x-forwarded-proto", "https".parse().expect("proto"));
        let uri = "/v1/repos?x=1".parse().expect("uri");
        let url = build_request_url(&headers, &uri).expect("url");
        assert_eq!(url, "https://gittr.ee/v1/repos?x=1");

        let normalized = normalize_host("https://Relay.Local:443").expect("host");
        assert_eq!(normalized, "relay.local");
        let invalid = normalize_host("relay.local/path").unwrap_err();
        assert!(matches!(invalid, ControlHttpError::BadRequest(_)));
    }

    #[test]
    fn build_request_url_rejects_missing_host_header() {
        let headers = HeaderMap::new();
        let uri = "/v1/repos".parse().expect("uri");
        let err = build_request_url(&headers, &uri).expect_err("missing host");
        assert!(matches!(err, ControlHttpError::BadRequest(_)));
    }

    #[test]
    fn parse_nostr_auth_and_secret_key_validate_inputs() {
        let mut headers = HeaderMap::new();
        headers.insert(AUTH_HEADER, "Nostr !!!".parse().expect("header"));
        let auth_err = parse_nostr_auth(&headers).unwrap_err();
        assert!(matches!(auth_err, ControlHttpError::Unauthorized(_)));

        let secret_err = parse_secret_key("bad").unwrap_err();
        assert!(matches!(secret_err, ControlHttpError::BadRequest(_)));
    }

    #[test]
    fn require_hex_len_and_npub_round_trip_cover_validation_edges() {
        let (pubkey, _) = test_keys();
        super::require_hex_len("pubkey", &pubkey, 64).expect("valid hex");

        let short_err = super::require_hex_len("pubkey", "aa", 64).unwrap_err();
        assert!(matches!(short_err, ControlHttpError::BadRequest(_)));

        let non_hex_err = super::require_hex_len("pubkey", &"zz".repeat(32), 64).unwrap_err();
        assert!(matches!(non_hex_err, ControlHttpError::BadRequest(_)));

        let npub = npub_from_hex(&pubkey).expect("npub");
        let decoded = pubkey_bytes_from_npub(&npub).expect("decode");
        assert_eq!(hex::encode(decoded), pubkey);
    }

    #[test]
    fn parse_secret_key_and_unix_timestamp_cover_success_and_failure() {
        let (_, privkey) = test_keys();
        parse_secret_key(&privkey).expect("valid secret");

        let invalid = parse_secret_key(&"gg".repeat(32)).unwrap_err();
        assert!(matches!(invalid, ControlHttpError::BadRequest(_)));

        let now = super::unix_timestamp();
        assert!(now >= 1_600_000_000);
    }

    #[test]
    fn error_display_and_source_cover_variants() {
        use std::error::Error as _;

        let config_variant = super::ControlConfigError::Config(gittree_config::ConfigError::InvalidConfig {
            field: "x",
            value: "y".to_string(),
        });
        assert!(config_variant.to_string().contains("control config error"));
        assert!(config_variant.source().is_some());

        let storage_variant =
            super::ControlConfigError::Storage(super::StorageConfigError::InvalidConfig(
                "bad storage".to_string(),
            ));
        assert!(storage_variant
            .to_string()
            .contains("control storage config error"));
        assert!(storage_variant.source().is_some());

        let storage_missing = super::StorageConfigError::MissingEnv("KEY");
        assert!(storage_missing.to_string().contains("missing env KEY"));
        let storage_invalid = super::StorageConfigError::InvalidEnv {
            key: "K",
            value: "V".to_string(),
        };
        assert!(storage_invalid.to_string().contains("invalid env K"));

        let control_config = super::ControlError::Config(config_variant);
        assert!(control_config.to_string().contains("control error"));
        assert!(control_config.source().is_some());

        let control_forgejo = super::ControlError::Forgejo(ForgejoError::Request("x".to_string()));
        assert!(control_forgejo.to_string().contains("control forgejo error"));
        assert!(control_forgejo.source().is_some());

        let control_obs_cfg = super::ControlError::ObservabilityConfig(
            gittree_observability::ObservabilityConfigError::InvalidEnv {
                key: "OBS",
                value: "bad".to_string(),
            },
        );
        assert!(control_obs_cfg
            .to_string()
            .contains("control observability config error"));
        assert!(control_obs_cfg.source().is_some());

        let control_obs = super::ControlError::Observability(
            gittree_observability::ObservabilityError::LogInit("x".to_string()),
        );
        assert!(control_obs.to_string().contains("control observability error"));
        assert!(control_obs.source().is_some());

        let control_storage = super::ControlError::Storage(StorageError::Internal {
            message: "boom".to_string(),
        });
        assert!(control_storage.to_string().contains("control storage error"));
        assert!(control_storage.source().is_some());

        let control_serve = super::ControlError::Serve("bind failed".to_string());
        assert!(control_serve.to_string().contains("control serve error"));
        assert!(control_serve.source().is_none());
    }

    #[test]
    fn parse_nostr_auth_rejects_non_nostr_prefix() {
        let mut headers = HeaderMap::new();
        headers.insert(AUTH_HEADER, "Bearer abc".parse().expect("header"));
        let err = parse_nostr_auth(&headers).unwrap_err();
        assert!(matches!(err, ControlHttpError::Unauthorized(_)));
    }

    #[test]
    fn normalize_host_rejects_malformed_ipv6_host() {
        let err = normalize_host("http://[::1").unwrap_err();
        assert!(matches!(err, ControlHttpError::BadRequest(_)));
    }

    #[test]
    fn normalize_host_accepts_ipv6_and_strips_brackets() {
        let host = normalize_host("http://[2001:db8::1]:443").expect("host");
        assert_eq!(host, "2001:db8::1");
    }

    #[test]
    fn storage_env_helpers_handle_empty_and_invalid_values() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var(super::ENV_STORAGE_MAX_CONNECTIONS, " ", || {
            let value = super::env_u32(super::ENV_STORAGE_MAX_CONNECTIONS).expect("env_u32");
            assert!(value.is_none());
        });
        with_env_var(super::ENV_STORAGE_IDLE_TIMEOUT_SECS, " ", || {
            let value = super::env_u64(super::ENV_STORAGE_IDLE_TIMEOUT_SECS).expect("env_u64");
            assert!(value.is_none());
        });
        with_env_var(super::ENV_STORAGE_IDLE_TIMEOUT_SECS, "bad", || {
            let err = super::env_u64(super::ENV_STORAGE_IDLE_TIMEOUT_SECS).unwrap_err();
            assert!(matches!(
                err,
                super::ControlConfigError::Storage(super::StorageConfigError::InvalidEnv {
                    key: super::ENV_STORAGE_IDLE_TIMEOUT_SECS,
                    ..
                })
            ));
        });
    }

    #[test]
    fn storage_from_env_reports_invalid_pool_bounds() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var(super::ENV_STORAGE_READ_URL, "postgres://user:pass@localhost:5432/gittree", || {
            with_env_var(super::ENV_STORAGE_MAX_CONNECTIONS, "1", || {
                with_env_var(super::ENV_STORAGE_MIN_CONNECTIONS, "2", || {
                    let err = super::storage_from_env().expect_err("invalid pool bounds");
                    assert!(matches!(
                        err,
                        super::ControlConfigError::Storage(super::StorageConfigError::InvalidConfig(_))
                    ));
                });
            });
        });
    }

    #[test]
    fn build_repositories_maps_invalid_pool_config_to_control_error() {
        let config = super::ControlConfig {
            bind: "127.0.0.1:0".to_string(),
            auth: ControlAuthConfig {
                token: "token".to_string(),
                admin_keys: Vec::new(),
            },
            forgejo: test_config(),
            storage: gittree_storage::StorageConfig {
                read_connection: "postgres://user:pass@localhost:5432/gittree".to_string(),
                write_connection: None,
                max_connections: 1,
                min_connections: 2,
                idle_timeout_secs: None,
                max_lifetime_secs: None,
                application_name: None,
            },
            relay_urls: vec!["ws://relay.local".to_string()],
            public_git_url: "http://localhost:8085".to_string(),
        };
        let err = super::build_repositories(&config).expect_err("invalid storage config");
        assert!(matches!(err, super::ControlError::Storage(_)));
    }

    #[test]
    fn map_storage_error_maps_to_internal_http_error() {
        let err = super::map_storage_error(StorageError::Internal {
            message: "storage exploded".to_string(),
        });
        assert!(matches!(err, ControlHttpError::Internal(_)));
    }

    #[test]
    fn without_env_var_restores_existing_value() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        let key = "GITTREE_CONTROL_TEST_RESTORE";
        unsafe {
            std::env::set_var(key, "before");
        }
        without_env_var(key, || {
            assert!(std::env::var(key).is_err());
        });
        assert_eq!(std::env::var(key).expect("restored"), "before");
        unsafe {
            std::env::remove_var(key);
        }
    }

    #[test]
    fn with_env_var_restores_existing_value() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        let key = "GITTREE_CONTROL_TEST_SET_RESTORE";
        unsafe {
            std::env::set_var(key, "before");
        }
        with_env_var(key, "during", || {
            assert_eq!(std::env::var(key).expect("set"), "during");
        });
        assert_eq!(std::env::var(key).expect("restored"), "before");
        unsafe {
            std::env::remove_var(key);
        }
    }

    #[tokio::test]
    async fn create_repo_nostr_rejects_invalid_json_body() {
        let (state, _transport, _repos) = test_state(Vec::new());
        let (_pubkey, privkey) = test_keys();
        let now = super::unix_timestamp();
        let secret = parse_secret_key(&privkey).expect("secret");
        let body = b"{not-json".to_vec();
        let auth_event = nip98_sign_event(
            &secret.secret_bytes(),
            "POST",
            "http://localhost/v1/repos",
            nip98_payload_hash(&body).as_deref(),
            now,
        )
        .expect("auth");
        let response = build_router(state)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/repos")
                    .header("host", "localhost")
                    .header("content-type", "application/json")
                    .header(AUTH_HEADER, nostr_auth_header(&auth_event))
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn create_repo_rejects_missing_relay_urls() {
        let responses = vec![ForgejoResponse {
            status: 201,
            body: r#"{"full_name":"alice/demo","name":"demo","owner":{"username":"alice"},"html_url":"http://localhost/alice/demo"}"#.to_string(),
        }];
        let (mut state, _transport, _repos) = test_state(responses);
        state.relay_urls.clear();
        let (pubkey, privkey) = test_keys();
        let app = build_router(state);
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/control/repos")
                    .header("content-type", "application/json")
                    .header(AUTH_HEADER, "Bearer token")
                    .body(Body::from(
                        serde_json::to_vec(&json!({
                            "owner":"alice",
                            "name":"demo",
                            "pubkey": pubkey,
                            "privkey": privkey
                        }))
                        .expect("body"),
                    ))
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), axum::http::StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn create_repo_nostr_rejects_missing_maintainer_pubkey() {
        let (state, _transport, _repos) = test_state(Vec::new());
        let (pubkey, privkey) = test_keys();
        let now = super::unix_timestamp();
        let secret = parse_secret_key(&privkey).expect("secret");
        let npub = npub_from_hex(&pubkey).expect("npub");
        let clone_url = format_grasp_server_url_as_clone_url(&state.public_git_url, &npub, "demo")
            .expect("clone");
        let announcement = RepoAnnouncement {
            identifier: "demo".to_string(),
            name: Some("Demo".to_string()),
            description: Some("missing maintainer".to_string()),
            root_commit: None,
            clone: vec![clone_url],
            web: Vec::new(),
            relays: state.relay_urls.clone(),
            blossoms: Vec::new(),
            hashtags: Vec::new(),
            maintainers: vec!["11".repeat(32)],
        };
        let event = api_event_from_relay(
            RelaySignedNostrEvent::from_announcement_with_created_at(&announcement, &secret, now)
                .expect("signed"),
        );
        let body = serde_json::to_vec(&RepoCreateRequest {
            event,
            private: Some(false),
        })
        .expect("body");
        let auth_event = nip98_sign_event(
            &secret.secret_bytes(),
            "POST",
            "http://localhost/v1/repos",
            nip98_payload_hash(&body).as_deref(),
            now,
        )
        .expect("auth");
        let response = build_router(state)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/repos")
                    .header("host", "localhost")
                    .header("content-type", "application/json")
                    .header(AUTH_HEADER, nostr_auth_header(&auth_event))
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn create_repo_nostr_rejects_identifier_that_normalizes_empty() {
        let (state, _transport, _repos) = test_state(Vec::new());
        let (pubkey, privkey) = test_keys();
        let now = super::unix_timestamp();
        let secret = parse_secret_key(&privkey).expect("secret");
        let npub = npub_from_hex(&pubkey).expect("npub");
        let clone_url = format_grasp_server_url_as_clone_url(&state.public_git_url, &npub, ".git")
            .expect("clone");
        let announcement = RepoAnnouncement {
            identifier: ".git".to_string(),
            name: Some("Demo".to_string()),
            description: Some("invalid identifier".to_string()),
            root_commit: None,
            clone: vec![clone_url],
            web: Vec::new(),
            relays: state.relay_urls.clone(),
            blossoms: Vec::new(),
            hashtags: Vec::new(),
            maintainers: vec![pubkey.clone()],
        };
        let event = api_event_from_relay(
            RelaySignedNostrEvent::from_announcement_with_created_at(&announcement, &secret, now)
                .expect("signed"),
        );
        let body = serde_json::to_vec(&RepoCreateRequest {
            event,
            private: Some(false),
        })
        .expect("body");
        let auth_event = nip98_sign_event(
            &secret.secret_bytes(),
            "POST",
            "http://localhost/v1/repos",
            nip98_payload_hash(&body).as_deref(),
            now,
        )
        .expect("auth");
        let response = build_router(state)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/repos")
                    .header("host", "localhost")
                    .header("content-type", "application/json")
                    .header(AUTH_HEADER, nostr_auth_header(&auth_event))
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn create_repo_nostr_rejects_missing_expected_clone_url() {
        let (state, _transport, _repos) = test_state(Vec::new());
        let (pubkey, privkey) = test_keys();
        let now = super::unix_timestamp();
        let secret = parse_secret_key(&privkey).expect("secret");
        let announcement = RepoAnnouncement {
            identifier: "demo".to_string(),
            name: Some("Demo".to_string()),
            description: Some("missing clone url".to_string()),
            root_commit: None,
            clone: vec!["https://example.com/repo.git".to_string()],
            web: Vec::new(),
            relays: state.relay_urls.clone(),
            blossoms: Vec::new(),
            hashtags: Vec::new(),
            maintainers: vec![pubkey.clone()],
        };
        let event = api_event_from_relay(
            RelaySignedNostrEvent::from_announcement_with_created_at(&announcement, &secret, now)
                .expect("signed"),
        );
        let body = serde_json::to_vec(&RepoCreateRequest {
            event,
            private: Some(false),
        })
        .expect("body");
        let auth_event = nip98_sign_event(
            &secret.secret_bytes(),
            "POST",
            "http://localhost/v1/repos",
            nip98_payload_hash(&body).as_deref(),
            now,
        )
        .expect("auth");
        let response = build_router(state)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/repos")
                    .header("host", "localhost")
                    .header("content-type", "application/json")
                    .header(AUTH_HEADER, nostr_auth_header(&auth_event))
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn create_tenant_rejects_missing_body() {
        let (state, _transport, _repos) = test_state(Vec::new());
        let (_pubkey, privkey) = test_keys();
        let now = super::unix_timestamp();
        let secret = parse_secret_key(&privkey).expect("secret");
        let auth_event = nip98_sign_event(
            &secret.secret_bytes(),
            "POST",
            "http://localhost/v1/relay/tenants",
            None,
            now,
        )
        .expect("auth");
        let response = build_router(state)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/relay/tenants")
                    .header("host", "localhost")
                    .header(AUTH_HEADER, nostr_auth_header(&auth_event))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn mock_transport_returns_request_error_when_responses_are_exhausted() {
        let transport = MockTransport::new(Vec::new());
        let err = transport
            .send(ForgejoRequest {
                method: ForgejoMethod::Get,
                url: "http://localhost/api/v1/user".to_string(),
                body: None,
            })
            .await
            .expect_err("expected missing mock response");
        assert!(matches!(err, ForgejoError::Request(_)));
    }

    #[test]
    fn map_forgejo_error_maps_client_and_server_failures() {
        let client = super::map_forgejo_error(ForgejoError::Response {
            status: 404,
            body: "missing".to_string(),
        });
        assert!(matches!(client, ControlHttpError::BadRequest(_)));

        let server = super::map_forgejo_error(ForgejoError::Response {
            status: 503,
            body: "down".to_string(),
        });
        assert!(matches!(server, ControlHttpError::Internal(_)));
    }

    #[test]
    fn control_http_error_into_response_maps_status_codes() {
        let unauthorized = ControlHttpError::Unauthorized("nope".to_string()).into_response();
        assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);

        let bad_request = ControlHttpError::BadRequest("bad".to_string()).into_response();
        assert_eq!(bad_request.status(), StatusCode::BAD_REQUEST);

        let internal = ControlHttpError::Internal("boom".to_string()).into_response();
        assert_eq!(internal.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[tokio::test]
    async fn health_endpoint_returns_ok() {
        let (state, _transport, _repos) = test_state(Vec::new());
        let app = build_router(state);
        let response = app
            .oneshot(Request::builder().uri("/health").body(Body::empty()).unwrap())
            .await
            .expect("response");
        assert_eq!(response.status(), axum::http::StatusCode::OK);
    }

    #[tokio::test]
    async fn reqwest_state_routes_fail_fast_before_network_io() {
        let (state, transport, _repos) = test_state(Vec::new());
        let app = build_router(state);
        let (pubkey, privkey) = test_keys();

        let user_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/control/users")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&json!({
                            "username": "alice",
                            "email": "alice@example.com",
                            "password": "secret"
                        }))
                        .expect("user body"),
                    ))
                    .unwrap(),
            )
            .await
            .expect("user response");
        assert_eq!(user_response.status(), StatusCode::UNAUTHORIZED);

        let org_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/control/orgs")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&json!({
                            "owner": "gittree",
                            "name": "demo"
                        }))
                        .expect("org body"),
                    ))
                    .unwrap(),
            )
            .await
            .expect("org response");
        assert_eq!(org_response.status(), StatusCode::UNAUTHORIZED);

        let repo_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/control/repos")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&json!({
                            "owner": "gittree",
                            "name": "demo",
                            "pubkey": pubkey,
                            "privkey": privkey
                        }))
                        .expect("repo body"),
                    ))
                    .unwrap(),
            )
            .await
            .expect("repo response");
        assert_eq!(repo_response.status(), StatusCode::UNAUTHORIZED);

        let pull_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/control/pulls")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&json!({
                            "owner": "gittree",
                            "repo": "demo",
                            "head": "feature",
                            "base": "main",
                            "title": "demo"
                        }))
                        .expect("pull body"),
                    ))
                    .unwrap(),
            )
            .await
            .expect("pull response");
        assert_eq!(pull_response.status(), StatusCode::UNAUTHORIZED);

        let event_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/control/events")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&json!({
                            "kind": KIND_GITTREE_CONTROL.0,
                            "pubkey": "11".repeat(32),
                            "content": "{\"action\":\"create_user\"}"
                        }))
                        .expect("event body"),
                    ))
                    .unwrap(),
            )
            .await
            .expect("event response");
        assert_eq!(event_response.status(), StatusCode::UNAUTHORIZED);

        let tenant_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/relay/tenants")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("tenant response");
        assert_eq!(tenant_response.status(), StatusCode::UNAUTHORIZED);

        let nostr_repo_response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/repos")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("nostr repo response");
        assert_eq!(nostr_repo_response.status(), StatusCode::UNAUTHORIZED);
        assert!(
            transport.requests().is_empty(),
            "unauthorized requests should not reach forgejo transport"
        );
    }

    #[tokio::test]
    async fn create_user_rejects_missing_auth() {
        let (state, _transport, _repos) = test_state(Vec::new());
        let app = build_router(state);
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/control/users")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&json!({
                            "username":"alice",
                            "email":"alice@example.com",
                            "password":"secret"
                        }))
                        .expect("body"),
                    ))
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), axum::http::StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn create_user_rejects_empty_username() {
        let (state, _transport, _repos) = test_state(Vec::new());
        let app = build_router(state);
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/control/users")
                    .header("content-type", "application/json")
                    .header(AUTH_HEADER, "Bearer token")
                    .body(Body::from(
                        serde_json::to_vec(&json!({
                            "username":"  ",
                            "email":"alice@example.com",
                            "password":"secret"
                        }))
                        .expect("body"),
                    ))
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), axum::http::StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn create_user_rejects_empty_email() {
        let (state, _transport, _repos) = test_state(Vec::new());
        let app = build_router(state);
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/control/users")
                    .header("content-type", "application/json")
                    .header(AUTH_HEADER, "Bearer token")
                    .body(Body::from(
                        serde_json::to_vec(&json!({
                            "username":"alice",
                            "email":"  ",
                            "password":"secret"
                        }))
                        .expect("body"),
                    ))
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), axum::http::StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn create_user_rejects_empty_password() {
        let (state, _transport, _repos) = test_state(Vec::new());
        let app = build_router(state);
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/control/users")
                    .header("content-type", "application/json")
                    .header(AUTH_HEADER, "Bearer token")
                    .body(Body::from(
                        serde_json::to_vec(&json!({
                            "username":"alice",
                            "email":"alice@example.com",
                            "password":" "
                        }))
                        .expect("body"),
                    ))
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), axum::http::StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn create_user_posts_to_admin_endpoint() {
        let responses = vec![ForgejoResponse {
            status: 201,
            body: r#"{"login":"alice","email":"alice@example.com"}"#.to_string(),
        }];
        let (state, transport, _repos) = test_state(responses);
        let app = build_router(state);
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/control/users")
                    .header("content-type", "application/json")
                    .header(AUTH_HEADER, "Bearer token")
                    .body(Body::from(
                        serde_json::to_vec(&json!({
                            "username":"alice",
                            "email":"alice@example.com",
                            "password":"secret"
                        }))
                        .expect("body"),
                    ))
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        let requests = transport.requests();
        assert!(requests[0].url.ends_with("/api/v1/admin/users"));
    }

    #[tokio::test]
    async fn create_org_posts_to_admin_endpoint() {
        let responses = vec![ForgejoResponse {
            status: 201,
            body: r#"{"name":"acme","full_name":"Acme Org"}"#.to_string(),
        }];
        let (state, transport, _repos) = test_state(responses);
        let app = build_router(state);
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/control/orgs")
                    .header("content-type", "application/json")
                    .header(AUTH_HEADER, "Bearer token")
                    .body(Body::from(
                        serde_json::to_vec(&json!({
                            "owner":"admin",
                            "name":"acme",
                            "full_name":"Acme Org"
                        }))
                        .expect("body"),
                    ))
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        let requests = transport.requests();
        assert!(requests[0]
            .url
            .ends_with("/api/v1/admin/users/admin/orgs"));
    }

    #[tokio::test]
    async fn create_org_rejects_empty_owner() {
        let (state, _transport, _repos) = test_state(Vec::new());
        let app = build_router(state);
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/control/orgs")
                    .header("content-type", "application/json")
                    .header(AUTH_HEADER, "Bearer token")
                    .body(Body::from(
                        serde_json::to_vec(&json!({
                            "owner":" ",
                            "name":"acme",
                            "full_name":"Acme Org"
                        }))
                        .expect("body"),
                    ))
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), axum::http::StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn create_org_rejects_empty_name() {
        let (state, _transport, _repos) = test_state(Vec::new());
        let app = build_router(state);
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/control/orgs")
                    .header("content-type", "application/json")
                    .header(AUTH_HEADER, "Bearer token")
                    .body(Body::from(
                        serde_json::to_vec(&json!({
                            "owner":"admin",
                            "name":" ",
                            "full_name":"Acme Org"
                        }))
                        .expect("body"),
                    ))
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), axum::http::StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn create_repo_posts_to_admin_endpoint() {
        let responses = vec![ForgejoResponse {
            status: 201,
            body: r#"{"full_name":"alice/demo","name":"demo","owner":{"username":"alice"},"html_url":"http://localhost/alice/demo"}"#.to_string(),
        }];
        let (state, transport, repos) = test_state(responses);
        let (pubkey, privkey) = test_keys();
        let app = build_router(state);
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/control/repos")
                    .header("content-type", "application/json")
                    .header(AUTH_HEADER, "Bearer token")
                    .body(Body::from(
                        serde_json::to_vec(&json!({
                            "owner":"alice",
                            "name":"demo",
                            "auto_init":true,
                            "pubkey": pubkey,
                            "privkey": privkey
                        }))
                        .expect("body"),
                    ))
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        let requests = transport.requests();
        assert!(requests[0]
            .url
            .ends_with("/api/v1/admin/users/alice/repos"));
        let job = repos
            .claim_relay_publish(OffsetDateTime::now_utc())
            .await
            .expect("job")
            .expect("job");
        assert_eq!(job.forgejo_owner, "alice");
        assert_eq!(job.forgejo_repo, "demo");
    }

    #[tokio::test]
    async fn create_repo_accepts_signed_announcement() {
        let responses = vec![ForgejoResponse {
            status: 201,
            body: r#"{"full_name":"alice/demo","name":"demo","owner":{"username":"alice"},"html_url":"http://localhost/alice/demo"}"#.to_string(),
        }];
        let (state, transport, repos) = test_state(responses);
        let (pubkey, privkey) = test_keys();
        repos
            .upsert_account(AccountRecord::new(&pubkey, "alice").expect("account"))
            .await
            .expect("upsert");

        let npub = npub_from_hex(&pubkey).expect("npub");
        let clone_url = format_grasp_server_url_as_clone_url(
            &state.public_git_url,
            &npub,
            "demo",
        )
        .expect("clone");
        let announcement = RepoAnnouncement {
            identifier: "demo".to_string(),
            name: Some("Demo".to_string()),
            description: Some("test repo".to_string()),
            root_commit: None,
            clone: vec![clone_url],
            web: Vec::new(),
            relays: state.relay_urls.clone(),
            blossoms: Vec::new(),
            hashtags: Vec::new(),
            maintainers: vec![pubkey.clone()],
        };
        let secret_bytes = hex::decode(&privkey).expect("privkey");
        let secret = SecretKey::from_slice(&secret_bytes).expect("secret");
        let now = super::unix_timestamp();
        let signed = RelaySignedNostrEvent::from_announcement_with_created_at(
            &announcement,
            &secret,
            now,
        )
        .expect("signed");
        let request = RepoCreateRequest {
            event: api_event_from_relay(signed),
            private: Some(false),
        };
        let body = serde_json::to_vec(&request).expect("body");
        let auth_event = nip98_sign_event(
            &secret.secret_bytes(),
            "POST",
            "http://localhost/v1/repos",
            nip98_payload_hash(&body).as_deref(),
            now,
        )
        .expect("auth");
        let header = nostr_auth_header(&auth_event);

        let app = build_router(state);
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/repos")
                    .header("host", "localhost")
                    .header("content-type", "application/json")
                    .header(AUTH_HEADER, header)
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), axum::http::StatusCode::OK);

        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let repo: RepoCreateResponse = serde_json::from_slice(&body).expect("repo");
        assert_eq!(repo.owner, "alice");
        assert_eq!(repo.name, "demo");

        let requests = transport.requests();
        assert!(requests[0]
            .url
            .ends_with("/api/v1/admin/users/alice/repos"));
        let job = repos
            .claim_relay_publish(OffsetDateTime::now_utc())
            .await
            .expect("job")
            .expect("job");
        assert_eq!(job.forgejo_owner, "alice");
        assert_eq!(job.forgejo_repo, "demo");
    }

    #[tokio::test]
    async fn create_tenant_creates_membership() {
        let (state, _transport, repos) = test_state(Vec::new());
        let (pubkey, privkey) = test_keys();

        let body = serde_json::to_vec(&json!({
            "host": "relay.local",
            "name": "Relay Local",
            "public_read": false,
            "public_write": false,
            "auth_required": true
        }))
        .expect("body");
        let secret_bytes = hex::decode(&privkey).expect("privkey");
        let secret = SecretKey::from_slice(&secret_bytes).expect("secret");
        let now = super::unix_timestamp();
        let auth_event = nip98_sign_event(
            &secret.secret_bytes(),
            "POST",
            "http://localhost/v1/relay/tenants",
            nip98_payload_hash(&body).as_deref(),
            now,
        )
        .expect("auth");
        let header = nostr_auth_header(&auth_event);

        let app = build_router(state);
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/relay/tenants")
                    .header("host", "localhost")
                    .header("content-type", "application/json")
                    .header(AUTH_HEADER, header)
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), axum::http::StatusCode::OK);

        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let response: ControlCreateTenantResponse =
            serde_json::from_slice(&body).expect("tenant response");
        assert_eq!(response.host, "relay.local");
        assert_eq!(response.owner_pubkey, pubkey);

        let tenant = repos
            .tenant_by_host("relay.local")
            .await
            .expect("tenant lookup")
            .expect("tenant");
        let pubkey_bytes = hex::decode(&pubkey).expect("pubkey");
        let membership = repos
            .membership_by_pubkey(&tenant.id, &pubkey_bytes)
            .await
            .expect("membership lookup")
            .expect("membership");
        assert_eq!(membership.role, "owner");
    }

    #[tokio::test]
    async fn create_tenant_rejects_duplicate_host() {
        let (state, _transport, _repos) = test_state(Vec::new());
        let (_pubkey, privkey) = test_keys();
        let secret_bytes = hex::decode(&privkey).expect("privkey");
        let secret = SecretKey::from_slice(&secret_bytes).expect("secret");
        let now = super::unix_timestamp();

        let build_request = || {
            let body = serde_json::to_vec(&json!({
                "host": "relay.local",
                "name": "Relay Local"
            }))
            .expect("body");
            let auth_event = nip98_sign_event(
                &secret.secret_bytes(),
                "POST",
                "http://localhost/v1/relay/tenants",
                nip98_payload_hash(&body).as_deref(),
                now,
            )
            .expect("auth");
            (body, nostr_auth_header(&auth_event))
        };

        let app = build_router(state);
        let (body1, header1) = build_request();
        let first = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/relay/tenants")
                    .header("host", "localhost")
                    .header("content-type", "application/json")
                    .header(AUTH_HEADER, header1)
                    .body(Body::from(body1))
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(first.status(), axum::http::StatusCode::OK);

        let (body2, header2) = build_request();
        let second = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/relay/tenants")
                    .header("host", "localhost")
                    .header("content-type", "application/json")
                    .header(AUTH_HEADER, header2)
                    .body(Body::from(body2))
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(second.status(), axum::http::StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn create_repo_nostr_rejects_missing_body() {
        let (state, _transport, _repos) = test_state(Vec::new());
        let (_pubkey, privkey) = test_keys();
        let secret_bytes = hex::decode(&privkey).expect("privkey");
        let secret = SecretKey::from_slice(&secret_bytes).expect("secret");
        let now = super::unix_timestamp();
        let auth_event =
            nip98_sign_event(&secret.secret_bytes(), "POST", "http://localhost/v1/repos", None, now)
                .expect("auth");
        let header = nostr_auth_header(&auth_event);

        let app = build_router(state);
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/repos")
                    .header("host", "localhost")
                    .header(AUTH_HEADER, header)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), axum::http::StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn create_repo_nostr_rejects_missing_auth() {
        let (state, _transport, _repos) = test_state(Vec::new());
        let app = build_router(state);
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/repos")
                    .header("host", "localhost")
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), axum::http::StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn create_repo_nostr_rejects_missing_host_header() {
        let (state, _transport, _repos) = test_state(Vec::new());
        let (_pubkey, privkey) = test_keys();
        let secret_bytes = hex::decode(&privkey).expect("privkey");
        let secret = SecretKey::from_slice(&secret_bytes).expect("secret");
        let now = super::unix_timestamp();
        let auth_event =
            nip98_sign_event(&secret.secret_bytes(), "POST", "http://localhost/v1/repos", None, now)
                .expect("auth");
        let header = nostr_auth_header(&auth_event);

        let app = build_router(state);
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/repos")
                    .header(AUTH_HEADER, header)
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), axum::http::StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn create_tenant_rejects_missing_auth() {
        let (state, _transport, _repos) = test_state(Vec::new());
        let app = build_router(state);
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/relay/tenants")
                    .header("host", "localhost")
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), axum::http::StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn create_tenant_rejects_invalid_json_body() {
        let (state, _transport, _repos) = test_state(Vec::new());
        let (_pubkey, privkey) = test_keys();
        let secret_bytes = hex::decode(&privkey).expect("privkey");
        let secret = SecretKey::from_slice(&secret_bytes).expect("secret");
        let body = b"{invalid-json".to_vec();
        let now = super::unix_timestamp();
        let auth_event = nip98_sign_event(
            &secret.secret_bytes(),
            "POST",
            "http://localhost/v1/relay/tenants",
            nip98_payload_hash(&body).as_deref(),
            now,
        )
        .expect("auth");
        let header = nostr_auth_header(&auth_event);

        let app = build_router(state);
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/relay/tenants")
                    .header("host", "localhost")
                    .header("content-type", "application/json")
                    .header(AUTH_HEADER, header)
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), axum::http::StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn create_tenant_rejects_empty_host_value() {
        let (state, _transport, _repos) = test_state(Vec::new());
        let (_pubkey, privkey) = test_keys();
        let secret_bytes = hex::decode(&privkey).expect("privkey");
        let secret = SecretKey::from_slice(&secret_bytes).expect("secret");
        let body = serde_json::to_vec(&json!({
            "host": "   ",
            "name": "Relay Local"
        }))
        .expect("body");
        let now = super::unix_timestamp();
        let auth_event = nip98_sign_event(
            &secret.secret_bytes(),
            "POST",
            "http://localhost/v1/relay/tenants",
            nip98_payload_hash(&body).as_deref(),
            now,
        )
        .expect("auth");
        let header = nostr_auth_header(&auth_event);

        let app = build_router(state);
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/relay/tenants")
                    .header("host", "localhost")
                    .header("content-type", "application/json")
                    .header(AUTH_HEADER, header)
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), axum::http::StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn create_repo_rejects_pubkey_privkey_mismatch() {
        let (state, transport, _repos) = test_state(Vec::new());
        let (pubkey, _) = test_keys();
        let wrong_secret = SecretKey::from_slice(&[2u8; 32]).expect("secret");
        let app = build_router(state);
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/control/repos")
                    .header("content-type", "application/json")
                    .header(AUTH_HEADER, "Bearer token")
                    .body(Body::from(
                        serde_json::to_vec(&json!({
                            "owner":"alice",
                            "name":"demo",
                            "pubkey": pubkey,
                            "privkey": hex::encode(wrong_secret.secret_bytes())
                        }))
                        .expect("body"),
                    ))
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), axum::http::StatusCode::BAD_REQUEST);
        assert!(transport.requests().is_empty());
    }

    #[tokio::test]
    async fn create_repo_rejects_invalid_pubkey_hex() {
        let (state, transport, _repos) = test_state(Vec::new());
        let (_pubkey, privkey) = test_keys();
        let app = build_router(state);
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/control/repos")
                    .header("content-type", "application/json")
                    .header(AUTH_HEADER, "Bearer token")
                    .body(Body::from(
                        serde_json::to_vec(&json!({
                            "owner":"alice",
                            "name":"demo",
                            "pubkey": "zz",
                            "privkey": privkey
                        }))
                        .expect("body"),
                    ))
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), axum::http::StatusCode::BAD_REQUEST);
        assert!(transport.requests().is_empty());
    }

    #[tokio::test]
    async fn create_repo_rejects_non_admin_pubkey() {
        let (state, transport, _repos) =
            test_state_with_auth(Vec::new(), vec!["aa".repeat(32)], "gittree");
        let app = build_router(state);
        let (pubkey, privkey) = test_keys();
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/control/repos")
                    .header("content-type", "application/json")
                    .header(AUTH_HEADER, "Bearer token")
                    .body(Body::from(
                        serde_json::to_vec(&json!({
                            "owner":"alice",
                            "name":"demo",
                            "pubkey": pubkey,
                            "privkey": privkey
                        }))
                        .expect("body"),
                    ))
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), axum::http::StatusCode::UNAUTHORIZED);
        assert!(transport.requests().is_empty());
    }

    #[tokio::test]
    async fn create_repo_nostr_rejects_when_account_missing() {
        let (state, transport, _repos) = test_state(Vec::new());
        let (pubkey, privkey) = test_keys();

        let npub = npub_from_hex(&pubkey).expect("npub");
        let clone_url = format_grasp_server_url_as_clone_url(
            &state.public_git_url,
            &npub,
            "demo",
        )
        .expect("clone");
        let announcement = RepoAnnouncement {
            identifier: "demo".to_string(),
            name: Some("Demo".to_string()),
            description: Some("missing account".to_string()),
            root_commit: None,
            clone: vec![clone_url],
            web: Vec::new(),
            relays: state.relay_urls.clone(),
            blossoms: Vec::new(),
            hashtags: Vec::new(),
            maintainers: vec![pubkey.clone()],
        };

        let secret = parse_secret_key(&privkey).expect("secret");
        let now = super::unix_timestamp();
        let signed = RelaySignedNostrEvent::from_announcement_with_created_at(
            &announcement,
            &secret,
            now,
        )
        .expect("signed");
        let request = RepoCreateRequest {
            event: api_event_from_relay(signed),
            private: Some(false),
        };
        let body = serde_json::to_vec(&request).expect("body");
        let auth_event = nip98_sign_event(
            &secret.secret_bytes(),
            "POST",
            "http://localhost/v1/repos",
            nip98_payload_hash(&body).as_deref(),
            now,
        )
        .expect("auth");
        let header = nostr_auth_header(&auth_event);

        let app = build_router(state);
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/repos")
                    .header("host", "localhost")
                    .header("content-type", "application/json")
                    .header(AUTH_HEADER, header)
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), axum::http::StatusCode::BAD_REQUEST);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let message = String::from_utf8(body.to_vec()).expect("utf8");
        assert!(message.contains("account not found"));
        assert!(transport.requests().is_empty());
    }

    #[tokio::test]
    async fn create_repo_nostr_rejects_missing_required_relay_url() {
        let (state, _transport, _repos) = test_state(Vec::new());
        let (pubkey, privkey) = test_keys();

        let npub = npub_from_hex(&pubkey).expect("npub");
        let clone_url = format_grasp_server_url_as_clone_url(
            &state.public_git_url,
            &npub,
            "demo",
        )
        .expect("clone");
        let announcement = RepoAnnouncement {
            identifier: "demo".to_string(),
            name: Some("Demo".to_string()),
            description: Some("bad relay".to_string()),
            root_commit: None,
            clone: vec![clone_url],
            web: Vec::new(),
            relays: vec!["wss://other-relay.local".to_string()],
            blossoms: Vec::new(),
            hashtags: Vec::new(),
            maintainers: vec![pubkey.clone()],
        };

        let secret = parse_secret_key(&privkey).expect("secret");
        let now = super::unix_timestamp();
        let signed = RelaySignedNostrEvent::from_announcement_with_created_at(
            &announcement,
            &secret,
            now,
        )
        .expect("signed");
        let request = RepoCreateRequest {
            event: api_event_from_relay(signed),
            private: Some(false),
        };
        let body = serde_json::to_vec(&request).expect("body");
        let auth_event = nip98_sign_event(
            &secret.secret_bytes(),
            "POST",
            "http://localhost/v1/repos",
            nip98_payload_hash(&body).as_deref(),
            now,
        )
        .expect("auth");
        let header = nostr_auth_header(&auth_event);

        let app = build_router(state);
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/repos")
                    .header("host", "localhost")
                    .header("content-type", "application/json")
                    .header(AUTH_HEADER, header)
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), axum::http::StatusCode::BAD_REQUEST);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let message = String::from_utf8(body.to_vec()).expect("utf8");
        assert!(message.contains("missing relay url"));
    }

    #[tokio::test]
    async fn create_pull_posts_to_repo_endpoint() {
        let responses = vec![ForgejoResponse {
            status: 201,
            body: r#"{"number":5,"url":"http://localhost/api/v1/repos/gittree/demo/pulls/5"}"#.to_string(),
        }];
        let (state, transport, _repos) = test_state(responses);
        let app = build_router(state);
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/control/pulls")
                    .header("content-type", "application/json")
                    .header(AUTH_HEADER, "Bearer token")
                    .body(Body::from(
                        serde_json::to_vec(&json!({
                            "owner":"gittree",
                            "repo":"demo",
                            "head":"feature",
                            "base":"main",
                            "title":"Add thing"
                        }))
                        .expect("body"),
                    ))
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        let requests = transport.requests();
        assert!(requests[0]
            .url
            .ends_with("/api/v1/repos/gittree/demo/pulls"));
        let body = to_bytes(response.into_body(), usize::MAX).await.expect("body");
        assert!(!body.is_empty());
    }

    #[tokio::test]
    async fn create_pull_rejects_empty_title() {
        let (state, transport, _repos) = test_state(Vec::new());
        let app = build_router(state);
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/control/pulls")
                    .header("content-type", "application/json")
                    .header(AUTH_HEADER, "Bearer token")
                    .body(Body::from(
                        serde_json::to_vec(&json!({
                            "owner":"gittree",
                            "repo":"demo",
                            "head":"feature",
                            "base":"main",
                            "title":" "
                        }))
                        .expect("body"),
                    ))
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), axum::http::StatusCode::BAD_REQUEST);
        assert!(transport.requests().is_empty());
    }

    #[tokio::test]
    async fn control_event_rejects_non_admin_pubkey() {
        let (pubkey, privkey) = test_keys();
        let (state, _transport, _repos) = test_state_with_auth(
            Vec::new(),
            vec!["aa".repeat(32)],
            "gittree",
        );
        let app = build_router(state);
        let content = serde_json::to_string(&json!({
            "action": "create_repo",
            "name": "demo",
            "pubkey": pubkey.clone(),
            "privkey": privkey
        }))
        .expect("content");
        let payload = json!({
            "kind": KIND_GITTREE_CONTROL.0,
            "pubkey": pubkey,
            "content": content
        });
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/control/events")
                    .header("content-type", "application/json")
                    .header(AUTH_HEADER, "Bearer token")
                    .body(Body::from(serde_json::to_vec(&payload).expect("body")))
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), axum::http::StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn control_event_rejects_missing_auth() {
        let (pubkey, _privkey) = test_keys();
        let (state, _transport, _repos) = test_state(Vec::new());
        let app = build_router(state);
        let content = serde_json::to_string(&json!({
            "action": "create_repo",
            "name": "demo",
            "pubkey": pubkey,
            "privkey": "11".repeat(32),
        }))
        .expect("content");

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/control/events")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&json!({
                            "kind": KIND_GITTREE_CONTROL.0 as i64,
                            "pubkey": pubkey,
                            "content": content
                        }))
                        .expect("body"),
                    ))
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), axum::http::StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn control_event_rejects_invalid_kind_out_of_range() {
        let (pubkey, _privkey) = test_keys();
        let (state, _transport, _repos) = test_state(Vec::new());
        let app = build_router(state);
        let payload = json!({
            "kind": u64::MAX,
            "pubkey": pubkey,
            "content": "{}"
        });
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/control/events")
                    .header("content-type", "application/json")
                    .header(AUTH_HEADER, "Bearer token")
                    .body(Body::from(serde_json::to_vec(&payload).expect("body")))
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), axum::http::StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn control_event_rejects_empty_content() {
        let (pubkey, _privkey) = test_keys();
        let (state, _transport, _repos) = test_state(Vec::new());
        let app = build_router(state);
        let payload = json!({
            "kind": KIND_GITTREE_CONTROL.0,
            "pubkey": pubkey,
            "content": " "
        });
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/control/events")
                    .header("content-type", "application/json")
                    .header(AUTH_HEADER, "Bearer token")
                    .body(Body::from(serde_json::to_vec(&payload).expect("body")))
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), axum::http::StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn control_event_defaults_repo_owner() {
        let responses = vec![ForgejoResponse {
            status: 201,
            body: r#"{"full_name":"gittree/demo","name":"demo","owner":{"username":"gittree"},"html_url":"http://localhost/gittree/demo"}"#.to_string(),
        }];
        let (state, transport, repos) = test_state_with_auth(responses, Vec::new(), "gittree");
        let app = build_router(state);
        let (pubkey, privkey) = test_keys();
        let content = serde_json::to_string(&json!({
            "action": "create_repo",
            "name": "demo",
            "pubkey": pubkey.clone(),
            "privkey": privkey
        }))
        .expect("content");
        let payload = json!({
            "kind": KIND_GITTREE_CONTROL.0,
            "pubkey": pubkey,
            "content": content
        });
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/control/events")
                    .header("content-type", "application/json")
                    .header(AUTH_HEADER, "Bearer token")
                    .body(Body::from(serde_json::to_vec(&payload).expect("body")))
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        let requests = transport.requests();
        assert!(requests[0]
            .url
            .ends_with("/api/v1/admin/users/gittree/repos"));
        let job = repos
            .claim_relay_publish(OffsetDateTime::now_utc())
            .await
            .expect("job")
            .expect("job");
        assert_eq!(job.identifier, "demo");
    }

    #[tokio::test]
    async fn create_repo_rejects_empty_owner() {
        let (state, _transport, _repos) = test_state(Vec::new());
        let (pubkey, privkey) = test_keys();
        let app = build_router(state);
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/control/repos")
                    .header("content-type", "application/json")
                    .header(AUTH_HEADER, "Bearer token")
                    .body(Body::from(
                        serde_json::to_vec(&json!({
                            "owner":" ",
                            "name":"demo",
                            "auto_init":true,
                            "pubkey": pubkey,
                            "privkey": privkey
                        }))
                        .expect("body"),
                    ))
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), axum::http::StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn create_repo_rejects_empty_identifier_when_present() {
        let (state, _transport, _repos) = test_state(Vec::new());
        let (pubkey, privkey) = test_keys();
        let app = build_router(state);
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/control/repos")
                    .header("content-type", "application/json")
                    .header(AUTH_HEADER, "Bearer token")
                    .body(Body::from(
                        serde_json::to_vec(&json!({
                            "owner":"gittree",
                            "name":"demo",
                            "identifier":" ",
                            "auto_init":true,
                            "pubkey": pubkey,
                            "privkey": privkey
                        }))
                        .expect("body"),
                    ))
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), axum::http::StatusCode::BAD_REQUEST);
    }

    #[test]
    fn helper_validators_cover_reject_paths() {
        let missing = super::require_non_empty("owner", "   ").unwrap_err();
        assert!(matches!(missing, ControlHttpError::BadRequest(_)));

        let bad_hex_len = super::require_hex_len("event.id", "11", 64).unwrap_err();
        assert!(matches!(bad_hex_len, ControlHttpError::BadRequest(_)));

        let bad_hex64 = super::require_hex64("pubkey", "xyz").unwrap_err();
        assert!(matches!(bad_hex64, ControlHttpError::BadRequest(_)));

        let bad_npub = npub_from_hex("11").unwrap_err();
        assert!(matches!(bad_npub, ControlHttpError::BadRequest(_)));

        let auth = ControlAuthConfig {
            token: "token".to_string(),
            admin_keys: vec!["aa".repeat(32)],
        };
        authorize_admin_pubkey(&"aa".repeat(32), &auth).expect("admin key accepted");
    }

    #[test]
    fn verify_signed_event_rejects_mismatched_event_id() {
        let (pubkey, privkey) = test_keys();
        let npub = npub_from_hex(&pubkey).expect("npub");
        let clone_url =
            format_grasp_server_url_as_clone_url("http://localhost:8085", &npub, "demo")
                .expect("clone");
        let announcement = RepoAnnouncement {
            identifier: "demo".to_string(),
            name: Some("Demo".to_string()),
            description: Some("event id mismatch".to_string()),
            root_commit: None,
            clone: vec![clone_url],
            web: Vec::new(),
            relays: vec!["ws://relay.local".to_string()],
            blossoms: Vec::new(),
            hashtags: Vec::new(),
            maintainers: vec![pubkey],
        };
        let secret = parse_secret_key(&privkey).expect("secret");
        let mut event = api_event_from_relay(
            RelaySignedNostrEvent::from_announcement_with_created_at(
                &announcement,
                &secret,
                super::unix_timestamp(),
            )
            .expect("signed"),
        );
        event.id = "00".repeat(32);
        let err = super::verify_signed_event(&event).unwrap_err();
        assert!(matches!(err, ControlHttpError::BadRequest(_)));
    }

    #[tokio::test]
    async fn control_event_routes_create_user_and_org_actions() {
        let responses = vec![
            ForgejoResponse {
                status: 201,
                body: r#"{"login":"alice","email":"alice@example.com"}"#.to_string(),
            },
            ForgejoResponse {
                status: 201,
                body: r#"{"name":"acme","full_name":"Acme Org"}"#.to_string(),
            },
        ];
        let (state, transport, _repos) = test_state_with_auth(responses, Vec::new(), "gittree");
        let app = build_router(state);
        let (pubkey, _privkey) = test_keys();

        let create_user = json!({
            "kind": KIND_GITTREE_CONTROL.0,
            "pubkey": pubkey,
            "content": serde_json::to_string(&json!({
                "action": "create_user",
                "username": "alice",
                "email": "alice@example.com",
                "password": "secret"
            })).expect("content")
        });
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/control/events")
                    .header("content-type", "application/json")
                    .header(AUTH_HEADER, "Bearer token")
                    .body(Body::from(serde_json::to_vec(&create_user).expect("body")))
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), axum::http::StatusCode::OK);

        let create_org = json!({
            "kind": KIND_GITTREE_CONTROL.0,
            "pubkey": test_keys().0,
            "content": serde_json::to_string(&json!({
                "action": "create_org",
                "name": "acme",
                "full_name": "Acme Org"
            })).expect("content")
        });
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/control/events")
                    .header("content-type", "application/json")
                    .header(AUTH_HEADER, "Bearer token")
                    .body(Body::from(serde_json::to_vec(&create_org).expect("body")))
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), axum::http::StatusCode::OK);

        let requests = transport.requests();
        assert!(requests[0].url.ends_with("/api/v1/admin/users"));
        assert!(requests[1].url.ends_with("/api/v1/admin/users/gittree/orgs"));
    }

    #[tokio::test]
    async fn control_event_routes_create_pull_request_action() {
        let responses = vec![ForgejoResponse {
            status: 201,
            body: r#"{"number":5,"url":"http://localhost/api/v1/repos/gittree/demo/pulls/5"}"#.to_string(),
        }];
        let (state, transport, _repos) = test_state_with_auth(responses, Vec::new(), "gittree");
        let app = build_router(state);
        let (pubkey, _privkey) = test_keys();
        let payload = json!({
            "kind": KIND_GITTREE_CONTROL.0,
            "pubkey": pubkey,
            "content": serde_json::to_string(&json!({
                "action": "create_pull_request",
                "owner": "gittree",
                "repo": "demo",
                "head": "feature",
                "base": "main",
                "title": "Add thing"
            })).expect("content")
        });
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/control/events")
                    .header("content-type", "application/json")
                    .header(AUTH_HEADER, "Bearer token")
                    .body(Body::from(serde_json::to_vec(&payload).expect("body")))
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        let requests = transport.requests();
        assert!(requests[0]
            .url
            .ends_with("/api/v1/repos/gittree/demo/pulls"));
    }

    #[tokio::test]
    async fn control_event_rejects_repo_action_pubkey_mismatch() {
        let (state, transport, _repos) = test_state(Vec::new());
        let app = build_router(state);
        let (pubkey, privkey) = test_keys();
        let mismatched = "22".repeat(32);
        let payload = json!({
            "kind": KIND_GITTREE_CONTROL.0,
            "pubkey": pubkey,
            "content": serde_json::to_string(&json!({
                "action": "create_repo",
                "name": "demo",
                "pubkey": mismatched,
                "privkey": privkey
            })).expect("content")
        });
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/control/events")
                    .header("content-type", "application/json")
                    .header(AUTH_HEADER, "Bearer token")
                    .body(Body::from(serde_json::to_vec(&payload).expect("body")))
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), axum::http::StatusCode::BAD_REQUEST);
        assert!(transport.requests().is_empty());
    }

    #[tokio::test]
    async fn create_repo_nostr_rejects_invalid_kind_and_auth_pubkey_mismatch() {
        let (state, _transport, repos) = test_state(Vec::new());
        let (pubkey, privkey) = test_keys();
        repos
            .upsert_account(AccountRecord::new(&pubkey, "alice").expect("account"))
            .await
            .expect("upsert");

        let npub = npub_from_hex(&pubkey).expect("npub");
        let clone_url = format_grasp_server_url_as_clone_url(&state.public_git_url, &npub, "demo")
            .expect("clone");
        let announcement = RepoAnnouncement {
            identifier: "demo".to_string(),
            name: Some("Demo".to_string()),
            description: Some("invalid kind".to_string()),
            root_commit: None,
            clone: vec![clone_url],
            web: Vec::new(),
            relays: state.relay_urls.clone(),
            blossoms: Vec::new(),
            hashtags: Vec::new(),
            maintainers: vec![pubkey.clone()],
        };
        let secret = parse_secret_key(&privkey).expect("secret");
        let now = super::unix_timestamp();
        let signed = RelaySignedNostrEvent::from_announcement_with_created_at(
            &announcement,
            &secret,
            now,
        )
        .expect("signed");

        let mut invalid_kind = api_event_from_relay(signed.clone());
        invalid_kind.kind = 1;
        let body = serde_json::to_vec(&RepoCreateRequest {
            event: invalid_kind,
            private: Some(false),
        })
        .expect("body");
        let auth_event = nip98_sign_event(
            &secret.secret_bytes(),
            "POST",
            "http://localhost/v1/repos",
            nip98_payload_hash(&body).as_deref(),
            now,
        )
        .expect("auth");
        let response = build_router(state.clone())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/repos")
                    .header("host", "localhost")
                    .header("content-type", "application/json")
                    .header(AUTH_HEADER, nostr_auth_header(&auth_event))
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), axum::http::StatusCode::BAD_REQUEST);

        let other_secret = SecretKey::from_slice(&[2u8; 32]).expect("secret");
        let mismatch_body = serde_json::to_vec(&RepoCreateRequest {
            event: api_event_from_relay(signed),
            private: Some(false),
        })
        .expect("body");
        let mismatch_auth = nip98_sign_event(
            &other_secret.secret_bytes(),
            "POST",
            "http://localhost/v1/repos",
            nip98_payload_hash(&mismatch_body).as_deref(),
            now,
        )
        .expect("auth");
        let response = build_router(state)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/repos")
                    .header("host", "localhost")
                    .header("content-type", "application/json")
                    .header(AUTH_HEADER, nostr_auth_header(&mismatch_auth))
                    .body(Body::from(mismatch_body))
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), axum::http::StatusCode::UNAUTHORIZED);
    }
}
