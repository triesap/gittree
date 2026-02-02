use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use bech32::{Bech32, Hrp};
use gittree_config::{
    ConfigError, ControlAuthConfig, ForgejoConfig, RelayTargetsConfig, ServicesConfig, UiConfig,
};
use gittree_core::kinds::KIND_GITTREE_CONTROL;
use gittree_core::{ControlAction, RepoAnnouncement, format_grasp_server_url_as_clone_url};
use gittree_forgejo::{
    ForgejoClient, ForgejoCreateOrg, ForgejoCreatePullRequest, ForgejoCreateRepo,
    ForgejoCreateUser, ForgejoError, ForgejoOrg, ForgejoPullRequest, ForgejoRepo, ForgejoTransport,
    ForgejoUser,
};
use gittree_observability::{ObservabilityConfigError, ObservabilityError, ObservabilityHandle};
use gittree_relay_adapter::SignedNostrEvent;
use gittree_storage::{
    PostgresRepositories, RelayPublishRepository, RelayPublishRequest, StorageConfig, StorageError,
};
use secp256k1::SecretKey;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[allow(dead_code)]
const AUTH_HEADER: &str = "authorization";
const ENV_STORAGE_READ_URL: &str = "GITTREE_STORAGE_READ_URL";
const ENV_STORAGE_WRITE_URL: &str = "GITTREE_STORAGE_WRITE_URL";
const ENV_STORAGE_MAX_CONNECTIONS: &str = "GITTREE_STORAGE_MAX_CONNECTIONS";
const ENV_STORAGE_MIN_CONNECTIONS: &str = "GITTREE_STORAGE_MIN_CONNECTIONS";
const ENV_STORAGE_IDLE_TIMEOUT_SECS: &str = "GITTREE_STORAGE_IDLE_TIMEOUT_SECS";
const ENV_STORAGE_MAX_LIFETIME_SECS: &str = "GITTREE_STORAGE_MAX_LIFETIME_SECS";
const ENV_STORAGE_APP_NAME: &str = "GITTREE_STORAGE_APP_NAME";

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
struct ControlAppState<T> {
    auth: ControlAuthConfig,
    forgejo: ForgejoClient<T>,
    forgejo_owner: String,
    repositories: Arc<dyn RelayPublishRepository>,
    relay_urls: Vec<String>,
    public_git_url: String,
}

pub async fn serve(config: ControlConfig) -> Result<(), ControlError> {
    let _observability = init_observability()?;
    let repositories = build_repositories(&config)?;
    let bind = config.bind.clone();
    let forgejo_owner = config.forgejo.owner.clone();
    let relay_urls = config.relay_urls;
    let public_git_url = config.public_git_url;
    let auth = config.auth;
    let forgejo = ForgejoClient::new(config.forgejo).map_err(ControlError::Forgejo)?;
    let state = ControlAppState {
        auth,
        forgejo,
        forgejo_owner,
        repositories: Arc::new(repositories),
        relay_urls,
        public_git_url,
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

fn build_router<T>(state: ControlAppState<T>) -> Router
where
    T: ForgejoTransport + Clone + Send + Sync + 'static,
{
    Router::new()
        .route("/health", get(health_handler))
        .route("/control/users", post(create_user_handler))
        .route("/control/orgs", post(create_org_handler))
        .route("/control/repos", post(create_repo_handler))
        .route("/control/pulls", post(create_pull_handler))
        .route("/control/events", post(control_event_handler))
        .with_state(state)
}

async fn health_handler<T>(State(state): State<ControlAppState<T>>) -> &'static str
where
    T: ForgejoTransport + Clone + Send + Sync + 'static,
{
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

async fn create_user_handler<T>(
    State(state): State<ControlAppState<T>>,
    headers: HeaderMap,
    Json(payload): Json<ControlCreateUserRequest>,
) -> Result<Json<ControlUserResponse>, ControlHttpError>
where
    T: ForgejoTransport + Clone + Send + Sync + 'static,
{
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

async fn create_org_handler<T>(
    State(state): State<ControlAppState<T>>,
    headers: HeaderMap,
    Json(payload): Json<ControlCreateOrgRequest>,
) -> Result<Json<ControlOrgResponse>, ControlHttpError>
where
    T: ForgejoTransport + Clone + Send + Sync + 'static,
{
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

async fn create_repo_handler<T>(
    State(state): State<ControlAppState<T>>,
    headers: HeaderMap,
    Json(payload): Json<ControlCreateRepoRequest>,
) -> Result<Json<ControlRepoResponse>, ControlHttpError>
where
    T: ForgejoTransport + Clone + Send + Sync + 'static,
{
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

async fn create_pull_handler<T>(
    State(state): State<ControlAppState<T>>,
    headers: HeaderMap,
    Json(payload): Json<ControlCreatePullRequest>,
) -> Result<Json<ControlPullResponse>, ControlHttpError>
where
    T: ForgejoTransport + Clone + Send + Sync + 'static,
{
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

async fn control_event_handler<T>(
    State(state): State<ControlAppState<T>>,
    headers: HeaderMap,
    Json(payload): Json<ControlEventRequest>,
) -> Result<Json<ControlEventResponse>, ControlHttpError>
where
    T: ForgejoTransport + Clone + Send + Sync + 'static,
{
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

async fn apply_control_action<T>(
    state: &ControlAppState<T>,
    action: ControlAction,
) -> Result<ControlEventResponse, ControlHttpError>
where
    T: ForgejoTransport + Clone + Send + Sync + 'static,
{
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

async fn create_repo_with_announcement<T>(
    state: &ControlAppState<T>,
    input: CreateRepoInput,
) -> Result<ControlRepoResponse, ControlHttpError>
where
    T: ForgejoTransport + Clone + Send + Sync + 'static,
{
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

    let signed = SignedNostrEvent::from_announcement(&announcement, &secret_key)
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

fn require_non_empty(field: &'static str, value: &str) -> Result<(), ControlHttpError> {
    if value.trim().is_empty() {
        return Err(ControlHttpError::BadRequest(format!(
            "missing {field}"
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
    if bytes.len() != 32 {
        return Err(ControlHttpError::BadRequest("invalid pubkey".to_string()));
    }
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
    use super::{AUTH_HEADER, ControlConfig, ControlHttpError, authorize, build_router};
    use async_trait::async_trait;
    use axum::body::{Body, to_bytes};
    use axum::http::{HeaderMap, Request};
    use gittree_config::{ControlAuthConfig, ForgejoConfig};
    use gittree_core::kinds::KIND_GITTREE_CONTROL;
    use gittree_forgejo::{ForgejoClient, ForgejoRequest, ForgejoResponse, ForgejoTransport};
    use gittree_storage::{InMemoryRepositories, RelayPublishRepository};
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
        super::ControlAppState<MockTransport>,
        MockTransport,
        Arc<InMemoryRepositories>,
    ) {
        test_state_with_auth(responses, Vec::new(), "gittree")
    }

    fn test_state_with_auth(
        responses: Vec<ForgejoResponse>,
        admin_keys: Vec<String>,
        owner: &str,
    ) -> (
        super::ControlAppState<MockTransport>,
        MockTransport,
        Arc<InMemoryRepositories>,
    ) {
        let transport = MockTransport::new(responses);
        let client = ForgejoClient::with_transport(test_config(), transport.clone());
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
}
