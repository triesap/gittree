#![forbid(unsafe_code)]

use axum::body::Bytes;
use axum::extract::{OriginalUri, Path, State};
use axum::http::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE};
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use gittree_app_core::{
    Profile, ProfileUpdate, ProfileVisibility as ApiProfileVisibility, pubkey_bytes_from_npub,
};
use gittree_config::{AuthConfig as AuthSettings, ConfigError, ForgejoConfig, ServicesConfig};
use gittree_forgejo::{
    ForgejoClient, ForgejoCreateUser, ForgejoError, ForgejoTransport, ReqwestTransport,
};
use gittree_nostr_auth::{Nip98Event, Nip98Request, validate_nip98};
use gittree_observability::{ObservabilityConfigError, ObservabilityError, ObservabilityHandle};
use gittree_storage::{
    AccountRecord, AccountRepository, PostgresRepositories, ProfileRecord, ProfileRepository,
    ProfileVisibility as StorageProfileVisibility, StorageConfig, StorageError,
};
use rand::RngCore;
use serde::Serialize;
use sha2::Digest;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tower_http::cors::{Any, CorsLayer};

const AUTH_HEADER: &str = "authorization";
const ENV_STORAGE_READ_URL: &str = "GITTREE_STORAGE_READ_URL";
const ENV_STORAGE_WRITE_URL: &str = "GITTREE_STORAGE_WRITE_URL";
const ENV_STORAGE_MAX_CONNECTIONS: &str = "GITTREE_STORAGE_MAX_CONNECTIONS";
const ENV_STORAGE_MIN_CONNECTIONS: &str = "GITTREE_STORAGE_MIN_CONNECTIONS";
const ENV_STORAGE_IDLE_TIMEOUT_SECS: &str = "GITTREE_STORAGE_IDLE_TIMEOUT_SECS";
const ENV_STORAGE_MAX_LIFETIME_SECS: &str = "GITTREE_STORAGE_MAX_LIFETIME_SECS";
const ENV_STORAGE_APP_NAME: &str = "GITTREE_STORAGE_APP_NAME";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthServiceConfig {
    pub bind: String,
    pub auth: AuthSettings,
    pub forgejo: ForgejoConfig,
    pub storage: StorageConfig,
}

impl AuthServiceConfig {
    pub fn from_env() -> Result<Self, AuthConfigError> {
        Self::from_env_with(|key| std::env::var(key).ok())
    }

    fn from_env_with<F>(mut get_var: F) -> Result<Self, AuthConfigError>
    where
        F: FnMut(&'static str) -> Option<String>,
    {
        let services = ServicesConfig::from_env_validated_with(&mut get_var)
            .map_err(AuthConfigError::Config)?;
        let auth = AuthSettings::from_env_with(&mut get_var).map_err(AuthConfigError::Config)?;
        let forgejo =
            ForgejoConfig::from_env_with(&mut get_var).map_err(AuthConfigError::Config)?;
        let storage = storage_from_env_with(&mut get_var)?;
        Ok(Self {
            bind: services.auth.bind,
            auth,
            forgejo,
            storage,
        })
    }
}

#[derive(Debug)]
pub enum AuthConfigError {
    Config(ConfigError),
    Storage(StorageConfigError),
}

impl std::fmt::Display for AuthConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AuthConfigError::Config(err) => write!(f, "auth config error: {err}"),
            AuthConfigError::Storage(err) => write!(f, "auth storage config error: {err}"),
        }
    }
}

impl std::error::Error for AuthConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            AuthConfigError::Config(err) => Some(err),
            AuthConfigError::Storage(err) => Some(err),
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

fn storage_from_env_with<F>(mut get_var: F) -> Result<StorageConfig, AuthConfigError>
where
    F: FnMut(&'static str) -> Option<String>,
{
    let read_connection = get_var(ENV_STORAGE_READ_URL).ok_or(AuthConfigError::Storage(
        StorageConfigError::MissingEnv(ENV_STORAGE_READ_URL),
    ))?;
    let write_connection = get_var(ENV_STORAGE_WRITE_URL);
    let max_connections = env_u32_with(ENV_STORAGE_MAX_CONNECTIONS, &mut get_var)?.unwrap_or(10);
    let min_connections = env_u32_with(ENV_STORAGE_MIN_CONNECTIONS, &mut get_var)?.unwrap_or(2);
    let idle_timeout_secs = env_u64_with(ENV_STORAGE_IDLE_TIMEOUT_SECS, &mut get_var)?;
    let max_lifetime_secs = env_u64_with(ENV_STORAGE_MAX_LIFETIME_SECS, &mut get_var)?;
    let application_name = get_var(ENV_STORAGE_APP_NAME);

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
        return Err(AuthConfigError::Storage(StorageConfigError::InvalidConfig(
            err.to_string(),
        )));
    }

    Ok(config)
}

fn env_u32_with<F>(key: &'static str, mut get_var: F) -> Result<Option<u32>, AuthConfigError>
where
    F: FnMut(&'static str) -> Option<String>,
{
    match get_var(key) {
        Some(value) => {
            if value.trim().is_empty() {
                return Ok(None);
            }
            match value.parse::<u32>() {
                Ok(parsed) => Ok(Some(parsed)),
                Err(_) => Err(AuthConfigError::Storage(StorageConfigError::InvalidEnv {
                    key,
                    value,
                })),
            }
        }
        None => Ok(None),
    }
}

fn env_u64_with<F>(key: &'static str, mut get_var: F) -> Result<Option<u64>, AuthConfigError>
where
    F: FnMut(&'static str) -> Option<String>,
{
    match get_var(key) {
        Some(value) => {
            if value.trim().is_empty() {
                return Ok(None);
            }
            match value.parse::<u64>() {
                Ok(parsed) => Ok(Some(parsed)),
                Err(_) => Err(AuthConfigError::Storage(StorageConfigError::InvalidEnv {
                    key,
                    value,
                })),
            }
        }
        None => Ok(None),
    }
}

#[derive(Debug)]
pub enum AuthError {
    Config(AuthConfigError),
    Forgejo(ForgejoError),
    ObservabilityConfig(ObservabilityConfigError),
    Observability(ObservabilityError),
    Storage(StorageError),
    Serve(String),
}

impl std::fmt::Display for AuthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AuthError::Config(err) => write!(f, "auth error: {err}"),
            AuthError::Forgejo(err) => write!(f, "auth forgejo error: {err}"),
            AuthError::ObservabilityConfig(err) => {
                write!(f, "auth observability config error: {err}")
            }
            AuthError::Observability(err) => write!(f, "auth observability error: {err}"),
            AuthError::Storage(err) => write!(f, "auth storage error: {err}"),
            AuthError::Serve(err) => write!(f, "auth serve error: {err}"),
        }
    }
}

impl std::error::Error for AuthError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            AuthError::Config(err) => Some(err),
            AuthError::Forgejo(err) => Some(err),
            AuthError::ObservabilityConfig(err) => Some(err),
            AuthError::Observability(err) => Some(err),
            AuthError::Storage(err) => Some(err),
            AuthError::Serve(_) => None,
        }
    }
}

pub fn init_observability() -> Result<ObservabilityHandle, AuthError> {
    init_observability_with(|| {
        gittree_observability::ObservabilityConfig::from_env("gittree-auth")
    })
}

fn init_observability_with<F>(load_config: F) -> Result<ObservabilityHandle, AuthError>
where
    F: FnOnce() -> Result<gittree_observability::ObservabilityConfig, ObservabilityConfigError>,
{
    let config = load_config().map_err(AuthError::ObservabilityConfig)?;
    let handle = gittree_observability::init(&config).map_err(AuthError::Observability)?;
    Ok(handle)
}

#[derive(Clone)]
struct AuthAppState {
    auth: AuthSettings,
    forgejo: ForgejoClient<Arc<dyn ForgejoTransport>>,
    accounts: Arc<dyn AccountRepository>,
    profiles: Arc<dyn ProfileRepository>,
}

async fn run_server<E, Fut>(server: Fut) -> Result<(), AuthError>
where
    E: std::fmt::Display,
    Fut: std::future::IntoFuture<Output = Result<(), E>>,
{
    match server.into_future().await {
        Ok(()) => Ok(()),
        Err(err) => Err(AuthError::Serve(err.to_string())),
    }
}

async fn serve_inner(bind: &str, router: Router) -> Result<(), AuthError> {
    let listener = match tokio::net::TcpListener::bind(bind).await {
        Ok(listener) => listener,
        Err(err) => return Err(AuthError::Serve(err.to_string())),
    };
    run_server(axum::serve(listener, router)).await
}

pub async fn serve(config: AuthServiceConfig) -> Result<(), AuthError> {
    let _observability = init_observability()?;
    serve_without_observability(config).await
}

async fn serve_without_observability(config: AuthServiceConfig) -> Result<(), AuthError> {
    let repositories = Arc::new(build_repositories(&config)?);
    let bind = config.bind.clone();
    let auth = config.auth;
    let forgejo_config = config.forgejo;
    let transport =
        ReqwestTransport::new(forgejo_config.api_token.clone()).map_err(AuthError::Forgejo)?;
    let transport: Arc<dyn ForgejoTransport> = Arc::new(transport);
    let forgejo = ForgejoClient::with_transport(forgejo_config, transport);
    let state = AuthAppState {
        auth,
        forgejo,
        accounts: repositories.clone(),
        profiles: repositories,
    };
    let router = build_router(state);
    serve_inner(&bind, router).await
}

fn build_router(state: AuthAppState) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods([Method::GET, Method::POST, Method::PATCH, Method::OPTIONS])
        .allow_headers([AUTHORIZATION, CONTENT_TYPE, ACCEPT]);
    Router::new()
        .route("/health", get(health_handler))
        .route("/v1/signup", post(signup_handler))
        .route(
            "/v1/profile",
            get(profile_get_handler).patch(profile_patch_handler),
        )
        .route("/v1/profile/:npub", get(profile_public_handler))
        .layer(cors)
        .with_state(state)
}

async fn health_handler(State(state): State<AuthAppState>) -> &'static str {
    let _ = (&state.auth, &state.forgejo);
    "ok"
}

#[derive(Debug, Serialize)]
struct SignupResponse {
    pubkey: String,
    username: String,
    status: String,
}

#[derive(Debug, Serialize)]
struct ErrorResponse {
    error: String,
}

#[derive(Debug)]
enum AuthHttpError {
    Unauthorized(String),
    BadRequest(String),
    NotFound(String),
    Internal(String),
}

impl IntoResponse for AuthHttpError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            AuthHttpError::Unauthorized(message) => (StatusCode::UNAUTHORIZED, message),
            AuthHttpError::BadRequest(message) => (StatusCode::BAD_REQUEST, message),
            AuthHttpError::NotFound(message) => (StatusCode::NOT_FOUND, message),
            AuthHttpError::Internal(message) => (StatusCode::INTERNAL_SERVER_ERROR, message),
        };
        (status, Json(ErrorResponse { error: message })).into_response()
    }
}

