use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use gittree_config::{ConfigError, ControlAuthConfig, ForgejoConfig, ServicesConfig};
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
    ObservabilityConfig(ObservabilityConfigError),
    Observability(ObservabilityError),
    Serve(String),
}

impl std::fmt::Display for ControlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ControlError::Config(err) => write!(f, "control error: {err}"),
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
struct ControlAppState {
    auth: ControlAuthConfig,
    forgejo: ForgejoConfig,
}

pub async fn serve(config: ControlConfig) -> Result<(), ControlError> {
    let _observability = init_observability()?;
    let state = ControlAppState {
        auth: config.auth,
        forgejo: config.forgejo,
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

fn build_router(state: ControlAppState) -> Router {
    Router::new()
        .route("/health", get(health_handler))
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
    use axum::body::Body;
    use axum::http::{HeaderMap, Request};
    use gittree_config::{ControlAuthConfig, ForgejoConfig};
    use tower::ServiceExt;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

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
        let state = super::ControlAppState {
            auth: ControlAuthConfig {
                token: "token".to_string(),
                admin_keys: Vec::new(),
            },
            forgejo: ForgejoConfig {
                base_url: "http://localhost:3000".to_string(),
                api_token: "token".to_string(),
                owner: "gittree".to_string(),
                webhook_url: "http://localhost:8087/".to_string(),
                webhook_secret: "secret".to_string(),
                repo_private: true,
            },
        };
        let app = build_router(state);
        let response = app
            .oneshot(Request::builder().uri("/health").body(Body::empty()).unwrap())
            .await
            .expect("response");
        assert_eq!(response.status(), axum::http::StatusCode::OK);
    }
}
