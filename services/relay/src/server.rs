use crate::{
    AdmissionDecider, AdmissionHookClient, EventStore, Policy, RelayConfig, RelayError,
    RepositoryStore, Session, SessionDriver, build_nip11_document,
};
use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use futures_util::{SinkExt, StreamExt};
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc};

#[derive(Clone)]
struct RelayState {
    config: RelayConfig,
    policy: Policy,
    store: Arc<dyn EventStore>,
    admission: Option<Arc<dyn AdmissionDecider>>,
    broadcast: broadcast::Sender<crate::NostrEvent>,
}

pub async fn serve(config: RelayConfig) -> Result<(), RelayError> {
    let _observability = crate::init_observability()?;
    let pool_options = config.storage.pool_options().map_err(RelayError::Storage)?;
    let connect_options = config
        .storage
        .write_connect_options()
        .map_err(RelayError::Storage)?;
    let pool = pool_options.connect_lazy_with(connect_options);
    let repos = gittree_storage::PostgresRepositories::new(pool);
    let store: Arc<dyn EventStore> = Arc::new(RepositoryStore::new(repos));
    let (broadcast, _) = broadcast::channel(1024);
    let policy = Policy::from_config(&config.policy);
    let admission = match &config.admission {
        Some(config) => Some(Arc::new(
            AdmissionHookClient::new_http(config.clone()).map_err(RelayError::Admission)?,
        ) as Arc<dyn AdmissionDecider>),
        None => None,
    };
    let state = RelayState {
        config: config.clone(),
        policy,
        store,
        admission,
        broadcast,
    };

    let router = build_router(Arc::new(state));
    let listener = tokio::net::TcpListener::bind(&config.bind)
        .await
        .map_err(|err| RelayError::Serve(err.to_string()))?;
    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .map_err(|err| RelayError::Serve(err.to_string()))?;
    Ok(())
}

fn build_router(state: Arc<RelayState>) -> Router {
    Router::new()
        .route("/", get(root_handler))
        .route("/health", get(health_handler))
        .with_state(state)
}

async fn root_handler(
    ws: Option<WebSocketUpgrade>,
    State(state): State<Arc<RelayState>>,
    headers: HeaderMap,
) -> Response {
    if let Some(ws) = ws {
        return ws.on_upgrade(move |socket| handle_socket(socket, state)).into_response();
    }

    if accepts_nostr_json(&headers) {
        return nip11_response(&state).into_response();
    }

    StatusCode::NOT_FOUND.into_response()
}

fn accepts_nostr_json(headers: &HeaderMap) -> bool {
    headers
        .get(header::ACCEPT)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.contains("application/nostr+json"))
        .unwrap_or(false)
}

fn nip11_response(state: &RelayState) -> Response {
    let doc = build_nip11_document(&state.config, &state.policy);
    let mut response = Json(doc).into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/nostr+json"),
    );
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_ORIGIN,
        HeaderValue::from_static("*"),
    );
    response
}

async fn health_handler() -> &'static str {
    "ok"
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}

async fn handle_socket(socket: WebSocket, state: Arc<RelayState>) {
    let (mut sender, mut receiver) = socket.split();
    let (in_tx, in_rx) = mpsc::channel(128);
    let (out_tx, mut out_rx) = mpsc::channel(128);
    let broadcast_rx = state.broadcast.subscribe();

    let session = Session::with_broadcast(
        state.store.clone(),
        state.policy,
        state.admission.clone(),
        state.broadcast.clone(),
        state.config.policy.auth_required,
    );
    let driver = SessionDriver::new(session);
    tokio::spawn(driver.run_with_broadcast(in_rx, out_tx, broadcast_rx));

    loop {
        tokio::select! {
            msg = receiver.next() => {
                let Some(msg) = msg else {
                    break;
                };
                match msg {
                    Ok(Message::Text(text)) => {
                        if in_tx.send(text).await.is_err() {
                            break;
                        }
                    }
                    Ok(Message::Close(_)) => break,
                    Ok(_) => {}
                    Err(_) => break,
                }
            }
            outbound = out_rx.recv() => {
                let Some(outbound) = outbound else {
                    break;
                };
                if sender.send(Message::Text(outbound)).await.is_err() {
                    break;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{build_router, nip11_response};
    use crate::{MemoryStore, Policy, RelayConfig};
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::http::header::{ACCEPT, CONTENT_TYPE};
    use std::sync::Arc;
    use tower::ServiceExt;

    fn sample_config() -> RelayConfig {
        RelayConfig {
            bind: "127.0.0.1:0".to_string(),
            storage: gittree_storage::StorageConfig {
                read_connection: "postgres://user:pass@localhost:5432/gittree".to_string(),
                write_connection: None,
                max_connections: 10,
                min_connections: 2,
                idle_timeout_secs: None,
                max_lifetime_secs: None,
                application_name: Some("gittree".to_string()),
            },
            policy: gittree_config::RelayPolicyConfig::default(),
            admission: None,
        }
    }

    #[tokio::test]
    async fn nip11_requires_accept_header() {
        let state = Arc::new(super::RelayState {
            config: sample_config(),
            policy: Policy::default(),
            store: Arc::new(MemoryStore::new()),
            admission: None,
            broadcast: tokio::sync::broadcast::channel(8).0,
        });
        let app = build_router(state);

        let response = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn nip11_returns_document() {
        let state = Arc::new(super::RelayState {
            config: sample_config(),
            policy: Policy::default(),
            store: Arc::new(MemoryStore::new()),
            admission: None,
            broadcast: tokio::sync::broadcast::channel(8).0,
        });
        let app = build_router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/")
                    .header(ACCEPT, "application/nostr+json")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::OK);
        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default();
        assert_eq!(content_type, "application/nostr+json");
    }

    #[tokio::test]
    async fn health_endpoint_returns_ok() {
        let state = Arc::new(super::RelayState {
            config: sample_config(),
            policy: Policy::default(),
            store: Arc::new(MemoryStore::new()),
            admission: None,
            broadcast: tokio::sync::broadcast::channel(8).0,
        });
        let app = build_router(state);

        let response = app
            .oneshot(Request::builder().uri("/health").body(Body::empty()).unwrap())
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[test]
    fn nip11_response_sets_cors() {
        let state = super::RelayState {
            config: sample_config(),
            policy: Policy::default(),
            store: Arc::new(MemoryStore::new()),
            admission: None,
            broadcast: tokio::sync::broadcast::channel(8).0,
        };
        let response = nip11_response(&state);
        let cors = response
            .headers()
            .get(axum::http::header::ACCESS_CONTROL_ALLOW_ORIGIN)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default();
        assert_eq!(cors, "*");
    }
}