async fn signup_handler(
    State(state): State<AuthAppState>,
    headers: HeaderMap,
    method: Method,
    OriginalUri(uri): OriginalUri,
    body: Bytes,
) -> Result<Json<SignupResponse>, AuthHttpError> {
    let event = parse_nostr_auth(&headers)?;
    let request_url = build_request_url(&headers, &uri)?;
    let payload_hash = payload_hash(&body);
    let now = unix_timestamp();
    let request = Nip98Request {
        method: method.as_str(),
        url: &request_url,
        payload_sha256: payload_hash.as_deref(),
        now,
        max_skew_seconds: state.auth.max_skew_seconds as i64,
    };
    let auth = match validate_nip98(&event, &request) {
        Ok(auth) => auth,
        Err(err) => return Err(AuthHttpError::Unauthorized(err.to_string())),
    };

    let username = username_from_pubkey(&auth.pubkey)?;
    let pubkey_bytes = parse_pubkey_bytes(&auth.pubkey)?;

    let existing = match state.accounts.account_by_pubkey(&pubkey_bytes).await {
        Ok(existing) => existing,
        Err(err) => return Err(AuthHttpError::Internal(err.to_string())),
    };

    if let Some(existing) = existing {
        return Ok(Json(SignupResponse {
            pubkey: auth.pubkey,
            username: existing.forgejo_username,
            status: "existing".to_string(),
        }));
    }

    let email = format!("{}@{}", username, state.auth.email_domain);
    let password = generate_password();
    let forgejo_user = match state
        .forgejo
        .ensure_user(ForgejoCreateUser {
            username: username.clone(),
            email,
            password,
            full_name: None,
            must_change_password: Some(false),
            send_notify: Some(false),
        })
        .await
    {
        Ok(user) => user,
        Err(err) => return Err(AuthHttpError::Internal(err.to_string())),
    };

    let record = match AccountRecord::new(&auth.pubkey, &forgejo_user.username) {
        Ok(record) => record,
        Err(err) => return Err(AuthHttpError::BadRequest(err.to_string())),
    };
    if let Err(err) = state.accounts.upsert_account(record).await {
        return Err(AuthHttpError::Internal(err.to_string()));
    }

    let profile = ProfileRecord::new(
        &auth.pubkey,
        Some(forgejo_user.username.clone()),
        None,
        None,
        None,
        None,
        StorageProfileVisibility::Private,
        now,
        now,
    )
    .map_err(profile_input_error)?;
    if let Err(err) = state.profiles.upsert_profile(profile).await {
        return Err(AuthHttpError::Internal(err.to_string()));
    }

    Ok(Json(SignupResponse {
        pubkey: auth.pubkey,
        username: forgejo_user.username,
        status: "created".to_string(),
    }))
}

async fn profile_get_handler(
    State(state): State<AuthAppState>,
    headers: HeaderMap,
    method: Method,
    OriginalUri(uri): OriginalUri,
) -> Result<Json<Profile>, AuthHttpError> {
    let event = parse_nostr_auth(&headers)?;
    let request_url = build_request_url(&headers, &uri)?;
    let now = unix_timestamp();
    let request = Nip98Request {
        method: method.as_str(),
        url: &request_url,
        payload_sha256: None,
        now,
        max_skew_seconds: state.auth.max_skew_seconds as i64,
    };
    let auth = match validate_nip98(&event, &request) {
        Ok(auth) => auth,
        Err(err) => return Err(AuthHttpError::Unauthorized(err.to_string())),
    };

    let pubkey_bytes = parse_pubkey_bytes(&auth.pubkey)?;
    let account = match state.accounts.account_by_pubkey(&pubkey_bytes).await {
        Ok(Some(account)) => account,
        Ok(None) => return Err(AuthHttpError::BadRequest("account not found".to_string())),
        Err(err) => return Err(AuthHttpError::Internal(err.to_string())),
    };
    let profile = ensure_profile(
        &state.profiles,
        &auth.pubkey,
        &account.forgejo_username,
        now,
    )
    .await?;
    Ok(Json(profile_response(
        &auth.pubkey,
        &account.forgejo_username,
        profile,
    )))
}

async fn profile_patch_handler(
    State(state): State<AuthAppState>,
    headers: HeaderMap,
    method: Method,
    OriginalUri(uri): OriginalUri,
    body: Bytes,
) -> Result<Json<Profile>, AuthHttpError> {
    let event = parse_nostr_auth(&headers)?;
    let request_url = build_request_url(&headers, &uri)?;
    let payload_hash = payload_hash(&body);
    let now = unix_timestamp();
    let request = Nip98Request {
        method: method.as_str(),
        url: &request_url,
        payload_sha256: payload_hash.as_deref(),
        now,
        max_skew_seconds: state.auth.max_skew_seconds as i64,
    };
    let auth = match validate_nip98(&event, &request) {
        Ok(auth) => auth,
        Err(err) => return Err(AuthHttpError::Unauthorized(err.to_string())),
    };

    if body.is_empty() {
        return Err(AuthHttpError::BadRequest(
            "missing profile update".to_string(),
        ));
    }
    let update: ProfileUpdate = match serde_json::from_slice(&body) {
        Ok(update) => update,
        Err(err) => {
            return Err(AuthHttpError::BadRequest(format!(
                "invalid profile update: {err}"
            )));
        }
    };

    let pubkey_bytes = parse_pubkey_bytes(&auth.pubkey)?;
    let account = match state.accounts.account_by_pubkey(&pubkey_bytes).await {
        Ok(Some(account)) => account,
        Ok(None) => return Err(AuthHttpError::BadRequest("account not found".to_string())),
        Err(err) => return Err(AuthHttpError::Internal(err.to_string())),
    };
    let profile = ensure_profile(
        &state.profiles,
        &auth.pubkey,
        &account.forgejo_username,
        now,
    )
    .await?;
    let updated =
        apply_profile_update(&auth.pubkey, profile, update, now).map_err(profile_input_error)?;
    if let Err(err) = state.profiles.upsert_profile(updated.clone()).await {
        return Err(AuthHttpError::Internal(err.to_string()));
    }
    Ok(Json(profile_response(
        &auth.pubkey,
        &account.forgejo_username,
        updated,
    )))
}

async fn profile_public_handler(
    State(state): State<AuthAppState>,
    Path(npub): Path<String>,
) -> Result<Json<Profile>, AuthHttpError> {
    let pubkey_bytes = match pubkey_bytes_from_npub(&npub) {
        Ok(pubkey_bytes) => pubkey_bytes,
        Err(_) => return Err(AuthHttpError::BadRequest("invalid npub".to_string())),
    };
    let account = match state.accounts.account_by_pubkey(&pubkey_bytes).await {
        Ok(Some(account)) => account,
        Ok(None) => return Err(AuthHttpError::NotFound("profile not found".to_string())),
        Err(err) => return Err(AuthHttpError::Internal(err.to_string())),
    };
    let profile = match state.profiles.profile_by_pubkey(&pubkey_bytes).await {
        Ok(Some(profile)) => profile,
        Ok(None) => return Err(AuthHttpError::NotFound("profile not found".to_string())),
        Err(err) => return Err(AuthHttpError::Internal(err.to_string())),
    };

    if profile.visibility != StorageProfileVisibility::Public {
        return Err(AuthHttpError::NotFound("profile not found".to_string()));
    }

    let pubkey_hex = hex::encode(&pubkey_bytes);
    Ok(Json(profile_response(
        &pubkey_hex,
        &account.forgejo_username,
        profile,
    )))
}

fn build_request_url(headers: &HeaderMap, uri: &Uri) -> Result<String, AuthHttpError> {
    let host = match headers.get("host").and_then(|value| value.to_str().ok()) {
        Some(host) => host,
        None => return Err(AuthHttpError::BadRequest("missing host header".to_string())),
    };
    let scheme = headers
        .get("x-forwarded-proto")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("http");
    let path = uri
        .path_and_query()
        .map(|value| value.as_str())
        .unwrap_or(uri.path());
    Ok(format!("{scheme}://{host}{path}"))
}

