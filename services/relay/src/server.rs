use crate::{
    AdmissionDecider, AdmissionHookClient, EventStore, Policy, RelayConfig, RelayError,
    RelayMetrics, RepositoryStore, Session, SessionDriver, build_nip11_document,
};
use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use futures_util::{SinkExt, StreamExt};
use gittree_storage::{PostgresRepositories, RelayTenantRecord, RelayTenantRepository};
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc};

#[derive(Clone)]
struct RelayState {
    config: RelayConfig,
    policy: Policy,
    store: Arc<dyn EventStore>,
    repos: Option<Arc<PostgresRepositories>>,
    admission: Option<Arc<dyn AdmissionDecider>>,
    broadcast: broadcast::Sender<crate::NostrEvent>,
    metrics: Arc<RelayMetrics>,
}

#[derive(Clone)]
struct TenantContext {
    tenant_id: String,
    store: Arc<dyn EventStore>,
    tenant: Option<RelayTenantRecord>,
}

pub async fn serve(config: RelayConfig) -> Result<(), RelayError> {
    let _observability = crate::init_observability()?;
    let pool_options = config.storage.pool_options().map_err(RelayError::Storage)?;
    let connect_options = config
        .storage
        .write_connect_options()
        .map_err(RelayError::Storage)?;
    let pool = pool_options.connect_lazy_with(connect_options);
    let repos = PostgresRepositories::new(pool);
    let store: Arc<dyn EventStore> = Arc::new(RepositoryStore::new(repos.clone()));
    let (broadcast, _) = broadcast::channel(1024);
    let policy = Policy::from_config(&config.policy);
    let metrics = Arc::new(RelayMetrics::new());
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
        repos: Some(Arc::new(repos)),
        admission,
        broadcast,
        metrics,
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
        let tenant = match resolve_tenant(&state, &headers).await {
            Ok(tenant) => tenant,
            Err(response) => return response,
        };
        return ws
            .on_upgrade(move |socket| handle_socket(socket, state, tenant))
            .into_response();
    }

    if accepts_nostr_json(&headers) {
        let tenant = match resolve_tenant(&state, &headers).await {
            Ok(tenant) => tenant,
            Err(response) => return response,
        };
        return nip11_response(&state, &tenant).into_response();
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

fn nip11_response(state: &RelayState, tenant: &TenantContext) -> Response {
    let doc = build_nip11_document(&state.config, &state.policy, tenant.tenant.as_ref());
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

async fn resolve_tenant(
    state: &RelayState,
    headers: &HeaderMap,
) -> Result<TenantContext, Response> {
    let Some(repos) = &state.repos else {
        return Ok(TenantContext {
            tenant_id: "default".to_string(),
            store: state.store.clone(),
            tenant: None,
        });
    };

    let host = extract_host(headers).ok_or_else(|| StatusCode::BAD_REQUEST.into_response())?;
    let tenant = repos
        .tenant_by_host(&host)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())?;
    let Some(tenant) = tenant else {
        return Err(StatusCode::NOT_FOUND.into_response());
    };
    let store: Arc<dyn EventStore> =
        Arc::new(RepositoryStore::with_tenant(repos.as_ref().clone(), tenant.id.clone()));
    Ok(TenantContext {
        tenant_id: tenant.id.clone(),
        store,
        tenant: Some(tenant),
    })
}

fn extract_host(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .map(normalize_host)
}

fn normalize_host(value: &str) -> String {
    let value = value.trim().trim_end_matches('.');
    if let Some(value) = value.strip_prefix('[') {
        if let Some(end) = value.find(']') {
            return value[..end].to_ascii_lowercase();
        }
    }
    value
        .split(':')
        .next()
        .unwrap_or(value)
        .to_ascii_lowercase()
}

async fn health_handler() -> &'static str {
    "ok"
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}

