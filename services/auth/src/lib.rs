#![forbid(unsafe_code)]

use axum::body::Bytes;
use axum::extract::{OriginalUri, Path, State};
use axum::http::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE};
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine;
use gittree_app_core::{
    pubkey_bytes_from_npub, Profile, ProfileUpdate,
    ProfileVisibility as ApiProfileVisibility,
};
use gittree_config::{AuthConfig as AuthSettings, ConfigError, ForgejoConfig, ServicesConfig};
use gittree_forgejo::{
    ForgejoClient, ForgejoCreateUser, ForgejoError, ForgejoTransport,
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
        let services = ServicesConfig::from_env_validated().map_err(AuthConfigError::Config)?;
        let auth = AuthSettings::from_env().map_err(AuthConfigError::Config)?;
        let forgejo = ForgejoConfig::from_env().map_err(AuthConfigError::Config)?;
        let storage = storage_from_env()?;
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

fn storage_from_env() -> Result<StorageConfig, AuthConfigError> {
    let read_connection = std::env::var(ENV_STORAGE_READ_URL)
        .map_err(|_| AuthConfigError::Storage(StorageConfigError::MissingEnv(ENV_STORAGE_READ_URL)))?;
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
        AuthConfigError::Storage(StorageConfigError::InvalidConfig(err.to_string()))
    })?;

    Ok(config)
}

fn env_u32(key: &'static str) -> Result<Option<u32>, AuthConfigError> {
    match std::env::var(key) {
        Ok(value) => {
            if value.trim().is_empty() {
                return Ok(None);
            }
            value.parse::<u32>().map(Some).map_err(|_| {
                AuthConfigError::Storage(StorageConfigError::InvalidEnv { key, value })
            })
        }
        Err(_) => Ok(None),
    }
}