fn parse_nostr_auth(headers: &HeaderMap) -> Result<Nip98Event, AuthHttpError> {
    let value = headers
        .get(AUTH_HEADER)
        .and_then(|header| header.to_str().ok())
        .ok_or_else(|| AuthHttpError::Unauthorized("missing authorization".to_string()))?;
    let value = value.trim();
    let Some(token) = value.strip_prefix("Nostr ") else {
        return Err(AuthHttpError::Unauthorized(
            "invalid authorization header".to_string(),
        ));
    };
    let decoded = BASE64_STANDARD
        .decode(token.as_bytes())
        .map_err(|_| AuthHttpError::Unauthorized("invalid nostr authorization".to_string()))?;
    match serde_json::from_slice::<Nip98Event>(&decoded) {
        Ok(event) => Ok(event),
        Err(_) => Err(AuthHttpError::Unauthorized(
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

fn username_from_pubkey(pubkey: &str) -> Result<String, AuthHttpError> {
    if pubkey.len() != 64 || !pubkey.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(AuthHttpError::BadRequest("invalid pubkey".to_string()));
    }
    let prefix = &pubkey[..12];
    let suffix = &pubkey[pubkey.len() - 12..];
    Ok(format!("gt_{prefix}{suffix}"))
}

fn parse_pubkey_bytes(pubkey: &str) -> Result<Vec<u8>, AuthHttpError> {
    if pubkey.len() != 64 {
        return Err(AuthHttpError::BadRequest("invalid pubkey".to_string()));
    }
    hex::decode(pubkey).map_err(|_| AuthHttpError::BadRequest("invalid pubkey".to_string()))
}

fn profile_input_error(err: StorageError) -> AuthHttpError {
    match err {
        StorageError::InvalidField { .. } | StorageError::InvalidHex { .. } => {
            AuthHttpError::BadRequest(err.to_string())
        }
        _ => AuthHttpError::Internal(err.to_string()),
    }
}

fn api_visibility_from_storage(value: StorageProfileVisibility) -> ApiProfileVisibility {
    match value {
        StorageProfileVisibility::Private => ApiProfileVisibility::Private,
        StorageProfileVisibility::Public => ApiProfileVisibility::Public,
    }
}

fn storage_visibility_from_api(value: ApiProfileVisibility) -> StorageProfileVisibility {
    match value {
        ApiProfileVisibility::Private => StorageProfileVisibility::Private,
        ApiProfileVisibility::Public => StorageProfileVisibility::Public,
    }
}

fn profile_response(pubkey: &str, username: &str, profile: ProfileRecord) -> Profile {
    Profile {
        pubkey: pubkey.to_string(),
        username: username.to_string(),
        display_name: profile.display_name,
        bio: profile.bio,
        avatar_url: profile.avatar_url,
        website_url: profile.website_url,
        location: profile.location,
        visibility: api_visibility_from_storage(profile.visibility),
        created_at: profile.created_at,
        updated_at: profile.updated_at,
    }
}

fn apply_profile_update(
    pubkey: &str,
    profile: ProfileRecord,
    update: ProfileUpdate,
    now: i64,
) -> Result<ProfileRecord, StorageError> {
    let display_name = update.display_name.or(profile.display_name);
    let bio = update.bio.or(profile.bio);
    let avatar_url = update.avatar_url.or(profile.avatar_url);
    let website_url = update.website_url.or(profile.website_url);
    let location = update.location.or(profile.location);
    let visibility = update
        .visibility
        .map(storage_visibility_from_api)
        .unwrap_or(profile.visibility);
    ProfileRecord::new(
        pubkey,
        display_name,
        bio,
        avatar_url,
        website_url,
        location,
        visibility,
        profile.created_at,
        now,
    )
}

async fn ensure_profile(
    profiles: &Arc<dyn ProfileRepository>,
    pubkey: &str,
    username: &str,
    now: i64,
) -> Result<ProfileRecord, AuthHttpError> {
    let pubkey_bytes = parse_pubkey_bytes(pubkey)?;
    let existing = match profiles.profile_by_pubkey(&pubkey_bytes).await {
        Ok(existing) => existing,
        Err(err) => return Err(AuthHttpError::Internal(err.to_string())),
    };
    if let Some(existing) = existing {
        return Ok(existing);
    }

    let record = ProfileRecord::new(
        pubkey,
        Some(username.to_string()),
        None,
        None,
        None,
        None,
        StorageProfileVisibility::Private,
        now,
        now,
    )
    .map_err(profile_input_error)?;
    match profiles.upsert_profile(record.clone()).await {
        Ok(()) => {}
        Err(err) => return Err(AuthHttpError::Internal(err.to_string())),
    }
    Ok(record)
}

fn generate_password() -> String {
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    hex::encode(bytes)
}

fn build_repositories(config: &AuthServiceConfig) -> Result<PostgresRepositories, AuthError> {
    let pool_options = config.storage.pool_options().map_err(AuthError::Storage)?;
    let connect_options = config
        .storage
        .write_connect_options()
        .map_err(AuthError::Storage)?;
    let pool = pool_options.connect_lazy_with(connect_options);
    Ok(PostgresRepositories::new(pool))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::body::to_bytes;
    use axum::http::HeaderMap;
    use axum::http::Request;
    use axum::http::Uri;
    use gittree_app_core::npub_from_bytes;
    use gittree_forgejo::{ForgejoRequest, ForgejoResponse};
    use gittree_nostr_auth::NIP98_KIND;
    use secp256k1::{Keypair, Message, Secp256k1, SecretKey, XOnlyPublicKey};
    use serde_json::json;
    use std::collections::{HashMap, VecDeque};
    use std::error::Error as _;
    use std::sync::{Arc, Mutex};
    use tower::ServiceExt;

    fn storage_from_map(
        entries: &[(&'static str, &'static str)],
    ) -> Result<StorageConfig, AuthConfigError> {
        let values: HashMap<&'static str, String> = entries
            .iter()
            .map(|(key, value)| (*key, (*value).to_string()))
            .collect();
        storage_from_env_with(|key| values.get(key).cloned())
    }

    fn auth_service_config_from_map(
        entries: &[(&'static str, &'static str)],
    ) -> Result<AuthServiceConfig, AuthConfigError> {
        let values: HashMap<&'static str, String> = entries
            .iter()
            .map(|(key, value)| (*key, (*value).to_string()))
            .collect();
        AuthServiceConfig::from_env_with(|key| values.get(key).cloned())
    }

    fn is_storage_invalid_config(err: &AuthConfigError) -> bool {
        matches!(err, AuthConfigError::Storage(StorageConfigError::InvalidConfig(_)))
    }

    fn is_auth_config_missing_env(err: &AuthConfigError) -> bool {
        matches!(err, AuthConfigError::Config(ConfigError::MissingEnv(_)))
    }

    fn is_storage_internal(result: &Result<Option<AccountRecord>, StorageError>) -> bool {
        matches!(result, Err(StorageError::Internal { .. }))
    }

    fn is_forgejo_request_error(result: &Result<ForgejoResponse, ForgejoError>) -> bool {
        matches!(result, Err(ForgejoError::Request(_)))
    }

    fn is_auth_error_serve(err: &AuthError) -> bool {
        matches!(err, AuthError::Serve(_))
    }

    fn is_auth_error_storage(err: &AuthError) -> bool {
        matches!(err, AuthError::Storage(_))
    }

    fn is_auth_error_forgejo(err: &AuthError) -> bool {
        matches!(err, AuthError::Forgejo(_))
    }

    fn is_auth_error_observability_config(err: &AuthError) -> bool {
        matches!(err, AuthError::ObservabilityConfig(_))
    }

    fn is_bad_request(err: &AuthHttpError) -> bool {
        matches!(err, AuthHttpError::BadRequest(_))
    }

    fn is_unauthorized(err: &AuthHttpError) -> bool {
        matches!(err, AuthHttpError::Unauthorized(_))
    }

    fn is_internal(err: &AuthHttpError) -> bool {
        matches!(err, AuthHttpError::Internal(_))
    }

    #[test]
    fn helper_matchers_cover_non_matching_variants() {
        let config_missing = AuthConfigError::Config(ConfigError::MissingEnv("TEST"));
        assert!(is_auth_config_missing_env(&config_missing));
        assert!(!is_storage_invalid_config(&config_missing));

        let storage_invalid = AuthConfigError::Storage(StorageConfigError::InvalidConfig(
            "broken".to_string(),
        ));
        assert!(is_storage_invalid_config(&storage_invalid));
        assert!(!is_auth_config_missing_env(&storage_invalid));

        let storage_lookup_ok: Result<Option<AccountRecord>, StorageError> = Ok(None);
        assert!(!is_storage_internal(&storage_lookup_ok));
        let storage_lookup_err = Err(StorageError::Internal {
            message: "boom".to_string(),
        });
        assert!(is_storage_internal(&storage_lookup_err));

        let forgejo_ok = Ok(ForgejoResponse {
            status: 200,
            body: "{}".to_string(),
        });
        assert!(!is_forgejo_request_error(&forgejo_ok));
        let forgejo_err = Err(ForgejoError::Request("boom".to_string()));
        assert!(is_forgejo_request_error(&forgejo_err));

        let serve_err = AuthError::Serve("bind failed".to_string());
        assert!(is_auth_error_serve(&serve_err));
        assert!(!is_auth_error_storage(&serve_err));
        assert!(!is_auth_error_forgejo(&serve_err));
        assert!(!is_auth_error_observability_config(&serve_err));
        let config_err = AuthError::Config(config_missing);
        assert!(!is_auth_error_serve(&config_err));

        let bad_request = AuthHttpError::BadRequest("bad".to_string());
        assert!(is_bad_request(&bad_request));
        assert!(!is_unauthorized(&bad_request));
        assert!(!is_internal(&bad_request));
        assert!(!is_bad_request(&AuthHttpError::Unauthorized("nope".to_string())));
        assert!(is_unauthorized(&AuthHttpError::Unauthorized("nope".to_string())));
        assert!(is_internal(&AuthHttpError::Internal("boom".to_string())));
    }

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

    #[async_trait::async_trait]
    impl ForgejoTransport for MockTransport {
        async fn send(&self, request: ForgejoRequest) -> Result<ForgejoResponse, ForgejoError> {
            self.requests.lock().expect("requests").push(request);
            self.responses
                .lock()
                .expect("responses")
                .pop_front()
                .ok_or_else(|| ForgejoError::Request("missing mock response".to_string()))
        }
    }

    #[derive(Clone, Default)]
    struct ScriptedAccountRepository {
        account: Option<AccountRecord>,
        lookup_error: Option<String>,
        upsert_error: Option<String>,
    }

    #[async_trait::async_trait]
    impl AccountRepository for ScriptedAccountRepository {
        async fn upsert_account(&self, _record: AccountRecord) -> Result<(), StorageError> {
            if let Some(message) = self.upsert_error.as_ref() {
                return Err(StorageError::Internal {
                    message: message.clone(),
                });
            }
            Ok(())
        }

        async fn account_by_pubkey(
            &self,
            _pubkey: &[u8],
        ) -> Result<Option<AccountRecord>, StorageError> {
            if let Some(message) = self.lookup_error.as_ref() {
                return Err(StorageError::Internal {
                    message: message.clone(),
                });
            }
            Ok(self.account.clone())
        }

        async fn account_by_username(
            &self,
            username: &str,
        ) -> Result<Option<AccountRecord>, StorageError> {
            if let Some(message) = self.lookup_error.as_ref() {
                return Err(StorageError::Internal {
                    message: message.clone(),
                });
            }
            let account = self
                .account
                .as_ref()
                .filter(|account| account.forgejo_username == username)
                .cloned();
            Ok(account)
        }
    }

    #[derive(Clone, Default)]
    struct ScriptedProfileRepository {
        profile: Option<ProfileRecord>,
        lookup_error: Option<String>,
        upsert_error: Option<String>,
    }

    #[async_trait::async_trait]
    impl ProfileRepository for ScriptedProfileRepository {
        async fn upsert_profile(&self, _record: ProfileRecord) -> Result<(), StorageError> {
            if let Some(message) = self.upsert_error.as_ref() {
                return Err(StorageError::Internal {
                    message: message.clone(),
                });
            }
            Ok(())
        }

        async fn profile_by_pubkey(
            &self,
            _pubkey: &[u8],
        ) -> Result<Option<ProfileRecord>, StorageError> {
            if let Some(message) = self.lookup_error.as_ref() {
                return Err(StorageError::Internal {
                    message: message.clone(),
                });
            }
            Ok(self.profile.clone())
        }
    }

    fn test_config() -> ForgejoConfig {
        ForgejoConfig {
            base_url: "http://localhost:3000".to_string(),
            api_token: "token".to_string(),
            owner: "gittree".to_string(),
            webhook_url: "http://localhost:8090/".to_string(),
            webhook_secret: "secret".to_string(),
            repo_private: true,
        }
    }

    fn user_json(username: &str) -> String {
        format!(
            r#"{{"login":"{username}","username":"{username}","email":"{username}@example.com"}}"#
        )
    }

    fn test_state(
        responses: Vec<ForgejoResponse>,
    ) -> (
        AuthAppState,
        Arc<gittree_storage::InMemoryRepositories>,
        Arc<MockTransport>,
    ) {
        let transport = Arc::new(MockTransport::new(responses));
        let transport_dyn: Arc<dyn ForgejoTransport> = transport.clone();
        let forgejo = ForgejoClient::with_transport(test_config(), transport_dyn);
        let repositories = Arc::new(gittree_storage::InMemoryRepositories::new());
        let state = AuthAppState {
            auth: AuthSettings {
                email_domain: "example.com".to_string(),
                max_skew_seconds: 60,
            },
            forgejo,
            accounts: repositories.clone(),
            profiles: repositories.clone(),
        };
        (state, repositories, transport)
    }

    fn scripted_state(
        responses: Vec<ForgejoResponse>,
        accounts: Arc<dyn AccountRepository>,
        profiles: Arc<dyn ProfileRepository>,
    ) -> (AuthAppState, Arc<MockTransport>) {
        let transport = Arc::new(MockTransport::new(responses));
        let transport_dyn: Arc<dyn ForgejoTransport> = transport.clone();
        let forgejo = ForgejoClient::with_transport(test_config(), transport_dyn);
        let state = AuthAppState {
            auth: AuthSettings {
                email_domain: "example.com".to_string(),
                max_skew_seconds: 60,
            },
            forgejo,
            accounts,
            profiles,
        };
        (state, transport)
    }

    fn signed_event(
        url: &str,
        method: &str,
        created_at: i64,
        payload_hash: Option<&str>,
    ) -> Nip98Event {
        let secp = Secp256k1::new();
        let secret_key = SecretKey::from_slice(&[2u8; 32]).expect("secret");
        let keypair = Keypair::from_secret_key(&secp, &secret_key);
        let (pubkey, _) = XOnlyPublicKey::from_keypair(&keypair);
        let pubkey_hex = hex::encode(pubkey.serialize());
        let mut tags = vec![
            vec!["u".to_string(), url.to_string()],
            vec!["method".to_string(), method.to_string()],
        ];
        if let Some(payload) = payload_hash {
            tags.push(vec!["payload".to_string(), payload.to_string()]);
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

    #[test]
    fn storage_from_env_with_uses_defaults_for_pool_limits() {
        let config = storage_from_map(&[(
            ENV_STORAGE_READ_URL,
            "postgres://user:pass@localhost:5432/gittree",
        )])
        .expect("config");
        assert_eq!(config.max_connections, 10);
        assert_eq!(config.min_connections, 2);
        assert_eq!(
            config.read_connection,
            "postgres://user:pass@localhost:5432/gittree"
        );
        assert!(config.idle_timeout_secs.is_none());
        assert!(config.max_lifetime_secs.is_none());
    }

    #[test]
    fn storage_from_env_requires_read_connection() {
        let err = storage_from_env_with(|_| None).unwrap_err();
        assert!(matches!(
            err,
            AuthConfigError::Storage(StorageConfigError::MissingEnv(ENV_STORAGE_READ_URL))
        ));
        assert_eq!(
            format!("{err}"),
            format!("auth storage config error: missing env {ENV_STORAGE_READ_URL}")
        );
        assert!(err.source().is_some());
    }

    #[test]
    fn storage_from_env_rejects_invalid_numeric_values() {
        let err = storage_from_map(&[
            (
                ENV_STORAGE_READ_URL,
                "postgres://user:pass@localhost:5432/gittree",
            ),
            (ENV_STORAGE_MAX_CONNECTIONS, "not-a-number"),
        ])
        .unwrap_err();
        assert!(matches!(
            err,
            AuthConfigError::Storage(StorageConfigError::InvalidEnv {
                key: ENV_STORAGE_MAX_CONNECTIONS,
                ..
            })
        ));
    }

    #[test]
    fn storage_from_env_rejects_invalid_min_connections() {
        let err = storage_from_map(&[
            (
                ENV_STORAGE_READ_URL,
                "postgres://user:pass@localhost:5432/gittree",
            ),
            (ENV_STORAGE_MIN_CONNECTIONS, "not-a-number"),
        ])
        .unwrap_err();
        assert!(matches!(
            err,
            AuthConfigError::Storage(StorageConfigError::InvalidEnv {
                key: ENV_STORAGE_MIN_CONNECTIONS,
                ..
            })
        ));
    }

    #[test]
    fn storage_from_env_rejects_invalid_max_lifetime() {
        let err = storage_from_map(&[
            (
                ENV_STORAGE_READ_URL,
                "postgres://user:pass@localhost:5432/gittree",
            ),
            (ENV_STORAGE_MAX_LIFETIME_SECS, "not-a-number"),
        ])
        .unwrap_err();
        assert!(matches!(
            err,
            AuthConfigError::Storage(StorageConfigError::InvalidEnv {
                key: ENV_STORAGE_MAX_LIFETIME_SECS,
                ..
            })
        ));
    }

    #[test]
    fn storage_from_env_rejects_invalid_idle_timeout() {
        let err = storage_from_map(&[
            (
                ENV_STORAGE_READ_URL,
                "postgres://user:pass@localhost:5432/gittree",
            ),
            (ENV_STORAGE_IDLE_TIMEOUT_SECS, "not-a-number"),
        ])
        .unwrap_err();
        assert!(matches!(
            err,
            AuthConfigError::Storage(StorageConfigError::InvalidEnv {
                key: ENV_STORAGE_IDLE_TIMEOUT_SECS,
                ..
            })
        ));
    }

    #[test]
    fn storage_from_env_rejects_invalid_pool_configuration() {
        let err = storage_from_map(&[
            (
                ENV_STORAGE_READ_URL,
                "postgres://user:pass@localhost:5432/gittree",
            ),
            (ENV_STORAGE_MAX_CONNECTIONS, "1"),
            (ENV_STORAGE_MIN_CONNECTIONS, "2"),
        ])
        .unwrap_err();
        assert!(is_storage_invalid_config(&err));
        assert!(err.to_string().contains("min_connections"));
    }

    #[test]
    fn env_parsers_treat_empty_values_as_absent() {
        let empty_u32 = env_u32_with(ENV_STORAGE_MAX_CONNECTIONS, |_| Some("   ".to_string()))
            .expect("empty u32");
        assert!(empty_u32.is_none());
        let empty_u64 = env_u64_with(ENV_STORAGE_IDLE_TIMEOUT_SECS, |_| Some(String::new()))
            .expect("empty u64");
        assert!(empty_u64.is_none());
    }

    #[test]
    fn env_u64_with_rejects_invalid_values() {
        let err =
            env_u64_with(ENV_STORAGE_IDLE_TIMEOUT_SECS, |_| Some("bad".to_string())).unwrap_err();
        assert!(matches!(
            err,
            AuthConfigError::Storage(StorageConfigError::InvalidEnv {
                key: ENV_STORAGE_IDLE_TIMEOUT_SECS,
                ..
            })
        ));
    }

    #[test]
    fn storage_config_error_display_variants_are_stable() {
        let invalid_env = StorageConfigError::InvalidEnv {
            key: ENV_STORAGE_MAX_CONNECTIONS,
            value: "bad".to_string(),
        };
        assert_eq!(
            invalid_env.to_string(),
            format!("invalid env {ENV_STORAGE_MAX_CONNECTIONS}: bad")
        );

        let invalid_config = StorageConfigError::InvalidConfig("broken".to_string());
        assert_eq!(invalid_config.to_string(), "broken");
    }

    #[test]
    fn auth_service_config_from_env_with_loads_required_sections() {
        let config = auth_service_config_from_map(&[
            ("GITTREE_AUTH_BIND", "127.0.0.1:18089"),
            ("GITTREE_AUTH_EMAIL_DOMAIN", "local.test"),
            ("GITTREE_AUTH_MAX_SKEW_SECONDS", "42"),
            ("GITTREE_FORGEJO_BASE_URL", "http://localhost:3000"),
            ("GITTREE_FORGEJO_API_TOKEN", "token"),
            ("GITTREE_FORGEJO_OWNER", "gittree"),
            ("GITTREE_FORGEJO_WEBHOOK_URL", "http://localhost:8087"),
            ("GITTREE_FORGEJO_WEBHOOK_SECRET", "secret"),
            (
                ENV_STORAGE_READ_URL,
                "postgres://user:pass@localhost:5432/gittree",
            ),
        ])
        .expect("auth config");
        assert_eq!(config.bind, "127.0.0.1:18089");
        assert_eq!(config.auth.email_domain, "local.test");
        assert_eq!(config.auth.max_skew_seconds, 42);
        assert_eq!(config.forgejo.owner, "gittree");
        assert_eq!(
            config.storage.read_connection,
            "postgres://user:pass@localhost:5432/gittree"
        );
    }

    #[test]
    fn auth_service_config_from_env_with_maps_config_error() {
        let err =
            auth_service_config_from_map(&[(ENV_STORAGE_READ_URL, "postgres://localhost/gittree")])
                .expect_err("missing forgejo config");
        assert!(is_auth_config_missing_env(&err));
    }

    #[test]
    fn auth_service_config_from_env_with_maps_services_config_error() {
        let err = auth_service_config_from_map(&[
            ("GITTREE_AUTH_BIND", "bad-bind"),
            (ENV_STORAGE_READ_URL, "postgres://localhost/gittree"),
        ])
        .expect_err("invalid service bind should fail");
        assert!(matches!(
            err,
            AuthConfigError::Config(ConfigError::InvalidServiceBind {
                service: "auth",
                ..
            })
        ));
    }

    #[test]
    fn auth_service_config_from_env_with_maps_auth_settings_error() {
        let err = auth_service_config_from_map(&[
            ("GITTREE_AUTH_MAX_SKEW_SECONDS", "bad"),
            ("GITTREE_FORGEJO_BASE_URL", "http://localhost:3000"),
            ("GITTREE_FORGEJO_API_TOKEN", "token"),
            ("GITTREE_FORGEJO_OWNER", "gittree"),
            ("GITTREE_FORGEJO_WEBHOOK_URL", "http://localhost:8087"),
            ("GITTREE_FORGEJO_WEBHOOK_SECRET", "secret"),
            (ENV_STORAGE_READ_URL, "postgres://localhost/gittree"),
        ])
        .expect_err("invalid auth settings should fail");
        assert!(matches!(
            err,
            AuthConfigError::Config(ConfigError::InvalidConfig {
                field: "auth.max_skew_seconds",
                ..
            })
        ));
    }

    #[test]
    fn auth_service_config_from_env_with_maps_storage_error() {
        let err = auth_service_config_from_map(&[
            ("GITTREE_AUTH_BIND", "127.0.0.1:18089"),
            ("GITTREE_AUTH_EMAIL_DOMAIN", "local.test"),
            ("GITTREE_AUTH_MAX_SKEW_SECONDS", "42"),
            ("GITTREE_FORGEJO_BASE_URL", "http://localhost:3000"),
            ("GITTREE_FORGEJO_API_TOKEN", "token"),
            ("GITTREE_FORGEJO_OWNER", "gittree"),
            ("GITTREE_FORGEJO_WEBHOOK_URL", "http://localhost:8087"),
            ("GITTREE_FORGEJO_WEBHOOK_SECRET", "secret"),
            (
                ENV_STORAGE_READ_URL,
                "postgres://user:pass@localhost:5432/gittree",
            ),
            (ENV_STORAGE_MAX_LIFETIME_SECS, "bad"),
        ])
        .expect_err("storage parse error");
        assert!(matches!(
            err,
            AuthConfigError::Storage(StorageConfigError::InvalidEnv {
                key: ENV_STORAGE_MAX_LIFETIME_SECS,
                ..
            })
        ));
    }

    #[test]
    fn auth_service_config_from_env_returns_result_without_panicking() {
        let _ = AuthServiceConfig::from_env();
    }

    #[test]
    fn auth_config_error_config_variant_exposes_source() {
        let err = AuthConfigError::Config(ConfigError::MissingEnv("GITTREE_AUTH_BIND"));
        assert_eq!(
            err.to_string(),
            "auth config error: missing env GITTREE_AUTH_BIND"
        );
        assert!(err.source().is_some());
    }

    #[test]
    fn auth_config_error_storage_variant_exposes_source() {
        let err = AuthConfigError::Storage(StorageConfigError::InvalidConfig("broken".to_string()));
        assert_eq!(err.to_string(), "auth storage config error: broken");
        assert!(err.source().is_some());
    }

    #[test]
    fn env_u64_with_parses_valid_values() {
        let parsed = env_u64_with(ENV_STORAGE_IDLE_TIMEOUT_SECS, |_| Some("120".to_string()))
            .expect("valid u64");
        assert_eq!(parsed, Some(120));
    }

    #[tokio::test]
    async fn scripted_account_repository_account_by_username_handles_match_and_miss() {
        let account = AccountRecord::new("11".repeat(32).as_str(), "alice").expect("account");
        let repository = ScriptedAccountRepository {
            account: Some(account),
            ..ScriptedAccountRepository::default()
        };
        let hit = AccountRepository::account_by_username(&repository, "alice")
            .await
            .expect("lookup");
        assert!(hit.is_some());
        let miss = AccountRepository::account_by_username(&repository, "bob")
            .await
            .expect("lookup");
        assert!(miss.is_none());
    }

    #[tokio::test]
    async fn scripted_account_repository_account_by_username_surfaces_lookup_error() {
        let repository = ScriptedAccountRepository {
            lookup_error: Some("lookup failed".to_string()),
            ..ScriptedAccountRepository::default()
        };
        let result = AccountRepository::account_by_username(&repository, "alice").await;
        assert!(is_storage_internal(&result));
    }

    #[test]
    fn auth_error_display_and_source_cover_all_variants() {
        let config = AuthError::Config(AuthConfigError::Config(ConfigError::MissingEnv(
            "GITTREE_FORGEJO_BASE_URL",
        )));
        assert_eq!(
            format!("{config}"),
            "auth error: auth config error: missing env GITTREE_FORGEJO_BASE_URL"
        );
        assert!(config.source().is_some());

        let forgejo = AuthError::Forgejo(ForgejoError::Request("boom".to_string()));
        assert_eq!(
            format!("{forgejo}"),
            "auth forgejo error: forgejo request error: boom"
        );
        assert!(forgejo.source().is_some());

        let storage = AuthError::Storage(StorageError::Internal {
            message: "storage down".to_string(),
        });
        assert_eq!(
            format!("{storage}"),
            "auth storage error: internal error: storage down"
        );
        assert!(storage.source().is_some());

        let observability_config =
            AuthError::ObservabilityConfig(ObservabilityConfigError::InvalidEnv {
                key: "GITTREE_LOG_JSON",
                value: "maybe".to_string(),
            });
        assert_eq!(
            format!("{observability_config}"),
            "auth observability config error: invalid env GITTREE_LOG_JSON: maybe"
        );
        assert!(observability_config.source().is_some());

        let observability =
            AuthError::Observability(ObservabilityError::LogInit("boom".to_string()));
        assert_eq!(
            format!("{observability}"),
            "auth observability error: observability log init failed: boom"
        );
        assert!(observability.source().is_some());

        let serve = AuthError::Serve("bind failed".to_string());
        assert_eq!(format!("{serve}"), "auth serve error: bind failed");
        assert!(serve.source().is_none());
    }

    #[tokio::test]
    async fn build_repositories_constructs_lazy_pool_from_storage_config() {
        let config = AuthServiceConfig {
            bind: "127.0.0.1:0".to_string(),
            auth: AuthSettings {
                email_domain: "example.com".to_string(),
                max_skew_seconds: 60,
            },
            forgejo: test_config(),
            storage: StorageConfig {
                read_connection: "postgres://user:pass@localhost:5432/gittree".to_string(),
                write_connection: None,
                max_connections: 10,
                min_connections: 2,
                idle_timeout_secs: None,
                max_lifetime_secs: None,
                application_name: None,
            },
        };
        let repos = build_repositories(&config).expect("repositories");
        let _ = repos;
    }

    #[test]
    fn build_repositories_maps_invalid_pool_options_to_auth_error() {
        let config = AuthServiceConfig {
            bind: "127.0.0.1:0".to_string(),
            auth: AuthSettings {
                email_domain: "example.com".to_string(),
                max_skew_seconds: 60,
            },
            forgejo: test_config(),
            storage: StorageConfig {
                read_connection: "postgres://user:pass@localhost:5432/gittree".to_string(),
                write_connection: None,
                max_connections: 1,
                min_connections: 2,
                idle_timeout_secs: None,
                max_lifetime_secs: None,
                application_name: None,
            },
        };
        let err = build_repositories(&config).expect_err("invalid pool bounds should fail");
        assert!(err.to_string().contains("auth storage error"));
    }

    #[test]
    fn build_repositories_maps_invalid_write_connection_to_auth_error() {
        let config = AuthServiceConfig {
            bind: "127.0.0.1:0".to_string(),
            auth: AuthSettings {
                email_domain: "example.com".to_string(),
                max_skew_seconds: 60,
            },
            forgejo: test_config(),
            storage: StorageConfig {
                read_connection: "postgres://user:pass@localhost:5432/gittree".to_string(),
                write_connection: Some("not a url".to_string()),
                max_connections: 10,
                min_connections: 2,
                idle_timeout_secs: None,
                max_lifetime_secs: None,
                application_name: None,
            },
        };
        let err = build_repositories(&config).expect_err("invalid write url should fail");
        assert!(err.to_string().contains("auth storage error"));
    }

    #[test]
    fn mock_transport_returns_error_when_response_queue_is_empty() {
        let transport = MockTransport::new(Vec::new());
        let request = ForgejoRequest {
            method: gittree_forgejo::ForgejoMethod::Get,
            url: "http://localhost/api/v1/users/alice".to_string(),
            body: None,
        };
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let result = runtime.block_on(async { transport.send(request).await });
        assert!(is_forgejo_request_error(&result));
    }

    #[tokio::test]
    async fn serve_starts_and_can_be_aborted() {
        let config = AuthServiceConfig {
            bind: "127.0.0.1:0".to_string(),
            auth: AuthSettings {
                email_domain: "example.com".to_string(),
                max_skew_seconds: 60,
            },
            forgejo: test_config(),
            storage: StorageConfig {
                read_connection: "postgres://user:pass@localhost:5432/gittree".to_string(),
                write_connection: None,
                max_connections: 10,
                min_connections: 2,
                idle_timeout_secs: None,
                max_lifetime_secs: None,
                application_name: None,
            },
        };
        let handle = tokio::spawn(serve(config));
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(!handle.is_finished());
        handle.abort();
        let join_error = handle.await.expect_err("join should be cancelled");
        assert!(join_error.is_cancelled());
    }

    #[tokio::test]
    async fn serve_returns_storage_error_for_invalid_pool_config() {
        let config = AuthServiceConfig {
            bind: "127.0.0.1:0".to_string(),
            auth: AuthSettings {
                email_domain: "example.com".to_string(),
                max_skew_seconds: 60,
            },
            forgejo: test_config(),
            storage: StorageConfig {
                read_connection: "postgres://user:pass@localhost:5432/gittree".to_string(),
                write_connection: None,
                max_connections: 1,
                min_connections: 2,
                idle_timeout_secs: None,
                max_lifetime_secs: None,
                application_name: None,
            },
        };
        let err = super::serve_without_observability(config)
            .await
            .expect_err("storage error");
        assert!(is_auth_error_storage(&err));
    }

    #[test]
    fn init_observability_reports_config_error_for_invalid_env() {
        let err = super::init_observability_with(|| {
            Err(ObservabilityConfigError::InvalidEnv {
                key: "GITTREE_LOG_JSON",
                value: "not-a-bool".to_string(),
            })
        })
        .expect_err("invalid observability env");
        assert!(is_auth_error_observability_config(&err));
    }

    #[tokio::test]
    async fn serve_without_observability_maps_invalid_forgejo_api_token() {
        let config = AuthServiceConfig {
            bind: "127.0.0.1:0".to_string(),
            auth: AuthSettings {
                email_domain: "example.com".to_string(),
                max_skew_seconds: 60,
            },
            forgejo: ForgejoConfig {
                api_token: "   ".to_string(),
                ..test_config()
            },
            storage: StorageConfig {
                read_connection: "postgres://user:pass@localhost:5432/gittree".to_string(),
                write_connection: None,
                max_connections: 10,
                min_connections: 2,
                idle_timeout_secs: None,
                max_lifetime_secs: None,
                application_name: None,
            },
        };
        let err = super::serve_without_observability(config)
            .await
            .expect_err("forgejo token error");
        assert!(is_auth_error_forgejo(&err));
    }

    #[tokio::test]
    async fn run_server_returns_ok_when_server_future_is_ok() {
        super::run_server(async { Ok::<(), &'static str>(()) })
            .await
            .expect("server");
    }

    #[tokio::test]
    async fn run_server_maps_errors_to_auth_serve_error() {
        let err = super::run_server(async { Err::<(), &'static str>("boom") })
            .await
            .expect_err("serve error");
        assert!(matches!(err, AuthError::Serve(message) if message == "boom"));
    }

    #[tokio::test]
    async fn serve_returns_serve_error_for_invalid_bind() {
        let (state, _repos, _transport) = test_state(Vec::new());
        let router = build_router(state);
        let err = serve_inner("not-a-socket", router)
            .await
            .expect_err("bind error");
        assert!(is_auth_error_serve(&err));
    }

    #[tokio::test]
    async fn signup_rejects_missing_auth() {
        let (state, _repos, _transport) = test_state(Vec::new());
        let app = build_router(state);
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/signup")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn signup_rejects_missing_host_header() {
        let now = unix_timestamp();
        let url = "http://localhost/v1/signup";
        let event = signed_event(url, "POST", now, None);
        let (state, _repos, _transport) = test_state(Vec::new());
        let app = build_router(state);
        let token = BASE64_STANDARD.encode(serde_json::to_vec(&event).expect("event json"));
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/signup")
                    .header(AUTH_HEADER, format!("Nostr {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn signup_rejects_method_mismatch() {
        let now = unix_timestamp();
        let url = "http://localhost/v1/signup";
        let event = signed_event(url, "GET", now, None);
        let (state, _repos, _transport) = test_state(Vec::new());
        let app = build_router(state);
        let token = BASE64_STANDARD.encode(serde_json::to_vec(&event).expect("event json"));
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/signup")
                    .header("host", "localhost")
                    .header(AUTH_HEADER, format!("Nostr {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn health_endpoint_returns_ok() {
        let (state, _repos, _transport) = test_state(Vec::new());
        let app = build_router(state);
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        assert_eq!(body, Bytes::from_static(b"ok"));
    }

    #[tokio::test]
    async fn reqwest_state_routes_reject_unauthorized_before_network_io() {
        let (state, _repos, transport) = test_state(Vec::new());
        let app = build_router(state);

        let signup_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/signup")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("signup response");
        assert_eq!(signup_response.status(), StatusCode::UNAUTHORIZED);

        let profile_get_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/v1/profile")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("profile get response");
        assert_eq!(profile_get_response.status(), StatusCode::UNAUTHORIZED);

        let profile_patch_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri("/v1/profile")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("profile patch response");
        assert_eq!(profile_patch_response.status(), StatusCode::UNAUTHORIZED);

        let public_profile_response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/v1/profile/not-an-npub")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("profile public response");
        assert_eq!(public_profile_response.status(), StatusCode::BAD_REQUEST);
        assert!(transport.requests().is_empty());
    }

    #[tokio::test]
    async fn signup_returns_internal_when_forgejo_responses_are_incomplete() {
        let now = unix_timestamp();
        let url = "http://localhost/v1/signup";
        let event = signed_event(url, "POST", now, None);
        let responses = vec![ForgejoResponse {
            status: 404,
            body: String::new(),
        }];
        let (state, _repos, _transport) = test_state(responses);
        let app = build_router(state);
        let token = BASE64_STANDARD.encode(serde_json::to_vec(&event).expect("event json"));
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/signup")
                    .header("host", "localhost")
                    .header(AUTH_HEADER, format!("Nostr {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[tokio::test]
    async fn signup_returns_internal_when_account_lookup_fails() {
        let now = unix_timestamp();
        let url = "http://localhost/v1/signup";
        let event = signed_event(url, "POST", now, None);
        let accounts: Arc<dyn AccountRepository> = Arc::new(ScriptedAccountRepository {
            lookup_error: Some("account lookup failed".to_string()),
            ..ScriptedAccountRepository::default()
        });
        let profiles: Arc<dyn ProfileRepository> = Arc::new(ScriptedProfileRepository::default());
        let (state, _transport) = scripted_state(Vec::new(), accounts, profiles);
        let app = build_router(state);
        let token = BASE64_STANDARD.encode(serde_json::to_vec(&event).expect("event json"));
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/signup")
                    .header("host", "localhost")
                    .header(AUTH_HEADER, format!("Nostr {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[tokio::test]
    async fn signup_rejects_empty_username_from_forgejo_response() {
        let now = unix_timestamp();
        let url = "http://localhost/v1/signup";
        let event = signed_event(url, "POST", now, None);
        let responses = vec![
            ForgejoResponse {
                status: 404,
                body: String::new(),
            },
            ForgejoResponse {
                status: 201,
                body: r#"{"login":"","username":"","email":"empty@example.com"}"#.to_string(),
            },
        ];
        let (state, _repos, _transport) = test_state(responses);
        let app = build_router(state);
        let token = BASE64_STANDARD.encode(serde_json::to_vec(&event).expect("event json"));
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/signup")
                    .header("host", "localhost")
                    .header(AUTH_HEADER, format!("Nostr {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn signup_rejects_profile_creation_for_invalid_forgejo_username() {
        let now = unix_timestamp();
        let url = "http://localhost/v1/signup";
        let event = signed_event(url, "POST", now, None);
        let long_username = "a".repeat(256);
        let responses = vec![
            ForgejoResponse {
                status: 404,
                body: String::new(),
            },
            ForgejoResponse {
                status: 201,
                body: user_json(&long_username),
            },
        ];
        let (state, _repos, _transport) = test_state(responses);
        let app = build_router(state);
        let token = BASE64_STANDARD.encode(serde_json::to_vec(&event).expect("event json"));
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/signup")
                    .header("host", "localhost")
                    .header(AUTH_HEADER, format!("Nostr {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn signup_returns_internal_when_account_upsert_fails() {
        let now = unix_timestamp();
        let url = "http://localhost/v1/signup";
        let event = signed_event(url, "POST", now, None);
        let username = username_from_pubkey(&event.pubkey).expect("username");
        let responses = vec![
            ForgejoResponse {
                status: 404,
                body: String::new(),
            },
            ForgejoResponse {
                status: 201,
                body: user_json(&username),
            },
        ];
        let accounts: Arc<dyn AccountRepository> = Arc::new(ScriptedAccountRepository {
            upsert_error: Some("account upsert failed".to_string()),
            ..ScriptedAccountRepository::default()
        });
        let profiles: Arc<dyn ProfileRepository> = Arc::new(ScriptedProfileRepository::default());
        let (state, _transport) = scripted_state(responses, accounts, profiles);
        let app = build_router(state);
        let token = BASE64_STANDARD.encode(serde_json::to_vec(&event).expect("event json"));
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/signup")
                    .header("host", "localhost")
                    .header(AUTH_HEADER, format!("Nostr {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[tokio::test]
    async fn signup_returns_internal_when_profile_upsert_fails() {
        let now = unix_timestamp();
        let url = "http://localhost/v1/signup";
        let event = signed_event(url, "POST", now, None);
        let username = username_from_pubkey(&event.pubkey).expect("username");
        let responses = vec![
            ForgejoResponse {
                status: 404,
                body: String::new(),
            },
            ForgejoResponse {
                status: 201,
                body: user_json(&username),
            },
        ];
        let accounts: Arc<dyn AccountRepository> = Arc::new(ScriptedAccountRepository::default());
        let profiles: Arc<dyn ProfileRepository> = Arc::new(ScriptedProfileRepository {
            upsert_error: Some("profile upsert failed".to_string()),
            ..ScriptedProfileRepository::default()
        });
        let (state, _transport) = scripted_state(responses, accounts, profiles);
        let app = build_router(state);
        let token = BASE64_STANDARD.encode(serde_json::to_vec(&event).expect("event json"));
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/signup")
                    .header("host", "localhost")
                    .header(AUTH_HEADER, format!("Nostr {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[test]
    fn auth_http_error_internal_maps_to_500() {
        let response = AuthHttpError::Internal("boom".to_string()).into_response();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[test]
    fn auth_http_error_status_mappings_cover_all_variants() {
        let unauthorized = AuthHttpError::Unauthorized("nope".to_string()).into_response();
        assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);

        let bad_request = AuthHttpError::BadRequest("bad".to_string()).into_response();
        assert_eq!(bad_request.status(), StatusCode::BAD_REQUEST);

        let not_found = AuthHttpError::NotFound("missing".to_string()).into_response();
        assert_eq!(not_found.status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn build_request_url_prefers_forwarded_proto() {
        let mut headers = HeaderMap::new();
        headers.insert("host", "gittr.ee".parse().expect("host"));
        headers.insert("x-forwarded-proto", "https".parse().expect("proto"));
        let uri: Uri = "/v1/signup?foo=bar".parse().expect("uri");
        let url = build_request_url(&headers, &uri).expect("url");
        assert_eq!(url, "https://gittr.ee/v1/signup?foo=bar");
    }

    #[test]
    fn build_request_url_requires_host_header() {
        let headers = HeaderMap::new();
        let uri: Uri = "/v1/signup".parse().expect("uri");
        let err = build_request_url(&headers, &uri).unwrap_err();
        assert!(is_bad_request(&err));
    }

    #[test]
    fn parse_nostr_auth_rejects_invalid_scheme() {
        let mut headers = HeaderMap::new();
        headers.insert(AUTH_HEADER, "Bearer token".parse().expect("auth"));
        let err = parse_nostr_auth(&headers).unwrap_err();
        assert!(is_unauthorized(&err));
    }

    #[test]
    fn parse_nostr_auth_rejects_invalid_base64() {
        let mut headers = HeaderMap::new();
        headers.insert(AUTH_HEADER, "Nostr !!!".parse().expect("auth"));
        let err = parse_nostr_auth(&headers).unwrap_err();
        assert!(is_unauthorized(&err));
    }

    #[test]
    fn parse_nostr_auth_rejects_invalid_event_json() {
        let mut headers = HeaderMap::new();
        let token = BASE64_STANDARD.encode(br#"{"invalid":"event"}"#);
        headers.insert(AUTH_HEADER, format!("Nostr {token}").parse().expect("auth"));
        let err = parse_nostr_auth(&headers).unwrap_err();
        assert!(is_unauthorized(&err));
    }

    #[test]
    fn payload_hash_handles_empty_and_non_empty_bodies() {
        assert!(payload_hash(&Bytes::new()).is_none());
        let digest = payload_hash(&Bytes::from_static(b"hello")).expect("digest");
        assert_eq!(digest.len(), 64);
    }

    #[test]
    fn unix_timestamp_returns_non_negative_epoch_seconds() {
        let now = unix_timestamp();
        assert!(now >= 0);
        assert!(now >= 1_600_000_000);
    }

    #[test]
    fn generate_password_returns_hex_with_expected_length() {
        let password = generate_password();
        assert_eq!(password.len(), 64);
        assert!(password.chars().all(|ch| ch.is_ascii_hexdigit()));
    }

    #[test]
    fn profile_response_maps_storage_fields() {
        let record = ProfileRecord::new(
            &"aa".repeat(32),
            Some("Alice".to_string()),
            Some("Bio".to_string()),
            Some("https://gittr.ee/avatar.png".to_string()),
            Some("https://gittr.ee".to_string()),
            Some("earth".to_string()),
            StorageProfileVisibility::Public,
            100,
            110,
        )
        .expect("profile");
        let response = profile_response(&"aa".repeat(32), "gt_alice", record);
        assert_eq!(response.username, "gt_alice");
        assert_eq!(response.visibility, ApiProfileVisibility::Public);
        assert_eq!(response.display_name, Some("Alice".to_string()));
        assert_eq!(response.bio, Some("Bio".to_string()));
        assert_eq!(response.created_at, 100);
        assert_eq!(response.updated_at, 110);
    }

    #[test]
    fn profile_visibility_mapping_round_trip() {
        assert_eq!(
            api_visibility_from_storage(StorageProfileVisibility::Private),
            ApiProfileVisibility::Private
        );
        assert_eq!(
            api_visibility_from_storage(StorageProfileVisibility::Public),
            ApiProfileVisibility::Public
        );
        assert_eq!(
            storage_visibility_from_api(ApiProfileVisibility::Private),
            StorageProfileVisibility::Private
        );
        assert_eq!(
            storage_visibility_from_api(ApiProfileVisibility::Public),
            StorageProfileVisibility::Public
        );
    }

    #[test]
    fn profile_input_error_maps_invalid_and_internal() {
        let invalid = profile_input_error(StorageError::InvalidField {
            field: "display_name",
            value: "bad".to_string(),
        });
        assert!(is_bad_request(&invalid));

        let internal = profile_input_error(StorageError::Internal {
            message: "boom".to_string(),
        });
        assert!(is_internal(&internal));
    }

    #[tokio::test]
    async fn ensure_profile_returns_existing_without_rewrite() {
        let repositories = Arc::new(gittree_storage::InMemoryRepositories::new());
        let profiles: Arc<dyn ProfileRepository> = repositories.clone();
        let pubkey = "aa".repeat(32);
        let existing = ProfileRecord::new(
            &pubkey,
            Some("alice".to_string()),
            Some("bio".to_string()),
            None,
            None,
            None,
            StorageProfileVisibility::Public,
            100,
            110,
        )
        .expect("profile");
        repositories
            .upsert_profile(existing.clone())
            .await
            .expect("upsert");

        let result = ensure_profile(&profiles, &pubkey, "ignored", 200)
            .await
            .expect("ensure");
        assert_eq!(result, existing);
    }

    #[tokio::test]
    async fn ensure_profile_creates_default_when_missing() {
        let repositories = Arc::new(gittree_storage::InMemoryRepositories::new());
        let profiles: Arc<dyn ProfileRepository> = repositories.clone();
        let pubkey = "ab".repeat(32);
        let created = ensure_profile(&profiles, &pubkey, "gt_test", 300)
            .await
            .expect("ensure");
        assert_eq!(created.visibility, StorageProfileVisibility::Private);
        assert_eq!(created.display_name, Some("gt_test".to_string()));

        let pubkey_bytes = hex::decode(&pubkey).expect("pubkey");
        let stored = repositories
            .profile_by_pubkey(&pubkey_bytes)
            .await
            .expect("repo")
            .expect("stored profile");
        assert_eq!(stored, created);
    }

    #[tokio::test]
    async fn ensure_profile_returns_internal_when_lookup_fails() {
        let profiles: Arc<dyn ProfileRepository> = Arc::new(ScriptedProfileRepository {
            lookup_error: Some("lookup failed".to_string()),
            ..ScriptedProfileRepository::default()
        });
        let pubkey = "ac".repeat(32);
        let err = ensure_profile(&profiles, &pubkey, "gt_test", 300)
            .await
            .expect_err("lookup error");
        assert!(
            matches!(err, AuthHttpError::Internal(message) if message.contains("lookup failed"))
        );
    }

    #[tokio::test]
    async fn ensure_profile_returns_internal_when_upsert_fails() {
        let profiles: Arc<dyn ProfileRepository> = Arc::new(ScriptedProfileRepository {
            upsert_error: Some("upsert failed".to_string()),
            ..ScriptedProfileRepository::default()
        });
        let pubkey = "ad".repeat(32);
        let err = ensure_profile(&profiles, &pubkey, "gt_test", 300)
            .await
            .expect_err("upsert error");
        assert!(
            matches!(err, AuthHttpError::Internal(message) if message.contains("upsert failed"))
        );
    }

    #[tokio::test]
    async fn ensure_profile_rejects_invalid_pubkey() {
        let repositories = Arc::new(gittree_storage::InMemoryRepositories::new());
        let profiles: Arc<dyn ProfileRepository> = repositories;
        let err = ensure_profile(&profiles, "bad-pubkey", "gt_test", 300)
            .await
            .expect_err("invalid pubkey should fail");
        assert!(is_bad_request(&err));
    }

    #[tokio::test]
    async fn ensure_profile_rejects_invalid_profile_payload() {
        let repositories = Arc::new(gittree_storage::InMemoryRepositories::new());
        let profiles: Arc<dyn ProfileRepository> = repositories;
        let pubkey = "ae".repeat(32);
        let long_username = "a".repeat(256);
        let err = ensure_profile(&profiles, &pubkey, &long_username, 300)
            .await
            .expect_err("invalid profile payload should fail");
        assert!(is_bad_request(&err));
    }

    #[test]
    fn username_and_pubkey_parsing_reject_invalid_values() {
        let username_err = username_from_pubkey("bad").unwrap_err();
        assert!(is_bad_request(&username_err));
        let pubkey_err = parse_pubkey_bytes("bad").unwrap_err();
        assert!(is_bad_request(&pubkey_err));
        let decode_err = parse_pubkey_bytes(&"gg".repeat(32)).unwrap_err();
        assert!(is_bad_request(&decode_err));
    }

    #[test]
    fn apply_profile_update_preserves_existing_fields() {
        let now = 1000;
        let existing = ProfileRecord::new(
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            Some("Alice".to_string()),
            Some("Bio".to_string()),
            None,
            None,
            None,
            StorageProfileVisibility::Private,
            now,
            now,
        )
        .expect("profile");
        let updated = apply_profile_update(
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            existing.clone(),
            ProfileUpdate::default(),
            now + 10,
        )
        .expect("updated");
        assert_eq!(updated.display_name, existing.display_name);
        assert_eq!(updated.bio, existing.bio);
        assert_eq!(updated.created_at, existing.created_at);
        assert_eq!(updated.updated_at, now + 10);
    }

    #[tokio::test]
    async fn signup_creates_account() {
        let now = unix_timestamp();
        let url = "http://localhost/v1/signup";
        let event = signed_event(url, "POST", now, None);
        let username = username_from_pubkey(&event.pubkey).expect("username");
        let responses = vec![
            ForgejoResponse {
                status: 404,
                body: "".to_string(),
            },
            ForgejoResponse {
                status: 201,
                body: user_json(&username),
            },
        ];
        let (state, repos, transport) = test_state(responses);
        let app = build_router(state);
        let token = BASE64_STANDARD.encode(serde_json::to_vec(&event).expect("event json"));
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/signup")
                    .header("host", "localhost")
                    .header(AUTH_HEADER, format!("Nostr {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::OK);

        let requests = transport.requests();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].method, gittree_forgejo::ForgejoMethod::Get);
        assert!(
            requests[0]
                .url
                .ends_with(&format!("/api/v1/users/{username}"))
        );
        assert_eq!(requests[1].method, gittree_forgejo::ForgejoMethod::Post);
        assert!(requests[1].url.ends_with("/api/v1/admin/users"));

        let pubkey_bytes = hex::decode(&event.pubkey).expect("pubkey bytes");
        let stored = repos
            .account_by_pubkey(&pubkey_bytes)
            .await
            .expect("lookup");
        assert!(stored.is_some());
        assert_eq!(stored.unwrap().forgejo_username, username);
    }

    #[tokio::test]
    async fn signup_returns_existing_account_without_forgejo_call() {
        let now = unix_timestamp();
        let url = "http://localhost/v1/signup";
        let event = signed_event(url, "POST", now, None);
        let username = username_from_pubkey(&event.pubkey).expect("username");
        let (state, repos, transport) = test_state(Vec::new());
        let account = AccountRecord::new(&event.pubkey, &username).expect("account");
        repos.upsert_account(account).await.expect("upsert");

        let app = build_router(state);
        let token = BASE64_STANDARD.encode(serde_json::to_vec(&event).expect("event json"));
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/signup")
                    .header("host", "localhost")
                    .header(AUTH_HEADER, format!("Nostr {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let body: serde_json::Value = serde_json::from_slice(&body).expect("json");
        assert_eq!(
            body.get("status").and_then(|value| value.as_str()),
            Some("existing")
        );
        assert_eq!(
            body.get("username").and_then(|value| value.as_str()),
            Some(username.as_str())
        );
        assert!(transport.requests().is_empty());
    }

    #[tokio::test]
    async fn profile_get_creates_default_profile() {
        let now = unix_timestamp();
        let url = "http://localhost/v1/profile";
        let event = signed_event(url, "GET", now, None);
        let (state, repos, _transport) = test_state(Vec::new());
        let account = AccountRecord::new(&event.pubkey, "alice").expect("account");
        repos.upsert_account(account).await.expect("upsert");

        let app = build_router(state);
        let token = BASE64_STANDARD.encode(serde_json::to_vec(&event).expect("event json"));
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/v1/profile")
                    .header("host", "localhost")
                    .header(AUTH_HEADER, format!("Nostr {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::OK);

        let pubkey_bytes = hex::decode(&event.pubkey).expect("pubkey");
        let stored = repos
            .profile_by_pubkey(&pubkey_bytes)
            .await
            .expect("profile")
            .expect("stored");
        assert_eq!(stored.display_name.as_deref(), Some("alice"));
        assert_eq!(stored.visibility, StorageProfileVisibility::Private);
    }

    #[tokio::test]
    async fn profile_get_rejects_missing_account() {
        let now = unix_timestamp();
        let url = "http://localhost/v1/profile";
        let event = signed_event(url, "GET", now, None);
        let (state, _repos, _transport) = test_state(Vec::new());
        let app = build_router(state);
        let token = BASE64_STANDARD.encode(serde_json::to_vec(&event).expect("event json"));
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/v1/profile")
                    .header("host", "localhost")
                    .header(AUTH_HEADER, format!("Nostr {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn profile_get_rejects_missing_host_header() {
        let now = unix_timestamp();
        let url = "http://localhost/v1/profile";
        let event = signed_event(url, "GET", now, None);
        let (state, _repos, _transport) = test_state(Vec::new());
        let app = build_router(state);
        let token = BASE64_STANDARD.encode(serde_json::to_vec(&event).expect("event json"));
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/v1/profile")
                    .header(AUTH_HEADER, format!("Nostr {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn profile_get_rejects_invalid_account_username_for_profile_bootstrap() {
        let now = unix_timestamp();
        let url = "http://localhost/v1/profile";
        let event = signed_event(url, "GET", now, None);
        let accounts: Arc<dyn AccountRepository> = Arc::new(ScriptedAccountRepository {
            account: Some(AccountRecord {
                pubkey: hex::decode(&event.pubkey).expect("pubkey"),
                forgejo_username: "a".repeat(256),
            }),
            ..ScriptedAccountRepository::default()
        });
        let profiles: Arc<dyn ProfileRepository> = Arc::new(ScriptedProfileRepository::default());
        let (state, _transport) = scripted_state(Vec::new(), accounts, profiles);
        let app = build_router(state);
        let token = BASE64_STANDARD.encode(serde_json::to_vec(&event).expect("event json"));
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/v1/profile")
                    .header("host", "localhost")
                    .header(AUTH_HEADER, format!("Nostr {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn profile_get_rejects_invalid_nip98_signature_context() {
        let now = unix_timestamp();
        let url = "http://localhost/v1/profile";
        let event = signed_event(url, "PATCH", now, None);
        let (state, _repos, _transport) = test_state(Vec::new());
        let app = build_router(state);
        let token = BASE64_STANDARD.encode(serde_json::to_vec(&event).expect("event json"));
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/v1/profile")
                    .header("host", "localhost")
                    .header(AUTH_HEADER, format!("Nostr {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn profile_get_returns_internal_when_account_lookup_fails() {
        let now = unix_timestamp();
        let url = "http://localhost/v1/profile";
        let event = signed_event(url, "GET", now, None);
        let accounts: Arc<dyn AccountRepository> = Arc::new(ScriptedAccountRepository {
            lookup_error: Some("account lookup failed".to_string()),
            ..ScriptedAccountRepository::default()
        });
        let profiles: Arc<dyn ProfileRepository> = Arc::new(ScriptedProfileRepository::default());
        let (state, _transport) = scripted_state(Vec::new(), accounts, profiles);
        let app = build_router(state);
        let token = BASE64_STANDARD.encode(serde_json::to_vec(&event).expect("event json"));
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/v1/profile")
                    .header("host", "localhost")
                    .header(AUTH_HEADER, format!("Nostr {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[tokio::test]
    async fn profile_patch_updates_profile() {
        let now = unix_timestamp();
        let url = "http://localhost/v1/profile";
        let event = signed_event(url, "PATCH", now, None);
        let (state, repos, _transport) = test_state(Vec::new());
        let account = AccountRecord::new(&event.pubkey, "alice").expect("account");
        repos.upsert_account(account).await.expect("upsert");
        let existing = ProfileRecord::new(
            &event.pubkey,
            Some("Alice".to_string()),
            None,
            None,
            None,
            None,
            StorageProfileVisibility::Private,
            now,
            now,
        )
        .expect("profile");
        repos.upsert_profile(existing).await.expect("upsert");

        let update = ProfileUpdate {
            display_name: Some("Ada".to_string()),
            bio: Some("Builder".to_string()),
            visibility: Some(ApiProfileVisibility::Public),
            ..ProfileUpdate::default()
        };
        let body = serde_json::to_vec(&update).expect("update json");
        let body_bytes = Bytes::from(body);
        let payload_hash = payload_hash(&body_bytes).expect("hash");
        let event = signed_event(url, "PATCH", now, Some(&payload_hash));

        let app = build_router(state);
        let token = BASE64_STANDARD.encode(serde_json::to_vec(&event).expect("event json"));
        let response = app
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri("/v1/profile")
                    .header("host", "localhost")
                    .header("content-type", "application/json")
                    .header(AUTH_HEADER, format!("Nostr {token}"))
                    .body(Body::from(body_bytes))
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::OK);

        let pubkey_bytes = hex::decode(&event.pubkey).expect("pubkey");
        let stored = repos
            .profile_by_pubkey(&pubkey_bytes)
            .await
            .expect("profile")
            .expect("stored");
        assert_eq!(stored.display_name.as_deref(), Some("Ada"));
        assert_eq!(stored.bio.as_deref(), Some("Builder"));
        assert_eq!(stored.visibility, StorageProfileVisibility::Public);
    }

    #[tokio::test]
    async fn profile_patch_rejects_missing_body() {
        let now = unix_timestamp();
        let url = "http://localhost/v1/profile";
        let event = signed_event(url, "PATCH", now, None);
        let (state, repos, _transport) = test_state(Vec::new());
        let account = AccountRecord::new(&event.pubkey, "alice").expect("account");
        repos.upsert_account(account).await.expect("upsert");

        let app = build_router(state);
        let token = BASE64_STANDARD.encode(serde_json::to_vec(&event).expect("event json"));
        let response = app
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri("/v1/profile")
                    .header("host", "localhost")
                    .header(AUTH_HEADER, format!("Nostr {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn profile_patch_rejects_missing_host_header() {
        let now = unix_timestamp();
        let url = "http://localhost/v1/profile";
        let update = ProfileUpdate {
            display_name: Some("Ada".to_string()),
            ..ProfileUpdate::default()
        };
        let body = Bytes::from(serde_json::to_vec(&update).expect("update json"));
        let hash = payload_hash(&body).expect("hash");
        let event = signed_event(url, "PATCH", now, Some(&hash));
        let (state, _repos, _transport) = test_state(Vec::new());
        let app = build_router(state);
        let token = BASE64_STANDARD.encode(serde_json::to_vec(&event).expect("event json"));
        let response = app
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri("/v1/profile")
                    .header("content-type", "application/json")
                    .header(AUTH_HEADER, format!("Nostr {token}"))
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn profile_patch_rejects_invalid_account_username_for_profile_bootstrap() {
        let now = unix_timestamp();
        let url = "http://localhost/v1/profile";
        let update = ProfileUpdate {
            display_name: Some("Ada".to_string()),
            ..ProfileUpdate::default()
        };
        let body = Bytes::from(serde_json::to_vec(&update).expect("update json"));
        let hash = payload_hash(&body).expect("hash");
        let event = signed_event(url, "PATCH", now, Some(&hash));
        let accounts: Arc<dyn AccountRepository> = Arc::new(ScriptedAccountRepository {
            account: Some(AccountRecord {
                pubkey: hex::decode(&event.pubkey).expect("pubkey"),
                forgejo_username: "a".repeat(256),
            }),
            ..ScriptedAccountRepository::default()
        });
        let profiles: Arc<dyn ProfileRepository> = Arc::new(ScriptedProfileRepository::default());
        let (state, _transport) = scripted_state(Vec::new(), accounts, profiles);
        let app = build_router(state);
        let token = BASE64_STANDARD.encode(serde_json::to_vec(&event).expect("event json"));
        let response = app
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri("/v1/profile")
                    .header("host", "localhost")
                    .header("content-type", "application/json")
                    .header(AUTH_HEADER, format!("Nostr {token}"))
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn profile_patch_rejects_invalid_profile_update_fields() {
        let now = unix_timestamp();
        let url = "http://localhost/v1/profile";
        let event = signed_event(url, "PATCH", now, None);
        let (state, repos, _transport) = test_state(Vec::new());
        let account = AccountRecord::new(&event.pubkey, "alice").expect("account");
        repos.upsert_account(account).await.expect("upsert");
        let existing = ProfileRecord::new(
            &event.pubkey,
            Some("Alice".to_string()),
            None,
            None,
            None,
            None,
            StorageProfileVisibility::Private,
            now,
            now,
        )
        .expect("profile");
        repos.upsert_profile(existing).await.expect("upsert");

        let update = ProfileUpdate {
            display_name: Some("a".repeat(256)),
            ..ProfileUpdate::default()
        };
        let body = Bytes::from(serde_json::to_vec(&update).expect("update json"));
        let hash = payload_hash(&body).expect("hash");
        let event = signed_event(url, "PATCH", now, Some(&hash));

        let app = build_router(state);
        let token = BASE64_STANDARD.encode(serde_json::to_vec(&event).expect("event json"));
        let response = app
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri("/v1/profile")
                    .header("host", "localhost")
                    .header("content-type", "application/json")
                    .header(AUTH_HEADER, format!("Nostr {token}"))
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn profile_patch_rejects_invalid_json_payload() {
        let now = unix_timestamp();
        let url = "http://localhost/v1/profile";
        let (state, repos, _transport) = test_state(Vec::new());
        let account = AccountRecord::new(
            "4f355bdcb7cc0af728ef3cceb9615d90684bb5b2ca5f859ab0f0b704075871aa",
            "alice",
        )
        .expect("account");
        repos.upsert_account(account).await.expect("upsert");

        let body_bytes = Bytes::from_static(b"{invalid");
        let hash = payload_hash(&body_bytes).expect("hash");
        let event = signed_event(url, "PATCH", now, Some(&hash));
        let app = build_router(state);
        let token = BASE64_STANDARD.encode(serde_json::to_vec(&event).expect("event json"));
        let response = app
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri("/v1/profile")
                    .header("host", "localhost")
                    .header("content-type", "application/json")
                    .header(AUTH_HEADER, format!("Nostr {token}"))
                    .body(Body::from(body_bytes))
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn profile_patch_rejects_missing_account() {
        let now = unix_timestamp();
        let url = "http://localhost/v1/profile";
        let update = ProfileUpdate {
            display_name: Some("Ada".to_string()),
            ..ProfileUpdate::default()
        };
        let body = Bytes::from(serde_json::to_vec(&update).expect("update json"));
        let hash = payload_hash(&body).expect("hash");
        let event = signed_event(url, "PATCH", now, Some(&hash));
        let (state, _repos, _transport) = test_state(Vec::new());
        let app = build_router(state);
        let token = BASE64_STANDARD.encode(serde_json::to_vec(&event).expect("event json"));
        let response = app
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri("/v1/profile")
                    .header("host", "localhost")
                    .header("content-type", "application/json")
                    .header(AUTH_HEADER, format!("Nostr {token}"))
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn profile_patch_rejects_invalid_nip98_signature_context() {
        let now = unix_timestamp();
        let url = "http://localhost/v1/profile";
        let update = ProfileUpdate {
            display_name: Some("Ada".to_string()),
            ..ProfileUpdate::default()
        };
        let body = Bytes::from(serde_json::to_vec(&update).expect("update json"));
        let hash = payload_hash(&body).expect("hash");
        let event = signed_event(url, "GET", now, Some(&hash));
        let (state, _repos, _transport) = test_state(Vec::new());
        let app = build_router(state);
        let token = BASE64_STANDARD.encode(serde_json::to_vec(&event).expect("event json"));
        let response = app
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri("/v1/profile")
                    .header("host", "localhost")
                    .header("content-type", "application/json")
                    .header(AUTH_HEADER, format!("Nostr {token}"))
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn profile_patch_returns_internal_when_account_lookup_fails() {
        let now = unix_timestamp();
        let url = "http://localhost/v1/profile";
        let update = ProfileUpdate {
            display_name: Some("Ada".to_string()),
            ..ProfileUpdate::default()
        };
        let body = Bytes::from(serde_json::to_vec(&update).expect("update json"));
        let hash = payload_hash(&body).expect("hash");
        let event = signed_event(url, "PATCH", now, Some(&hash));
        let accounts: Arc<dyn AccountRepository> = Arc::new(ScriptedAccountRepository {
            lookup_error: Some("account lookup failed".to_string()),
            ..ScriptedAccountRepository::default()
        });
        let profiles: Arc<dyn ProfileRepository> = Arc::new(ScriptedProfileRepository::default());
        let (state, _transport) = scripted_state(Vec::new(), accounts, profiles);
        let app = build_router(state);
        let token = BASE64_STANDARD.encode(serde_json::to_vec(&event).expect("event json"));
        let response = app
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri("/v1/profile")
                    .header("host", "localhost")
                    .header("content-type", "application/json")
                    .header(AUTH_HEADER, format!("Nostr {token}"))
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[tokio::test]
    async fn profile_patch_returns_internal_when_profile_upsert_fails() {
        let now = unix_timestamp();
        let url = "http://localhost/v1/profile";
        let update = ProfileUpdate {
            display_name: Some("Ada".to_string()),
            ..ProfileUpdate::default()
        };
        let body = Bytes::from(serde_json::to_vec(&update).expect("update json"));
        let hash = payload_hash(&body).expect("hash");
        let event = signed_event(url, "PATCH", now, Some(&hash));
        let account = AccountRecord::new(&event.pubkey, "alice").expect("account");
        let profile = ProfileRecord::new(
            &event.pubkey,
            Some("Alice".to_string()),
            None,
            None,
            None,
            None,
            StorageProfileVisibility::Private,
            now,
            now,
        )
        .expect("profile");
        let accounts: Arc<dyn AccountRepository> = Arc::new(ScriptedAccountRepository {
            account: Some(account),
            ..ScriptedAccountRepository::default()
        });
        let profiles: Arc<dyn ProfileRepository> = Arc::new(ScriptedProfileRepository {
            profile: Some(profile),
            upsert_error: Some("profile upsert failed".to_string()),
            ..ScriptedProfileRepository::default()
        });
        let (state, _transport) = scripted_state(Vec::new(), accounts, profiles);
        let app = build_router(state);
        let token = BASE64_STANDARD.encode(serde_json::to_vec(&event).expect("event json"));
        let response = app
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri("/v1/profile")
                    .header("host", "localhost")
                    .header("content-type", "application/json")
                    .header(AUTH_HEADER, format!("Nostr {token}"))
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[tokio::test]
    async fn profile_public_returns_profile_for_public() {
        let now = unix_timestamp();
        let event = signed_event("http://localhost/v1/profile", "GET", now, None);
        let (state, repos, _transport) = test_state(Vec::new());
        let account = AccountRecord::new(&event.pubkey, "alice").expect("account");
        repos.upsert_account(account).await.expect("upsert");
        let profile = ProfileRecord::new(
            &event.pubkey,
            Some("Alice".to_string()),
            None,
            None,
            None,
            None,
            StorageProfileVisibility::Public,
            now,
            now,
        )
        .expect("profile");
        repos.upsert_profile(profile).await.expect("upsert");

        let pubkey_bytes = hex::decode(&event.pubkey).expect("pubkey");
        let npub = npub_from_bytes(&pubkey_bytes).expect("npub");
        let app = build_router(state);
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/v1/profile/{npub}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::OK);

        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let profile: Profile = serde_json::from_slice(&body).expect("profile");
        assert_eq!(profile.username, "alice");
        assert_eq!(profile.visibility, ApiProfileVisibility::Public);
    }

    #[tokio::test]
    async fn profile_public_hides_private_profiles() {
        let now = unix_timestamp();
        let event = signed_event("http://localhost/v1/profile", "GET", now, None);
        let (state, repos, _transport) = test_state(Vec::new());
        let account = AccountRecord::new(&event.pubkey, "alice").expect("account");
        repos.upsert_account(account).await.expect("upsert");
        let profile = ProfileRecord::new(
            &event.pubkey,
            Some("Alice".to_string()),
            None,
            None,
            None,
            None,
            StorageProfileVisibility::Private,
            now,
            now,
        )
        .expect("profile");
        repos.upsert_profile(profile).await.expect("upsert");

        let pubkey_bytes = hex::decode(&event.pubkey).expect("pubkey");
        let npub = npub_from_bytes(&pubkey_bytes).expect("npub");
        let app = build_router(state);
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/v1/profile/{npub}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn profile_public_rejects_invalid_npub() {
        let (state, _repos, _transport) = test_state(Vec::new());
        let app = build_router(state);
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/v1/profile/not-an-npub")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn profile_public_returns_not_found_without_account() {
        let now = unix_timestamp();
        let event = signed_event("http://localhost/v1/profile", "GET", now, None);
        let pubkey_bytes = hex::decode(&event.pubkey).expect("pubkey");
        let npub = npub_from_bytes(&pubkey_bytes).expect("npub");
        let (state, _repos, _transport) = test_state(Vec::new());
        let app = build_router(state);
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/v1/profile/{npub}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn profile_public_returns_not_found_when_profile_is_missing() {
        let now = unix_timestamp();
        let event = signed_event("http://localhost/v1/profile", "GET", now, None);
        let account = AccountRecord::new(&event.pubkey, "alice").expect("account");
        let pubkey_bytes = hex::decode(&event.pubkey).expect("pubkey");
        let npub = npub_from_bytes(&pubkey_bytes).expect("npub");
        let accounts: Arc<dyn AccountRepository> = Arc::new(ScriptedAccountRepository {
            account: Some(account),
            ..ScriptedAccountRepository::default()
        });
        let profiles: Arc<dyn ProfileRepository> = Arc::new(ScriptedProfileRepository::default());
        let (state, _transport) = scripted_state(Vec::new(), accounts, profiles);
        let app = build_router(state);
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/v1/profile/{npub}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn profile_public_returns_internal_when_account_lookup_fails() {
        let now = unix_timestamp();
        let event = signed_event("http://localhost/v1/profile", "GET", now, None);
        let pubkey_bytes = hex::decode(&event.pubkey).expect("pubkey");
        let npub = npub_from_bytes(&pubkey_bytes).expect("npub");
        let accounts: Arc<dyn AccountRepository> = Arc::new(ScriptedAccountRepository {
            lookup_error: Some("account lookup failed".to_string()),
            ..ScriptedAccountRepository::default()
        });
        let profiles: Arc<dyn ProfileRepository> = Arc::new(ScriptedProfileRepository::default());
        let (state, _transport) = scripted_state(Vec::new(), accounts, profiles);
        let app = build_router(state);
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/v1/profile/{npub}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[tokio::test]
    async fn profile_public_returns_internal_when_profile_lookup_fails() {
        let now = unix_timestamp();
        let event = signed_event("http://localhost/v1/profile", "GET", now, None);
        let account = AccountRecord::new(&event.pubkey, "alice").expect("account");
        let pubkey_bytes = hex::decode(&event.pubkey).expect("pubkey");
        let npub = npub_from_bytes(&pubkey_bytes).expect("npub");
        let accounts: Arc<dyn AccountRepository> = Arc::new(ScriptedAccountRepository {
            account: Some(account),
            ..ScriptedAccountRepository::default()
        });
        let profiles: Arc<dyn ProfileRepository> = Arc::new(ScriptedProfileRepository {
            lookup_error: Some("profile lookup failed".to_string()),
            ..ScriptedProfileRepository::default()
        });
        let (state, _transport) = scripted_state(Vec::new(), accounts, profiles);
        let app = build_router(state);
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/v1/profile/{npub}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }
}
