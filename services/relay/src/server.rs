use crate::{
    AdmissionDecider, AdmissionHookClient, EventStore, Policy, RelayConfig, RelayError,
    RelayMetrics, RepositoryStore, Session, SessionDriver, build_nip11_document,
};
use async_trait::async_trait;
use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{Sink, SinkExt, Stream, StreamExt};
use gittree_storage::{
    PostgresRepositories, RelayMembershipRecord, RelayMembershipRepository, RelayTenantRecord,
    RelayTenantRepository, StorageError,
};
use std::future::Future;
use std::future::IntoFuture;
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc};

#[derive(Clone)]
struct RelayState {
    config: RelayConfig,
    policy: Policy,
    store: Arc<dyn EventStore>,
    repos: Option<Arc<dyn TenantRepository>>,
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

#[async_trait]
trait TenantRepository: Send + Sync {
    async fn tenant_by_host(&self, host: &str) -> Result<Option<RelayTenantRecord>, StorageError>;
    fn tenant_store(&self, tenant_id: &str) -> Arc<dyn EventStore>;
    fn membership_repository(&self) -> Arc<dyn RelayMembershipRepository>;
}

#[async_trait]
impl TenantRepository for PostgresRepositories {
    async fn tenant_by_host(&self, host: &str) -> Result<Option<RelayTenantRecord>, StorageError> {
        RelayTenantRepository::tenant_by_host(self, host).await
    }

    fn tenant_store(&self, tenant_id: &str) -> Arc<dyn EventStore> {
        Arc::new(RepositoryStore::with_tenant(
            self.clone(),
            tenant_id.to_string(),
        ))
    }

    fn membership_repository(&self) -> Arc<dyn RelayMembershipRepository> {
        Arc::new(self.clone())
    }
}

pub async fn serve(config: RelayConfig) -> Result<(), RelayError> {
    let _observability = crate::init_observability()?;
    serve_inner(config).await
}

async fn serve_inner(config: RelayConfig) -> Result<(), RelayError> {
    serve_inner_with_shutdown(config, await_shutdown_signal(tokio::signal::ctrl_c())).await
}

async fn serve_inner_with_shutdown<F>(config: RelayConfig, shutdown: F) -> Result<(), RelayError>
where
    F: std::future::Future<Output = ()> + Send + 'static,
{
    let state = build_state(config.clone())?;
    let router = build_router(Arc::new(state));
    let listener = bind_listener(&config.bind).await?;
    run_server(axum::serve(listener, router).with_graceful_shutdown(shutdown)).await
}

async fn run_server<E, S>(server: S) -> Result<(), RelayError>
where
    E: std::fmt::Display,
    S: IntoFuture<Output = Result<(), E>>,
{
    match server.into_future().await {
        Ok(()) => Ok(()),
        Err(err) => Err(RelayError::Serve(err.to_string())),
    }
}

fn build_state(config: RelayConfig) -> Result<RelayState, RelayError> {
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
        Some(config) => {
            Some(Arc::new(AdmissionHookClient::new_http(config.clone())) as Arc<dyn AdmissionDecider>)
        }
        None => None,
    };
    Ok(RelayState {
        config: config.clone(),
        policy,
        store,
        repos: Some(Arc::new(repos)),
        admission,
        broadcast,
        metrics,
    })
}