async fn handle_socket(socket: WebSocket, state: Arc<RelayState>, tenant: TenantContext) {
    let _ = (&tenant.tenant_id, &tenant.tenant);
    let (mut sender, mut receiver) = socket.split();
    let (in_tx, in_rx) = mpsc::channel(128);
    let (out_tx, mut out_rx) = mpsc::channel(128);
    let broadcast_rx = state.broadcast.subscribe();

    let (read_auth_required, write_auth_required) = match tenant.tenant.as_ref() {
        Some(record) => (!record.public_read, record.auth_required),
        None => (
            state.config.policy.auth_required,
            state.config.policy.auth_required,
        ),
    };
    let session = Session::with_broadcast(
        tenant.store.clone(),
        state.policy,
        state.admission.clone(),
        state.broadcast.clone(),
        read_auth_required,
        write_auth_required,
    )
    .with_metrics(state.metrics.clone());
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
    use crate::{MemoryStore, NostrEvent, Policy, RelayConfig, RelayMetrics};
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::http::header::{ACCEPT, CONTENT_TYPE};
    use futures_util::{SinkExt, StreamExt};
    use secp256k1::{Keypair, Secp256k1, SecretKey};
    use serde_json::json;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::net::TcpListener;
    use tokio::time::timeout;
    use tokio_tungstenite::connect_async;
    use tokio_tungstenite::tungstenite::Message as WsMessage;
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

    async fn spawn_ws_server() -> std::net::SocketAddr {
        let state = Arc::new(super::RelayState {
            config: sample_config(),
            policy: Policy::default(),
            store: Arc::new(MemoryStore::new()),
            repos: None,
            admission: None,
            broadcast: tokio::sync::broadcast::channel(8).0,
            metrics: Arc::new(RelayMetrics::new()),
        });
        let app = build_router(state);
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve");
        });
        addr
    }

    fn signed_event(seed: &str) -> NostrEvent {
        let secp = Secp256k1::new();
        let secret_key = SecretKey::from_slice(&[0x11; 32]).expect("secret");
        let keypair = Keypair::from_secret_key(&secp, &secret_key);
        let (pubkey, _) = secp256k1::XOnlyPublicKey::from_keypair(&keypair);

        let mut event = NostrEvent {
            id: seed.to_string(),
            pubkey: hex::encode(pubkey.serialize()),
            created_at: 1,
            kind: 1,
            tags: Vec::new(),
            content: String::new(),
            sig: String::new(),
        };
        event.id = event.compute_id().expect("id");
        let id_bytes = hex::decode(&event.id).expect("id bytes");
        let msg = secp256k1::Message::from_digest_slice(&id_bytes).expect("msg");
        let sig = secp.sign_schnorr(&msg, &keypair);
        event.sig = hex::encode(sig.as_ref());
        event
    }

    #[tokio::test]
    async fn nip11_requires_accept_header() {
        let state = Arc::new(super::RelayState {
            config: sample_config(),
            policy: Policy::default(),
            store: Arc::new(MemoryStore::new()),
            repos: None,
            admission: None,
            broadcast: tokio::sync::broadcast::channel(8).0,
            metrics: Arc::new(RelayMetrics::new()),
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
            repos: None,
            admission: None,
            broadcast: tokio::sync::broadcast::channel(8).0,
            metrics: Arc::new(RelayMetrics::new()),
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
            repos: None,
            admission: None,
            broadcast: tokio::sync::broadcast::channel(8).0,
            metrics: Arc::new(RelayMetrics::new()),
        });
        let app = build_router(state);

        let response = app
            .oneshot(Request::builder().uri("/health").body(Body::empty()).unwrap())
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn websocket_req_sends_eose() {
        let addr = spawn_ws_server().await;
        let url = format!("ws://{addr}/");
        let (mut socket, _) = connect_async(url).await.expect("connect");

        socket
            .send(WsMessage::Text(r#"["REQ","sub",{}]"#.to_string()))
            .await
            .expect("send");

        let message = timeout(Duration::from_secs(2), socket.next())
            .await
            .expect("timeout")
            .expect("message")
            .expect("ws");
        let payload = message.into_text().expect("text");
        let value: serde_json::Value = serde_json::from_str(&payload).expect("json");
        assert_eq!(value[0], "EOSE");
    }

    #[tokio::test]
    async fn websocket_event_returns_ok() {
        let addr = spawn_ws_server().await;
        let url = format!("ws://{addr}/");
        let (mut socket, _) = connect_async(url).await.expect("connect");

        let event = signed_event("event-1");
        let payload = json!(["EVENT", event]).to_string();
        socket
            .send(WsMessage::Text(payload))
            .await
            .expect("send");

        let message = timeout(Duration::from_secs(2), socket.next())
            .await
            .expect("timeout")
            .expect("message")
            .expect("ws");
        let payload = message.into_text().expect("text");
        let value: serde_json::Value = serde_json::from_str(&payload).expect("json");
        assert_eq!(value[0], "OK");
        assert_eq!(value[2], true);
    }

    #[test]
    fn nip11_response_sets_cors() {
        let state = super::RelayState {
            config: sample_config(),
            policy: Policy::default(),
            store: Arc::new(MemoryStore::new()),
            repos: None,
            admission: None,
            broadcast: tokio::sync::broadcast::channel(8).0,
            metrics: Arc::new(RelayMetrics::new()),
        };
        let tenant = super::TenantContext {
            tenant_id: "default".to_string(),
            store: state.store.clone(),
            tenant: None,
        };
        let response = nip11_response(&state, &tenant);
        let cors = response
            .headers()
            .get(axum::http::header::ACCESS_CONTROL_ALLOW_ORIGIN)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default();
        assert_eq!(cors, "*");
    }

    #[test]
    fn normalize_host_strips_port() {
        assert_eq!(super::normalize_host("Example.COM:8080"), "example.com");
    }

    #[test]
    fn normalize_host_handles_ipv6() {
        assert_eq!(super::normalize_host("[::1]:8080"), "::1");
    }
}
