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
use gittree_storage::{
    PostgresRepositories, RelayMembershipRecord, RelayMembershipRepository, RelayTenantRecord,
    RelayTenantRepository, StorageError,
};
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
        .trim_end_matches('.')
        .to_ascii_lowercase()
}

fn relay_url_from_host(host: &str) -> String {
    let host = host.trim();
    if host.starts_with("ws://") || host.starts_with("wss://") {
        host.to_string()
    } else {
        format!("wss://{host}")
    }
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

    let (read_auth_required, write_auth_required, read_membership_required, write_membership_required) =
        match tenant.tenant.as_ref() {
            Some(record) => {
                let read_membership_required = !record.public_read;
                let write_membership_required = !record.public_write;
                (
                    read_membership_required,
                    record.auth_required || write_membership_required,
                    read_membership_required,
                    write_membership_required,
                )
            }
            None => (
                state.config.policy.auth_required,
                state.config.policy.auth_required,
                false,
                false,
            ),
        };
    let relay_url = tenant
        .tenant
        .as_ref()
        .map(|record| relay_url_from_host(&record.host));
    let membership = state
        .repos
        .as_ref()
        .map(|repos| repos.clone() as Arc<dyn RelayMembershipRepository>);
    let mut session = Session::with_broadcast(
        tenant.store.clone(),
        state.policy,
        state.admission.clone(),
        state.broadcast.clone(),
        read_auth_required,
        write_auth_required,
    )
    .with_membership(Some(tenant.tenant_id.clone()), membership.clone())
    .with_membership_requirements(read_membership_required, write_membership_required)
    .with_relay_url(relay_url);

    if let Some(record) = tenant.tenant.as_ref() {
        session =
            session.with_relay_signer(record.relay_pubkey.clone(), record.relay_secret.clone());
        if let Some(membership) = membership.as_ref() {
            let _ = seed_owner_membership(
                membership,
                &tenant.tenant_id,
                &record.relay_pubkey,
            )
            .await;
        }
    }

    let session = session.with_metrics(state.metrics.clone());
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

async fn seed_owner_membership(
    membership: &Arc<dyn RelayMembershipRepository>,
    tenant_id: &str,
    relay_pubkey: &[u8],
) -> Result<(), StorageError> {
    let existing = membership
        .membership_by_pubkey(tenant_id, relay_pubkey)
        .await?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let created_at = existing
        .as_ref()
        .map(|record| record.created_at)
        .unwrap_or(now);
    let record = RelayMembershipRecord {
        tenant_id: tenant_id.to_string(),
        pubkey: relay_pubkey.to_vec(),
        role: "owner".to_string(),
        status: "active".to_string(),
        created_at,
        updated_at: now,
    };
    membership.upsert_membership(record).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        accepts_nostr_json, build_router, extract_host, handle_socket, nip11_response,
        relay_url_from_host, resolve_tenant, seed_owner_membership, shutdown_signal,
    };
    use crate::{MemoryStore, NostrEvent, Policy, RelayConfig, RelayMetrics};
    use axum::Router;
    use axum::extract::ws::WebSocketUpgrade;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::http::header::{ACCEPT, CONTENT_TYPE};
    use axum::routing::get;
    use axum::response::IntoResponse;
    use futures_util::{SinkExt, StreamExt};
    use gittree_storage::{InMemoryRepositories, RelayMembershipRepository};
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

    fn unreachable_repos() -> Arc<gittree_storage::PostgresRepositories> {
        let storage = gittree_storage::StorageConfig {
            read_connection: "postgres://user:pass@127.0.0.1:1/gittree".to_string(),
            write_connection: None,
            max_connections: 1,
            min_connections: 0,
            idle_timeout_secs: None,
            max_lifetime_secs: None,
            application_name: Some("gittree-test".to_string()),
        };
        let pool_options = storage.pool_options().expect("pool options");
        let connect_options = storage.read_connect_options().expect("connect options");
        Arc::new(gittree_storage::PostgresRepositories::new(
            pool_options.connect_lazy_with(connect_options),
        ))
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
            let _ = axum::serve(listener, app).await;
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

    fn sample_tenant_record() -> gittree_storage::RelayTenantRecord {
        gittree_storage::RelayTenantRecord::new(
            "tenant.local",
            "tenant.local",
            &"44".repeat(32),
            vec![1, 2, 3],
            vec![4, 5, 6],
            "v1",
            Some("Tenant".to_string()),
            None,
            None,
            None,
            None,
            true,
            false,
            false,
            1,
            1,
        )
        .expect("tenant")
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
    async fn nip11_tenant_mode_requires_host_header() {
        let state = Arc::new(super::RelayState {
            config: sample_config(),
            policy: Policy::default(),
            store: Arc::new(MemoryStore::new()),
            repos: Some(unreachable_repos()),
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
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn nip11_tenant_mode_maps_repository_errors_to_500() {
        let state = Arc::new(super::RelayState {
            config: sample_config(),
            policy: Policy::default(),
            store: Arc::new(MemoryStore::new()),
            repos: Some(unreachable_repos()),
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
                    .header("host", "tenant.local")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
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

    #[tokio::test]
    async fn websocket_upgrade_maps_tenant_lookup_failure_to_http_error() {
        let state = Arc::new(super::RelayState {
            config: sample_config(),
            policy: Policy::default(),
            store: Arc::new(MemoryStore::new()),
            repos: Some(unreachable_repos()),
            admission: None,
            broadcast: tokio::sync::broadcast::channel(8).0,
            metrics: Arc::new(RelayMetrics::new()),
        });
        let app = build_router(state);
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        let url = format!("ws://{addr}/");
        let err = connect_async(url).await.expect_err("expected handshake failure");
        match err {
            tokio_tungstenite::tungstenite::Error::Http(response) => {
                assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
            }
            other => panic!("expected http handshake error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn handle_socket_tenant_mode_executes_signer_and_membership_paths() {
        let tenant = sample_tenant_record();
        let state = Arc::new(super::RelayState {
            config: sample_config(),
            policy: Policy::default(),
            store: Arc::new(MemoryStore::new()),
            repos: Some(unreachable_repos()),
            admission: None,
            broadcast: tokio::sync::broadcast::channel(8).0,
            metrics: Arc::new(RelayMetrics::new()),
        });
        let tenant_context = super::TenantContext {
            tenant_id: tenant.id.clone(),
            store: Arc::new(MemoryStore::new()),
            tenant: Some(tenant),
        };
        let app = Router::new().route(
            "/",
            get(move |ws: WebSocketUpgrade| {
                let state = state.clone();
                let tenant = tenant_context.clone();
                async move { ws.on_upgrade(move |socket| handle_socket(socket, state, tenant)) }
            }),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        let url = format!("ws://{addr}/");
        let (mut socket, _) = connect_async(url).await.expect("connect");
        socket
            .send(WsMessage::Text(r#"["REQ","sub",{}]"#.to_string()))
            .await
            .expect("send req");
        socket.send(WsMessage::Close(None)).await.expect("close");
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

    #[test]
    fn normalize_host_handles_malformed_ipv6_prefix() {
        assert_eq!(super::normalize_host("[::1"), "[");
    }

    #[test]
    fn accepts_nostr_json_matches_mixed_accept_header() {
        let mut headers = axum::http::HeaderMap::new();
        headers.insert(
            ACCEPT,
            "text/html,application/nostr+json;q=0.9".parse().expect("accept"),
        );
        assert!(accepts_nostr_json(&headers));
    }

    #[test]
    fn extract_host_returns_none_when_missing() {
        let headers = axum::http::HeaderMap::new();
        assert!(extract_host(&headers).is_none());
    }

    #[test]
    fn extract_host_normalizes_case_and_trailing_dot() {
        let mut headers = axum::http::HeaderMap::new();
        headers.insert("host", "Relay.Local.:443".parse().expect("host"));
        assert_eq!(extract_host(&headers).as_deref(), Some("relay.local"));
    }

    #[test]
    fn relay_url_from_host_adds_scheme_when_missing() {
        assert_eq!(relay_url_from_host("relay.local"), "wss://relay.local");
        assert_eq!(relay_url_from_host("ws://relay.local"), "ws://relay.local");
    }

    #[tokio::test]
    async fn resolve_tenant_defaults_without_repository_backing() {
        let state = super::RelayState {
            config: sample_config(),
            policy: Policy::default(),
            store: Arc::new(MemoryStore::new()),
            repos: None,
            admission: None,
            broadcast: tokio::sync::broadcast::channel(8).0,
            metrics: Arc::new(RelayMetrics::new()),
        };
        let headers = axum::http::HeaderMap::new();
        let tenant = resolve_tenant(&state, &headers).await.expect("tenant");
        assert_eq!(tenant.tenant_id, "default");
        assert!(tenant.tenant.is_none());
    }

    #[tokio::test]
    async fn resolve_tenant_requires_host_when_repositories_enabled() {
        let state = super::RelayState {
            config: sample_config(),
            policy: Policy::default(),
            store: Arc::new(MemoryStore::new()),
            repos: Some(unreachable_repos()),
            admission: None,
            broadcast: tokio::sync::broadcast::channel(8).0,
            metrics: Arc::new(RelayMetrics::new()),
        };
        let headers = axum::http::HeaderMap::new();
        let response = resolve_tenant(&state, &headers)
            .await
            .err()
            .unwrap_or_else(|| StatusCode::IM_A_TEAPOT.into_response());
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn resolve_tenant_maps_repository_failures_to_500() {
        let state = super::RelayState {
            config: sample_config(),
            policy: Policy::default(),
            store: Arc::new(MemoryStore::new()),
            repos: Some(unreachable_repos()),
            admission: None,
            broadcast: tokio::sync::broadcast::channel(8).0,
            metrics: Arc::new(RelayMetrics::new()),
        };

        let mut headers = axum::http::HeaderMap::new();
        headers.insert("host", "tenant.local".parse().expect("host"));
        let response = resolve_tenant(&state, &headers)
            .await
            .err()
            .unwrap_or_else(|| StatusCode::IM_A_TEAPOT.into_response());
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[tokio::test]
    async fn seed_owner_membership_sets_owner_role_and_preserves_created_at() {
        let repo = Arc::new(InMemoryRepositories::new());
        let membership: Arc<dyn RelayMembershipRepository> = repo.clone();
        let tenant_id = "tenant-test";
        let relay_pubkey = vec![0x11; 32];

        seed_owner_membership(&membership, tenant_id, &relay_pubkey)
            .await
            .expect("seed first");
        let first = repo
            .membership_by_pubkey(tenant_id, &relay_pubkey)
            .await
            .expect("lookup first")
            .expect("membership");

        tokio::time::sleep(Duration::from_millis(5)).await;
        seed_owner_membership(&membership, tenant_id, &relay_pubkey)
            .await
            .expect("seed second");
        let second = repo
            .membership_by_pubkey(tenant_id, &relay_pubkey)
            .await
            .expect("lookup second")
            .expect("membership");

        assert_eq!(first.created_at, second.created_at);
        assert!(second.updated_at >= first.updated_at);
        assert_eq!(second.role, "owner");
        assert_eq!(second.status, "active");
    }

    #[tokio::test]
    async fn shutdown_signal_future_can_start_and_be_aborted() {
        let task = tokio::spawn(shutdown_signal());
        tokio::time::sleep(Duration::from_millis(10)).await;
        task.abort();
    }
}