async fn bind_listener(bind: &str) -> Result<tokio::net::TcpListener, RelayError> {
    match tokio::net::TcpListener::bind(bind).await {
        Ok(listener) => Ok(listener),
        Err(err) => Err(RelayError::Serve(err.to_string())),
    }
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
    let store = repos.tenant_store(&tenant.id);
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

async fn await_shutdown_signal<S>(signal: S)
where
    S: std::future::Future<Output = Result<(), std::io::Error>>,
{
    let _ = signal.await;
}

enum InboundSocketAction {
    Forward(String),
    Continue,
    Break,
}

fn classify_inbound_message(msg: Option<Result<Message, axum::Error>>) -> InboundSocketAction {
    let Some(msg) = msg else {
        return InboundSocketAction::Break;
    };
    match msg {
        Ok(Message::Text(text)) => InboundSocketAction::Forward(text.to_string()),
        Ok(Message::Close(_)) | Err(_) => InboundSocketAction::Break,
        Ok(_) => InboundSocketAction::Continue,
    }
}

fn classify_outbound_message(outbound: Option<String>) -> Option<Message> {
    outbound.map(Message::Text)
}

async fn handle_inbound_action(in_tx: &mpsc::Sender<String>, action: InboundSocketAction) -> bool {
    match action {
        InboundSocketAction::Forward(text) => in_tx.send(text).await.is_ok(),
        InboundSocketAction::Continue => true,
        InboundSocketAction::Break => false,
    }
}

async fn handle_outbound_message<S>(sender: &mut S, outbound: Option<String>) -> bool
where
    S: Sink<Message> + Unpin,
{
    let Some(outbound) = classify_outbound_message(outbound) else {
        return false;
    };
    sender.send(outbound).await.is_ok()
}

async fn pump_socket_io<R, S>(
    receiver: &mut R,
    sender: &mut S,
    in_tx: &mpsc::Sender<String>,
    out_rx: &mut mpsc::Receiver<String>,
) where
    R: Stream<Item = Result<Message, axum::Error>> + Unpin,
    S: Sink<Message> + Unpin,
{
    while tokio::select! {
        biased;
        msg = receiver.next() => {
            let action = classify_inbound_message(msg);
            handle_inbound_action(in_tx, action).await
        }
        outbound = out_rx.recv() => {
            handle_outbound_message(sender, outbound).await
        }
    } {}
}

type SocketReceiver = SplitStream<WebSocket>;
type SocketSender = SplitSink<WebSocket, Message>;
type SocketPumpFuture<'a> = Pin<Box<dyn Future<Output = ()> + Send + 'a>>;

fn default_socket_pump<'a>(
    receiver: &'a mut SocketReceiver,
    sender: &'a mut SocketSender,
    in_tx: &'a mpsc::Sender<String>,
    out_rx: &'a mut mpsc::Receiver<String>,
) -> SocketPumpFuture<'a> {
    Box::pin(pump_socket_io(receiver, sender, in_tx, out_rx))
}