fn env_u64(key: &'static str) -> Result<Option<u64>, AuthConfigError> {
    match std::env::var(key) {
        Ok(value) => {
            if value.trim().is_empty() {
                return Ok(None);
            }
            value.parse::<u64>().map(Some).map_err(|_| {
                AuthConfigError::Storage(StorageConfigError::InvalidEnv { key, value })
            })
        }
        Err(_) => Ok(None),
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
    let config = gittree_observability::ObservabilityConfig::from_env("gittree-auth")
        .map_err(AuthError::ObservabilityConfig)?;
    let handle = gittree_observability::init(&config).map_err(AuthError::Observability)?;
    Ok(handle)
}

#[derive(Clone)]
struct AuthAppState<T> {
    auth: AuthSettings,
    forgejo: ForgejoClient<T>,
    accounts: Arc<dyn AccountRepository>,
    profiles: Arc<dyn ProfileRepository>,
}

pub async fn serve(config: AuthServiceConfig) -> Result<(), AuthError> {
    let _observability = init_observability()?;
    let repositories = Arc::new(build_repositories(&config)?);
    let bind = config.bind.clone();
    let auth = config.auth;
    let forgejo = ForgejoClient::new(config.forgejo).map_err(AuthError::Forgejo)?;
    let state = AuthAppState {
        auth,
        forgejo,
        accounts: repositories.clone(),
        profiles: repositories,
    };
    let router = build_router(state);
    let listener = tokio::net::TcpListener::bind(&bind)
        .await
        .map_err(|err| AuthError::Serve(err.to_string()))?;
    axum::serve(listener, router)
        .await
        .map_err(|err| AuthError::Serve(err.to_string()))?;
    Ok(())
}

fn build_router<T>(state: AuthAppState<T>) -> Router
where
    T: ForgejoTransport + Clone + Send + Sync + 'static,
{
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

async fn health_handler<T>(State(state): State<AuthAppState<T>>) -> &'static str
where
    T: ForgejoTransport + Clone + Send + Sync + 'static,
{
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

async fn signup_handler<T>(
    State(state): State<AuthAppState<T>>,
    headers: HeaderMap,
    method: Method,
    OriginalUri(uri): OriginalUri,
    body: Bytes,
) -> Result<Json<SignupResponse>, AuthHttpError>
where
    T: ForgejoTransport + Clone + Send + Sync + 'static,
{
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
    let auth = validate_nip98(&event, &request)
        .map_err(|err| AuthHttpError::Unauthorized(err.to_string()))?;

    let username = username_from_pubkey(&auth.pubkey)?;
    let pubkey_bytes = hex::decode(&auth.pubkey)
        .map_err(|_| AuthHttpError::BadRequest("invalid pubkey".to_string()))?;

    if let Some(existing) = state
        .accounts
        .account_by_pubkey(&pubkey_bytes)
        .await
        .map_err(|err| AuthHttpError::Internal(err.to_string()))?
    {
        return Ok(Json(SignupResponse {
            pubkey: auth.pubkey,
            username: existing.forgejo_username,
            status: "existing".to_string(),
        }));
    }

    let email = format!("{}@{}", username, state.auth.email_domain);
    let password = generate_password();
    let forgejo_user = state
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
        .map_err(|err| AuthHttpError::Internal(err.to_string()))?;

    let record = AccountRecord::new(&auth.pubkey, &forgejo_user.username)
        .map_err(|err| AuthHttpError::BadRequest(err.to_string()))?;
    state
        .accounts
        .upsert_account(record)
        .await
        .map_err(|err| AuthHttpError::Internal(err.to_string()))?;

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
    state
        .profiles
        .upsert_profile(profile)
        .await
        .map_err(|err| AuthHttpError::Internal(err.to_string()))?;

    Ok(Json(SignupResponse {
        pubkey: auth.pubkey,
        username: forgejo_user.username,
        status: "created".to_string(),
    }))
}

async fn profile_get_handler<T>(
    State(state): State<AuthAppState<T>>,
    headers: HeaderMap,
    method: Method,
    OriginalUri(uri): OriginalUri,
) -> Result<Json<Profile>, AuthHttpError>
where
    T: ForgejoTransport + Clone + Send + Sync + 'static,
{
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
    let auth = validate_nip98(&event, &request)
        .map_err(|err| AuthHttpError::Unauthorized(err.to_string()))?;

    let pubkey_bytes = parse_pubkey_bytes(&auth.pubkey)?;
    let account = state
        .accounts
        .account_by_pubkey(&pubkey_bytes)
        .await
        .map_err(|err| AuthHttpError::Internal(err.to_string()))?
        .ok_or_else(|| AuthHttpError::BadRequest("account not found".to_string()))?;
    let profile = ensure_profile(&state.profiles, &auth.pubkey, &account.forgejo_username, now)
        .await?;
    Ok(Json(profile_response(
        &auth.pubkey,
        &account.forgejo_username,
        profile,
    )))
}

async fn profile_patch_handler<T>(
    State(state): State<AuthAppState<T>>,
    headers: HeaderMap,
    method: Method,
    OriginalUri(uri): OriginalUri,
    body: Bytes,
) -> Result<Json<Profile>, AuthHttpError>
where
    T: ForgejoTransport + Clone + Send + Sync + 'static,
{
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
    let auth = validate_nip98(&event, &request)
        .map_err(|err| AuthHttpError::Unauthorized(err.to_string()))?;

    if body.is_empty() {
        return Err(AuthHttpError::BadRequest(
            "missing profile update".to_string(),
        ));
    }
    let update: ProfileUpdate = serde_json::from_slice(&body)
        .map_err(|err| AuthHttpError::BadRequest(format!("invalid profile update: {err}")))?;

    let pubkey_bytes = parse_pubkey_bytes(&auth.pubkey)?;
    let account = state
        .accounts
        .account_by_pubkey(&pubkey_bytes)
        .await
        .map_err(|err| AuthHttpError::Internal(err.to_string()))?
        .ok_or_else(|| AuthHttpError::BadRequest("account not found".to_string()))?;
    let profile = ensure_profile(&state.profiles, &auth.pubkey, &account.forgejo_username, now)
        .await?;
    let updated = apply_profile_update(&auth.pubkey, profile, update, now)
        .map_err(profile_input_error)?;
    state
        .profiles
        .upsert_profile(updated.clone())
        .await
        .map_err(|err| AuthHttpError::Internal(err.to_string()))?;
    Ok(Json(profile_response(
        &auth.pubkey,
        &account.forgejo_username,
        updated,
    )))
}

async fn profile_public_handler<T>(
    State(state): State<AuthAppState<T>>,
    Path(npub): Path<String>,
) -> Result<Json<Profile>, AuthHttpError>
where
    T: ForgejoTransport + Clone + Send + Sync + 'static,
{
    let pubkey_bytes = pubkey_bytes_from_npub(&npub)
        .map_err(|_| AuthHttpError::BadRequest("invalid npub".to_string()))?;
    let account = state
        .accounts
        .account_by_pubkey(&pubkey_bytes)
        .await
        .map_err(|err| AuthHttpError::Internal(err.to_string()))?
        .ok_or_else(|| AuthHttpError::NotFound("profile not found".to_string()))?;
    let profile = state
        .profiles
        .profile_by_pubkey(&pubkey_bytes)
        .await
        .map_err(|err| AuthHttpError::Internal(err.to_string()))?
        .ok_or_else(|| AuthHttpError::NotFound("profile not found".to_string()))?;

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
    let host = headers
        .get("host")
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| AuthHttpError::BadRequest("missing host header".to_string()))?;
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
    serde_json::from_slice::<Nip98Event>(&decoded)
        .map_err(|_| AuthHttpError::Unauthorized("invalid nostr event".to_string()))
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
    if pubkey.len() != 64 || !pubkey.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(AuthHttpError::BadRequest("invalid pubkey".to_string()));
    }
    hex::decode(pubkey)
        .map_err(|_| AuthHttpError::BadRequest("invalid pubkey".to_string()))
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
    if let Some(existing) = profiles
        .profile_by_pubkey(&pubkey_bytes)
        .await
        .map_err(|err| AuthHttpError::Internal(err.to_string()))?
    {
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
    profiles
        .upsert_profile(record.clone())
        .await
        .map_err(|err| AuthHttpError::Internal(err.to_string()))?;
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
    use axum::http::Request;
    use gittree_app_core::npub_from_bytes;
    use gittree_forgejo::{ForgejoRequest, ForgejoResponse};
    use gittree_nostr_auth::NIP98_KIND;
    use secp256k1::{Keypair, Message, Secp256k1, SecretKey, XOnlyPublicKey};
    use serde_json::json;
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};
    use tower::ServiceExt;

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
    ) -> (AuthAppState<MockTransport>, Arc<gittree_storage::InMemoryRepositories>, MockTransport) {
        let transport = MockTransport::new(responses);
        let forgejo = ForgejoClient::with_transport(test_config(), transport.clone());
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

    fn sign_event_id(event_id: &str, keypair: &Keypair, secp: &Secp256k1<secp256k1::All>) -> String {
        let bytes = hex::decode(event_id).expect("decode");
        let msg = Message::from_digest_slice(&bytes).expect("msg");
        let sig = secp.sign_schnorr(&msg, keypair);
        hex::encode(sig.as_ref())
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
    async fn profile_get_creates_default_profile() {
        let now = unix_timestamp();
        let url = "http://localhost/v1/profile";
        let event = signed_event(url, "GET", now, None);
        let (state, repos, _transport) = test_state(Vec::new());
        let account =
            AccountRecord::new(&event.pubkey, "alice").expect("account");
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
    async fn profile_patch_updates_profile() {
        let now = unix_timestamp();
        let url = "http://localhost/v1/profile";
        let event = signed_event(url, "PATCH", now, None);
        let (state, repos, _transport) = test_state(Vec::new());
        let account =
            AccountRecord::new(&event.pubkey, "alice").expect("account");
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
}
