use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use gittree_config::{ConfigError, ControlAuthConfig, ForgejoConfig, ServicesConfig};
use gittree_core::kinds::KIND_GITTREE_CONTROL;
use gittree_core::ControlAction;
use gittree_forgejo::{
    ForgejoClient, ForgejoCreateOrg, ForgejoCreatePullRequest, ForgejoCreateRepo,
    ForgejoCreateUser, ForgejoError, ForgejoOrg, ForgejoPullRequest, ForgejoRepo, ForgejoTransport,
    ForgejoUser,
};
use gittree_observability::{ObservabilityConfigError, ObservabilityError, ObservabilityHandle};
use serde::{Deserialize, Serialize};

#[allow(dead_code)]
const AUTH_HEADER: &str = "authorization";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControlConfig {
    pub bind: String,
    pub auth: ControlAuthConfig,
    pub forgejo: ForgejoConfig,
}

impl ControlConfig {
    pub fn from_env() -> Result<Self, ControlConfigError> {
        let services = ServicesConfig::from_env_validated().map_err(ControlConfigError::Config)?;
        let auth = ControlAuthConfig::from_env().map_err(ControlConfigError::Config)?;
        let forgejo = ForgejoConfig::from_env().map_err(ControlConfigError::Config)?;
        Ok(Self {
            bind: services.control.bind,
            auth,
            forgejo,
        })
    }
}

#[derive(Debug)]
pub enum ControlConfigError {
    Config(ConfigError),
}

impl std::fmt::Display for ControlConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ControlConfigError::Config(err) => write!(f, "control config error: {err}"),
        }
    }
}

impl std::error::Error for ControlConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ControlConfigError::Config(err) => Some(err),
        }
    }
}

#[derive(Debug)]
pub enum ControlError {
    Config(ControlConfigError),
    Forgejo(ForgejoError),
    ObservabilityConfig(ObservabilityConfigError),
    Observability(ObservabilityError),
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

#[derive(Clone)]
struct ControlAppState<T> {
    auth: ControlAuthConfig,
    forgejo: ForgejoClient<T>,
    forgejo_owner: String,
}

pub async fn serve(config: ControlConfig) -> Result<(), ControlError> {
    let _observability = init_observability()?;
    let forgejo_owner = config.forgejo.owner.clone();
    let forgejo = ForgejoClient::new(config.forgejo).map_err(ControlError::Forgejo)?;
    let state = ControlAppState {
        auth: config.auth,
        forgejo,
        forgejo_owner,
    };
    let router = build_router(state);
    let listener = tokio::net::TcpListener::bind(&config.bind)
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
    description: Option<String>,
    private: Option<bool>,
    auto_init: Option<bool>,
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
    let repo = state
        .forgejo
        .create_repo_for_owner(
            &payload.owner,
            ForgejoCreateRepo {
                name: payload.name,
                description: payload.description,
                private: payload.private,
                auto_init: payload.auto_init,
            },
        )
        .await
        .map_err(map_forgejo_error)?;
    Ok(Json(repo.into()))
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
            description,
            private,
        } => {
            let owner = owner.unwrap_or_else(|| state.forgejo_owner.clone());
            let repo = state
                .forgejo
                .create_repo_for_owner(
                    &owner,
                    ForgejoCreateRepo {
                        name,
                        description,
                        private,
                        auto_init: None,
                    },
                )
                .await
                .map_err(map_forgejo_error)?;
            Ok(ControlEventResponse::CreateRepo {
                repo: repo.into(),
            })
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

fn require_non_empty(field: &'static str, value: &str) -> Result<(), ControlHttpError> {
    if value.trim().is_empty() {
        return Err(ControlHttpError::BadRequest(format!(
            "missing {field}"
        )));
    }
    Ok(())
}

fn map_forgejo_error(error: ForgejoError) -> ControlHttpError {
    match error {
        ForgejoError::Response { status, body } if status >= 400 && status < 500 => {
            ControlHttpError::BadRequest(format!("forgejo error {status}: {body}"))
        }
        err => ControlHttpError::Internal(err.to_string()),
    }
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
    use serde_json::json;
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};
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
    ) -> (super::ControlAppState<MockTransport>, MockTransport) {
        test_state_with_auth(responses, Vec::new(), "gittree")
    }

    fn test_state_with_auth(
        responses: Vec<ForgejoResponse>,
        admin_keys: Vec<String>,
        owner: &str,
    ) -> (super::ControlAppState<MockTransport>, MockTransport) {
        let transport = MockTransport::new(responses);
        let client = ForgejoClient::with_transport(test_config(), transport.clone());
        (
            super::ControlAppState {
                auth: ControlAuthConfig {
                    token: "token".to_string(),
                    admin_keys,
                },
                forgejo: client,
                forgejo_owner: owner.to_string(),
            },
            transport,
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
                                let config = ControlConfig::from_env().expect("config");
                                assert!(!config.bind.is_empty());
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
        let (state, _transport) = test_state(Vec::new());
        let app = build_router(state);
        let response = app
            .oneshot(Request::builder().uri("/health").body(Body::empty()).unwrap())
            .await
            .expect("response");
        assert_eq!(response.status(), axum::http::StatusCode::OK);
    }

    #[tokio::test]
    async fn create_user_rejects_missing_auth() {
        let (state, _transport) = test_state(Vec::new());
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
        let (state, transport) = test_state(responses);
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
        let (state, transport) = test_state(responses);
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
        let (state, transport) = test_state(responses);
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
                            "auto_init":true
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
    }

    #[tokio::test]
    async fn create_pull_posts_to_repo_endpoint() {
        let responses = vec![ForgejoResponse {
            status: 201,
            body: r#"{"number":5,"url":"http://localhost/api/v1/repos/gittree/demo/pulls/5"}"#.to_string(),
        }];
        let (state, transport) = test_state(responses);
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
        let (state, _transport) = test_state_with_auth(
            Vec::new(),
            vec!["npub1admin".to_string()],
            "gittree",
        );
        let app = build_router(state);
        let payload = json!({
            "kind": KIND_GITTREE_CONTROL.0,
            "pubkey": "npub1other",
            "content": r#"{"action":"create_repo","name":"demo"}"#
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
        let (state, transport) = test_state_with_auth(responses, Vec::new(), "gittree");
        let app = build_router(state);
        let payload = json!({
            "kind": KIND_GITTREE_CONTROL.0,
            "pubkey": "npub1admin",
            "content": r#"{"action":"create_repo","name":"demo"}"#
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
    }
}