async fn handle_socket_with_pump<P>(
    socket: WebSocket,
    state: Arc<RelayState>,
    tenant: TenantContext,
    pump: P,
) where
    P: for<'a> FnOnce(
        &'a mut SocketReceiver,
        &'a mut SocketSender,
        &'a mpsc::Sender<String>,
        &'a mut mpsc::Receiver<String>,
    ) -> SocketPumpFuture<'a>,
{
    let _ = (&tenant.tenant_id, &tenant.tenant);
    let (mut sender, mut receiver) = socket.split();
    let (in_tx, in_rx) = mpsc::channel(128);
    let (out_tx, mut out_rx) = mpsc::channel(128);
    let broadcast_rx = state.broadcast.subscribe();

    let (
        read_auth_required,
        write_auth_required,
        read_membership_required,
        write_membership_required,
    ) = match tenant.tenant.as_ref() {
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
        .map(|repos| repos.membership_repository());
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

    if let (Some(record), Some(membership)) = (tenant.tenant.as_ref(), membership.as_ref()) {
        session =
            session.with_relay_signer(record.relay_pubkey.clone(), record.relay_secret.clone());
        let _ = seed_owner_membership(membership, &tenant.tenant_id, &record.relay_pubkey).await;
    }

    let session = session.with_metrics(state.metrics.clone());
    let driver = SessionDriver::new(session);
    tokio::spawn(driver.run_with_broadcast(in_rx, out_tx, broadcast_rx));

    pump(&mut receiver, &mut sender, &in_tx, &mut out_rx).await;
}

fn handle_socket(
    socket: WebSocket,
    state: Arc<RelayState>,
    tenant: TenantContext,
) -> impl std::future::Future<Output = ()> + Send + 'static {
    handle_socket_with_pump(socket, state, tenant, default_socket_pump)
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
        accepts_nostr_json, bind_listener, build_router, build_state, extract_host, handle_socket,
        handle_socket_with_pump, nip11_response, relay_url_from_host, resolve_tenant,
        seed_owner_membership,
    };
    use crate::{MemoryStore, NostrEvent, Policy, RelayConfig, RelayError, RelayMetrics};
    use async_trait::async_trait;
    use axum::Router;
    use axum::body::{Body, to_bytes};
    use axum::extract::ws::{Message, WebSocketUpgrade};
    use axum::http::header::{ACCEPT, CONTENT_TYPE};
    use axum::http::{Request, StatusCode};
    use axum::routing::get;
    use futures_util::{SinkExt, StreamExt};
    use gittree_storage::{
        InMemoryRepositories, RelayInviteRecord, RelayMembershipRecord, RelayMembershipRepository,
        StorageError,
    };
    use secp256k1::{Keypair, Secp256k1, SecretKey};
    use serde_json::json;
    use std::collections::HashMap;
    use std::pin::Pin;
    use std::sync::Arc;
    use std::task::{Context, Poll};
    use std::time::Duration;
    use tokio::net::TcpListener;
    use tokio::sync::mpsc;
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

    struct FakeTenantRepository {
        tenants_by_host: HashMap<String, gittree_storage::RelayTenantRecord>,
    }

    impl FakeTenantRepository {
        fn with_tenant(host: &str, tenant: gittree_storage::RelayTenantRecord) -> Self {
            let mut tenants_by_host = HashMap::new();
            tenants_by_host.insert(host.to_string(), tenant);
            Self { tenants_by_host }
        }
    }

    #[async_trait]
    impl super::TenantRepository for FakeTenantRepository {
        async fn tenant_by_host(
            &self,
            host: &str,
        ) -> Result<Option<gittree_storage::RelayTenantRecord>, gittree_storage::StorageError>
        {
            Ok(self.tenants_by_host.get(host).cloned())
        }

        fn tenant_store(&self, _tenant_id: &str) -> Arc<dyn crate::EventStore> {
            Arc::new(MemoryStore::new())
        }

        fn membership_repository(&self) -> Arc<dyn RelayMembershipRepository> {
            Arc::new(gittree_storage::InMemoryRepositories::new())
        }
    }

    struct FailingMembershipRepository {
        fail_lookup: bool,
        fail_upsert: bool,
    }

    impl FailingMembershipRepository {
        fn lookup() -> Self {
            Self {
                fail_lookup: true,
                fail_upsert: false,
            }
        }

        fn upsert() -> Self {
            Self {
                fail_lookup: false,
                fail_upsert: true,
            }
        }
    }

    #[async_trait]
    impl RelayMembershipRepository for FailingMembershipRepository {
        async fn upsert_membership(
            &self,
            _record: RelayMembershipRecord,
        ) -> Result<(), StorageError> {
            if self.fail_upsert {
                return Err(StorageError::Internal {
                    message: "upsert failed".to_string(),
                });
            }
            Ok(())
        }

        async fn membership_by_pubkey(
            &self,
            _tenant_id: &str,
            _pubkey: &[u8],
        ) -> Result<Option<RelayMembershipRecord>, StorageError> {
            if self.fail_lookup {
                return Err(StorageError::Internal {
                    message: "lookup failed".to_string(),
                });
            }
            Ok(None)
        }

        async fn list_memberships(
            &self,
            _tenant_id: &str,
        ) -> Result<Vec<RelayMembershipRecord>, StorageError> {
            Ok(Vec::new())
        }

        async fn remove_membership(
            &self,
            _tenant_id: &str,
            _pubkey: &[u8],
        ) -> Result<bool, StorageError> {
            Ok(false)
        }

        async fn insert_invite(&self, _record: RelayInviteRecord) -> Result<(), StorageError> {
            Ok(())
        }

        async fn invite_by_code(
            &self,
            _tenant_id: &str,
            _invite_code: &str,
        ) -> Result<Option<RelayInviteRecord>, StorageError> {
            Ok(None)
        }

        async fn delete_invite(
            &self,
            _tenant_id: &str,
            _invite_code: &str,
        ) -> Result<(), StorageError> {
            Ok(())
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
        tokio::spawn(axum::serve(listener, app).into_future());
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
        event.id = event.compute_id();
        let id_bytes = hex::decode(&event.id).expect("id bytes");
        let msg = secp256k1::Message::from_digest_slice(&id_bytes).expect("msg");
        let sig = secp.sign_schnorr(&msg, &keypair);
        event.sig = hex::encode(sig.as_ref());
        event
    }

    fn websocket_upgrade_request() -> Request<Body> {
        Request::builder()
            .method("GET")
            .uri("/")
            .header("connection", "upgrade")
            .header("upgrade", "websocket")
            .header("sec-websocket-version", "13")
            .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
            .body(Body::empty())
            .expect("upgrade request")
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

    fn no_op_socket_pump<'a>(
        _receiver: &'a mut super::SocketReceiver,
        _sender: &'a mut super::SocketSender,
        _in_tx: &'a mpsc::Sender<String>,
        _out_rx: &'a mut mpsc::Receiver<String>,
    ) -> super::SocketPumpFuture<'a> {
        Box::pin(async {})
    }

    struct ScriptedSink {
        fail_send: bool,
        sent: Vec<Message>,
    }

    impl futures_util::Sink<Message> for ScriptedSink {
        type Error = std::io::Error;

        fn poll_ready(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
        ) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(Ok(()))
        }

        fn start_send(self: Pin<&mut Self>, item: Message) -> Result<(), Self::Error> {
            if self.fail_send {
                return Err(std::io::Error::other("sink send failed"));
            }
            self.get_mut().sent.push(item);
            Ok(())
        }

        fn poll_flush(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
        ) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(Ok(()))
        }

        fn poll_close(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
        ) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(Ok(()))
        }
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
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
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
        socket.send(WsMessage::Text(payload)).await.expect("send");

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
        tokio::spawn(axum::serve(listener, app).into_future());

        let url = format!("ws://{addr}/");
        let err = connect_async(url)
            .await
            .expect_err("expected handshake failure");
        match err {
            tokio_tungstenite::tungstenite::Error::Http(response) => {
                assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
            }
            other => panic!("expected http handshake error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn websocket_upgrade_without_explicit_host_returns_not_found() {
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
            .oneshot(websocket_upgrade_request())
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn websocket_upgrade_returns_not_found_for_unknown_tenant() {
        let repos = Arc::new(FakeTenantRepository {
            tenants_by_host: HashMap::new(),
        });
        let state = Arc::new(super::RelayState {
            config: sample_config(),
            policy: Policy::default(),
            store: Arc::new(MemoryStore::new()),
            repos: Some(repos),
            admission: None,
            broadcast: tokio::sync::broadcast::channel(8).0,
            metrics: Arc::new(RelayMetrics::new()),
        });
        let app = build_router(state);
        let mut request = websocket_upgrade_request();
        request
            .headers_mut()
            .insert("host", "unknown.local".parse().expect("host"));
        let response = app.oneshot(request).await.expect("response");
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn websocket_ignores_binary_messages() {
        let addr = spawn_ws_server().await;
        let url = format!("ws://{addr}/");
        let (mut socket, _) = connect_async(url).await.expect("connect");
        socket
            .send(WsMessage::Binary(vec![0, 1, 2].into()))
            .await
            .expect("send binary");
        socket
            .send(WsMessage::Text(r#"["REQ","sub",{}]"#.to_string()))
            .await
            .expect("send req");

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
        tokio::spawn(axum::serve(listener, app).into_future());

        let url = format!("ws://{addr}/");
        let (mut socket, _) = connect_async(url).await.expect("connect");
        socket
            .send(WsMessage::Text(r#"["REQ","sub",{}]"#.to_string()))
            .await
            .expect("send req");
        socket.send(WsMessage::Close(None)).await.expect("close");
    }

    #[tokio::test]
    async fn handle_socket_with_injected_pump_returns_after_upgrade() {
        let state = Arc::new(super::RelayState {
            config: sample_config(),
            policy: Policy::default(),
            store: Arc::new(MemoryStore::new()),
            repos: None,
            admission: None,
            broadcast: tokio::sync::broadcast::channel(8).0,
            metrics: Arc::new(RelayMetrics::new()),
        });
        let tenant_context = super::TenantContext {
            tenant_id: "default".to_string(),
            store: Arc::new(MemoryStore::new()),
            tenant: None,
        };
        let (done_tx, mut done_rx) = tokio::sync::broadcast::channel(1);
        let app = Router::new().route(
            "/",
            get(move |ws: WebSocketUpgrade| {
                let state = state.clone();
                let tenant = tenant_context.clone();
                let done_tx = done_tx.clone();
                async move {
                    ws.on_upgrade(move |socket| async move {
                        handle_socket_with_pump(socket, state, tenant, no_op_socket_pump).await;
                        let _ = done_tx.send(());
                    })
                }
            }),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(axum::serve(listener, app).into_future());

        let url = format!("ws://{addr}/");
        let (_socket, _) = connect_async(url).await.expect("connect");
        timeout(Duration::from_secs(2), done_rx.recv())
            .await
            .expect("completion timeout")
            .expect("completion signal");
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

    #[tokio::test]
    async fn nip11_response_body_matches_relay_info_document_contract() {
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
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body bytes");
        let json = std::str::from_utf8(&bytes).expect("utf8 body");
        let doc = gittree_core::nip11::RelayInfoDocument::from_json_str(json).expect("nip11");

        assert!(doc.supports_nip(11));
        assert!(doc.software.is_some());
        assert!(doc.version.is_some());
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
            "text/html,application/nostr+json;q=0.9"
                .parse()
                .expect("accept"),
        );
        assert!(accepts_nostr_json(&headers));
    }

    #[test]
    fn accepts_nostr_json_rejects_missing_invalid_and_unrelated_accept_headers() {
        let missing = axum::http::HeaderMap::new();
        assert!(!accepts_nostr_json(&missing));

        let mut invalid = axum::http::HeaderMap::new();
        invalid.insert(
            ACCEPT,
            axum::http::HeaderValue::from_bytes(&[0xFF]).expect("invalid header bytes"),
        );
        assert!(!accepts_nostr_json(&invalid));

        let mut unrelated = axum::http::HeaderMap::new();
        unrelated.insert(ACCEPT, "application/json".parse().expect("accept"));
        assert!(!accepts_nostr_json(&unrelated));
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
        assert_eq!(
            relay_url_from_host("wss://relay.local"),
            "wss://relay.local"
        );
    }

    #[test]
    fn classify_inbound_message_covers_all_branches() {
        assert!(matches!(
            super::classify_inbound_message(None),
            super::InboundSocketAction::Break
        ));
        assert!(matches!(
            super::classify_inbound_message(Some(Ok(Message::Close(None)))),
            super::InboundSocketAction::Break
        ));
        assert!(matches!(
            super::classify_inbound_message(Some(Ok(Message::Binary(vec![0, 1].into())))),
            super::InboundSocketAction::Continue
        ));

        let forwarded = super::classify_inbound_message(Some(Ok(Message::Text("req".into()))));
        assert!(matches!(
            forwarded,
            super::InboundSocketAction::Forward(ref text) if text == "req"
        ));

        let err = axum::Error::new(std::io::Error::other("ws read failed"));
        assert!(matches!(
            super::classify_inbound_message(Some(Err(err))),
            super::InboundSocketAction::Break
        ));
    }

    #[test]
    fn classify_outbound_message_maps_text_and_none() {
        assert!(matches!(super::classify_outbound_message(None), None));

        let mapped = super::classify_outbound_message(Some("frame".to_string())).expect("message");
        assert!(matches!(mapped, Message::Text(ref payload) if payload == "frame"));
    }

    #[tokio::test]
    async fn handle_inbound_action_covers_forward_continue_break_and_send_failure() {
        let (in_tx, mut in_rx) = mpsc::channel(1);
        assert!(
            super::handle_inbound_action(
                &in_tx,
                super::InboundSocketAction::Forward("req".to_string()),
            )
            .await
        );
        assert_eq!(in_rx.recv().await.as_deref(), Some("req"));

        assert!(super::handle_inbound_action(&in_tx, super::InboundSocketAction::Continue,).await);
        assert!(!super::handle_inbound_action(&in_tx, super::InboundSocketAction::Break).await);

        drop(in_rx);
        assert!(
            !super::handle_inbound_action(
                &in_tx,
                super::InboundSocketAction::Forward("dropped".to_string()),
            )
            .await
        );
    }

    #[tokio::test]
    async fn handle_outbound_message_covers_none_send_and_send_failure_paths() {
        let mut ok_sink = ScriptedSink {
            fail_send: false,
            sent: Vec::new(),
        };
        assert!(!super::handle_outbound_message(&mut ok_sink, None).await);
        assert!(super::handle_outbound_message(&mut ok_sink, Some("frame".to_string())).await);
        assert!(matches!(ok_sink.sent.first(), Some(Message::Text(payload)) if payload == "frame"));
        ok_sink.close().await.expect("close sink");

        let mut fail_sink = ScriptedSink {
            fail_send: true,
            sent: Vec::new(),
        };
        assert!(!super::handle_outbound_message(&mut fail_sink, Some("frame".to_string())).await);
    }

    #[tokio::test]
    async fn pump_socket_io_processes_forwarded_frame_then_exits_on_close() {
        let mut receiver = futures_util::stream::iter(vec![
            Ok::<Message, axum::Error>(Message::Text("req".to_string())),
            Ok::<Message, axum::Error>(Message::Close(None)),
        ]);
        let mut sink = ScriptedSink {
            fail_send: false,
            sent: Vec::new(),
        };
        let (in_tx, mut in_rx) = mpsc::channel(2);
        let (_out_tx, mut out_rx) = mpsc::channel(1);

        super::pump_socket_io(&mut receiver, &mut sink, &in_tx, &mut out_rx).await;
        assert_eq!(in_rx.recv().await.as_deref(), Some("req"));
    }

    #[tokio::test]
    async fn postgres_tenant_repository_impl_paths_are_exercised() {
        let repos = unreachable_repos();
        let _store = super::TenantRepository::tenant_store(&*repos, "tenant-a");
        let membership = super::TenantRepository::membership_repository(&*repos);
        let err = super::TenantRepository::tenant_by_host(&*repos, "tenant.local")
            .await
            .expect_err("expected db lookup error");
        assert!(!err.to_string().is_empty());
        drop(membership);
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
        let result = resolve_tenant(&state, &headers).await;
        assert!(result.is_err());
        let response = result.err().expect("missing host should error");
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
        let result = resolve_tenant(&state, &headers).await;
        assert!(result.is_err());
        let response = result
            .err()
            .expect("storage failure should map to http 500");
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[tokio::test]
    async fn resolve_tenant_returns_not_found_when_host_has_no_match() {
        let repos = Arc::new(FakeTenantRepository {
            tenants_by_host: HashMap::new(),
        });
        let state = super::RelayState {
            config: sample_config(),
            policy: Policy::default(),
            store: Arc::new(MemoryStore::new()),
            repos: Some(repos),
            admission: None,
            broadcast: tokio::sync::broadcast::channel(8).0,
            metrics: Arc::new(RelayMetrics::new()),
        };

        let mut headers = axum::http::HeaderMap::new();
        headers.insert("host", "tenant.local".parse().expect("host"));
        let result = resolve_tenant(&state, &headers).await;
        assert!(result.is_err());
        let response = result.err().expect("unknown host should map to http 404");
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn resolve_tenant_returns_tenant_context_when_found() {
        let tenant = sample_tenant_record();
        let repos = Arc::new(FakeTenantRepository::with_tenant(
            "tenant.local",
            tenant.clone(),
        ));
        let state = super::RelayState {
            config: sample_config(),
            policy: Policy::default(),
            store: Arc::new(MemoryStore::new()),
            repos: Some(repos),
            admission: None,
            broadcast: tokio::sync::broadcast::channel(8).0,
            metrics: Arc::new(RelayMetrics::new()),
        };

        let mut headers = axum::http::HeaderMap::new();
        headers.insert("host", "TENANT.LOCAL:443".parse().expect("host"));
        let context = resolve_tenant(&state, &headers)
            .await
            .expect("tenant context");
        assert_eq!(context.tenant_id, tenant.id);
        assert!(context.tenant.is_some());
    }

    #[tokio::test]
    async fn fake_tenant_repository_membership_repository_paths_are_exercised() {
        let repos = FakeTenantRepository::with_tenant("tenant.local", sample_tenant_record());
        let membership = super::TenantRepository::membership_repository(&repos);
        let members = membership
            .list_memberships("tenant-id")
            .await
            .expect("list memberships");
        assert!(members.is_empty());
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
    async fn seed_owner_membership_promotes_existing_member_to_owner_active() {
        let repo = Arc::new(InMemoryRepositories::new());
        let membership: Arc<dyn RelayMembershipRepository> = repo.clone();
        let tenant_id = "tenant-test";
        let relay_pubkey = vec![0x22; 32];
        repo.upsert_membership(RelayMembershipRecord {
            tenant_id: tenant_id.to_string(),
            pubkey: relay_pubkey.clone(),
            role: "member".to_string(),
            status: "left".to_string(),
            created_at: 5,
            updated_at: 6,
        })
        .await
        .expect("seed existing");

        seed_owner_membership(&membership, tenant_id, &relay_pubkey)
            .await
            .expect("promote to owner");
        let record = repo
            .membership_by_pubkey(tenant_id, &relay_pubkey)
            .await
            .expect("lookup")
            .expect("membership");
        assert_eq!(record.role, "owner");
        assert_eq!(record.status, "active");
        assert_eq!(record.created_at, 5);
        assert!(record.updated_at >= 6);
    }

    #[tokio::test]
    async fn seed_owner_membership_surfaces_repository_errors() {
        let lookup_repo: Arc<dyn RelayMembershipRepository> =
            Arc::new(FailingMembershipRepository::lookup());
        let lookup_err = seed_owner_membership(&lookup_repo, "tenant", &[0x33; 32])
            .await
            .expect_err("lookup error");
        assert!(lookup_err.to_string().contains("lookup failed"));

        let upsert_repo: Arc<dyn RelayMembershipRepository> =
            Arc::new(FailingMembershipRepository::upsert());
        let upsert_err = seed_owner_membership(&upsert_repo, "tenant", &[0x33; 32])
            .await
            .expect_err("upsert error");
        assert!(upsert_err.to_string().contains("upsert failed"));
    }

    #[tokio::test]
    async fn failing_membership_repository_passthrough_methods_return_defaults() {
        let repo = FailingMembershipRepository {
            fail_lookup: false,
            fail_upsert: false,
        };

        let listed = repo.list_memberships("tenant").await.expect("list");
        assert!(listed.is_empty());

        let removed = repo
            .remove_membership("tenant", &[0x11; 32])
            .await
            .expect("remove");
        assert!(!removed);

        repo.insert_invite(RelayInviteRecord {
            tenant_id: "tenant".to_string(),
            invite_code: "invite-code".to_string(),
            role: "member".to_string(),
            inviter_pubkey: vec![0x22; 32],
            invitee_pubkey: None,
            expires_at: None,
            created_at: 1,
        })
        .await
        .expect("insert invite");

        let invite = repo
            .invite_by_code("tenant", "invite-code")
            .await
            .expect("invite lookup");
        assert!(invite.is_none());

        repo.delete_invite("tenant", "invite-code")
            .await
            .expect("delete invite");
    }

    #[tokio::test]
    async fn shutdown_signal_future_can_start_and_be_aborted() {
        let task = tokio::spawn(super::await_shutdown_signal(std::future::pending::<
            Result<(), std::io::Error>,
        >()));
        tokio::time::sleep(Duration::from_millis(10)).await;
        task.abort();
    }

    #[tokio::test]
    async fn await_shutdown_signal_handles_ready_results() {
        super::await_shutdown_signal(async { Ok(()) }).await;
        super::await_shutdown_signal(async {
            Err(std::io::Error::other("ignored in shutdown helper"))
        })
        .await;
    }

    #[tokio::test]
    async fn bind_listener_accepts_ephemeral_bind_address() {
        let listener = bind_listener("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("local addr");
        assert!(addr.port() > 0);
    }

    #[tokio::test]
    async fn bind_listener_returns_serve_error_for_invalid_bind_address() {
        let mut config = sample_config();
        config.bind = "127.0.0.1:99999".to_string();
        let err = bind_listener(&config.bind).await.expect_err("invalid bind");
        assert!(matches!(err, RelayError::Serve(_)));
    }

    #[tokio::test]
    async fn serve_inner_returns_serve_error_for_invalid_bind_address() {
        let mut config = sample_config();
        config.bind = "127.0.0.1:99999".to_string();
        let err = super::serve_inner(config).await.expect_err("invalid bind");
        assert!(matches!(err, RelayError::Serve(_)));
    }

    #[tokio::test]
    async fn serve_returns_serve_error_for_invalid_bind_address() {
        let mut config = sample_config();
        config.bind = "127.0.0.1:99999".to_string();
        let err = super::serve(config).await.expect_err("invalid bind");
        assert!(matches!(err, RelayError::Serve(_)));
    }

    #[tokio::test]
    async fn serve_inner_with_shutdown_returns_ok_for_ephemeral_bind() {
        let mut config = sample_config();
        config.bind = "127.0.0.1:0".to_string();
        super::serve_inner_with_shutdown(config, async {})
            .await
            .expect("serve");
    }

    #[tokio::test]
    async fn run_server_returns_ok_when_future_is_ok() {
        super::run_server::<std::io::Error, _>(async { Ok(()) })
            .await
            .expect("ok");
    }

    #[tokio::test]
    async fn run_server_accepts_axum_server_with_graceful_shutdown_type() {
        let state = build_state(sample_config()).expect("state");
        let app = build_router(Arc::new(state));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener");
        let server = axum::serve(listener, app).with_graceful_shutdown(async {});
        super::run_server(server).await.expect("graceful shutdown");
    }

    #[tokio::test]
    async fn run_server_polls_axum_server_with_shutdown_signal_type() {
        let state = build_state(sample_config()).expect("state");
        let app = build_router(Arc::new(state));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener");
        let server = axum::serve(listener, app)
            .with_graceful_shutdown(async { std::future::pending::<()>().await });
        let result =
            tokio::time::timeout(Duration::from_millis(20), super::run_server(server)).await;
        assert!(
            result.is_err(),
            "run_server should still be pending without a shutdown signal"
        );
    }

    #[tokio::test]
    async fn run_server_maps_error_to_serve() {
        let err = super::run_server(async { Err(std::io::Error::other("boom")) })
            .await
            .expect_err("error");
        assert!(matches!(err, RelayError::Serve(message) if message.contains("boom")));
    }

    #[tokio::test]
    async fn build_state_returns_storage_error_for_invalid_pool_settings() {
        let mut config = sample_config();
        config.storage.max_connections = 0;
        let result = build_state(config);
        assert!(result.is_err());
        let err = result.err().expect("expected storage error");
        assert!(matches!(err, RelayError::Storage(_)));
    }

    #[tokio::test]
    async fn build_state_with_admission_config_constructs_client() {
        let mut config = sample_config();
        config.admission = Some(crate::AdmissionHookConfig::new(
            "http://127.0.0.1:8081/decide".to_string(),
            Duration::from_secs(2),
            crate::AdmissionFallback::Reject,
        ));
        let state = build_state(config).expect("state");
        assert!(state.admission.is_some());
    }

    #[tokio::test]
    async fn build_state_returns_storage_error_for_invalid_write_connection() {
        let mut config = sample_config();
        config.storage.write_connection = Some("not-a-valid-url".to_string());
        let result = build_state(config);
        assert!(result.is_err());
        let err = result.err().expect("expected invalid write url");
        assert!(matches!(err, RelayError::Storage(_)));
    }
}
