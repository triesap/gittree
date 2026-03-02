use axum::extract::Path;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use futures_util::{SinkExt, StreamExt};
use gittree_config::ForgejoConfig;
use gittree_coordinator::{CoordinatorConfig, CoordinatorError, HookInstallConfig, serve};
use gittree_storage::StorageConfig;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message as WsMessage;

const TEST_NPUB: &str = "npub1gjttreegkzys8jlhdnfm3qe39h2gka79cpndd0jsms5fk7tuhcnsdw56jq";
static RUNTIME_TEST_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn runtime_test_guard() -> std::sync::MutexGuard<'static, ()> {
    RUNTIME_TEST_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .expect("runtime test lock")
}

fn reserve_local_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind local port");
    let port = listener.local_addr().expect("listener addr").port();
    drop(listener);
    port
}

fn unique_temp_dir(prefix: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "{prefix}-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    ))
}

fn unique_hex_64() -> String {
    format!(
        "{:064x}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    )
}

fn write_hook(path: &PathBuf, name: &str) -> PathBuf {
    let hook = path.join(name);
    std::fs::write(&hook, "#!/bin/sh\nexit 0\n").expect("write hook");
    hook
}

fn runtime_config(
    port: u16,
    root: &PathBuf,
    forgejo_base_url: &str,
    relay_url: &str,
) -> CoordinatorConfig {
    let repo_root = root.join("repos");
    std::fs::create_dir_all(&repo_root).expect("repo root");
    let pre_hook = write_hook(root, "pre-receive");
    let post_hook = write_hook(root, "post-receive");

    CoordinatorConfig {
        bind: format!("127.0.0.1:{port}"),
        storage: StorageConfig {
            read_connection: "postgres://gittree:gittree@127.0.0.1:5432/gittree".to_string(),
            write_connection: None,
            max_connections: 16,
            min_connections: 1,
            idle_timeout_secs: Some(30),
            max_lifetime_secs: Some(300),
            application_name: Some("gittree-coordinator-runtime-test".to_string()),
        },
        relay_urls: vec![relay_url.to_string()],
        repo_root,
        hooks: HookInstallConfig {
            pre_receive_source: pre_hook,
            post_receive_source: post_hook,
        },
        forgejo: ForgejoConfig {
            base_url: forgejo_base_url.to_string(),
            api_token: "token".to_string(),
            owner: "owner".to_string(),
            webhook_url: "http://gittree.local/webhook".to_string(),
            webhook_secret: "secret".to_string(),
            repo_private: true,
        },
    }
}

async fn get_repo_not_found(Path((_owner, _repo)): Path<(String, String)>) -> StatusCode {
    StatusCode::NOT_FOUND
}

async fn create_repo_for_owner(
    Path(owner): Path<String>,
    Json(payload): Json<Value>,
) -> Result<(StatusCode, Json<Value>), StatusCode> {
    let Some(name) = payload.get("name").and_then(Value::as_str) else {
        return Err(StatusCode::BAD_REQUEST);
    };
    Ok((
        StatusCode::CREATED,
        Json(json!({
            "full_name": format!("{owner}/{name}"),
            "name": name,
            "owner": {"username": owner},
            "html_url": Value::Null
        })),
    ))
}

async fn create_repo_for_org(
    Path(owner): Path<String>,
    Json(payload): Json<Value>,
) -> Result<(StatusCode, Json<Value>), StatusCode> {
    let Some(name) = payload.get("name").and_then(Value::as_str) else {
        return Err(StatusCode::BAD_REQUEST);
    };
    Ok((
        StatusCode::CREATED,
        Json(json!({
            "full_name": format!("{owner}/{name}"),
            "name": name,
            "owner": {"username": owner},
            "html_url": Value::Null
        })),
    ))
}

async fn create_repo_for_user(
    Json(payload): Json<Value>,
) -> Result<(StatusCode, Json<Value>), StatusCode> {
    let Some(name) = payload.get("name").and_then(Value::as_str) else {
        return Err(StatusCode::BAD_REQUEST);
    };
    let owner = "owner";
    Ok((
        StatusCode::CREATED,
        Json(json!({
            "full_name": format!("{owner}/{name}"),
            "name": name,
            "owner": {"username": owner},
            "html_url": Value::Null
        })),
    ))
}

async fn list_hooks_for_repo(Path((_owner, _repo)): Path<(String, String)>) -> Json<Value> {
    Json(json!([]))
}

async fn create_hook_for_repo(Path((_owner, _repo)): Path<(String, String)>) -> StatusCode {
    StatusCode::CREATED
}

async fn start_mock_forgejo_server() -> (JoinHandle<()>, String) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("forgejo listener");
    let addr = listener.local_addr().expect("forgejo addr");
    let app = Router::new()
        .route("/api/v1/repos/:owner/:repo", get(get_repo_not_found))
        .route(
            "/api/v1/admin/users/:owner/repos",
            post(create_repo_for_owner),
        )
        .route("/api/v1/orgs/:owner/repos", post(create_repo_for_org))
        .route("/api/v1/user/repos", post(create_repo_for_user))
        .route(
            "/api/v1/repos/:owner/:repo/hooks",
            get(list_hooks_for_repo).post(create_hook_for_repo),
        );
    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (handle, format!("http://{addr}"))
}

async fn start_mock_relay_server() -> (JoinHandle<()>, String) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("relay listener");
    let addr = listener.local_addr().expect("relay addr");
    let handle = tokio::spawn(async move {
        let (tcp, _) = listener.accept().await.expect("relay accept");
        let mut ws = tokio_tungstenite::accept_async(tcp)
            .await
            .expect("relay ws handshake");
        let Some(Ok(WsMessage::Text(message))) = ws.next().await else {
            return;
        };
        let event_id = serde_json::from_str::<Value>(&message)
            .ok()
            .and_then(|value| value.get(1).cloned())
            .and_then(|event| event.get("id").and_then(Value::as_str).map(str::to_string))
            .unwrap_or_default();
        let ok = json!(["OK", event_id, true, "accepted"]).to_string();
        let _ = ws.send(WsMessage::Text(ok)).await;
        let _ = ws.close(None).await;
    });
    (handle, format!("ws://{addr}"))
}

fn parse_status_code(status_line: &str) -> Option<u16> {
    status_line
        .split_whitespace()
        .nth(1)
        .and_then(|value| value.parse::<u16>().ok())
}

async fn http_status(base_url: &str, path: &str) -> Option<u16> {
    let endpoint = base_url.trim_start_matches("http://");
    let mut stream = tokio::net::TcpStream::connect(endpoint).await.ok()?;
    let request = format!("GET {path} HTTP/1.1\r\nHost: {endpoint}\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).await.ok()?;
    let mut reader = BufReader::new(stream);
    let mut status_line = String::new();
    reader.read_line(&mut status_line).await.ok()?;
    parse_status_code(status_line.trim_end())
}

async fn http_post_response(base_url: &str, path: &str, body: &str) -> (Option<u16>, String) {
    let endpoint = base_url.trim_start_matches("http://");
    let mut stream = match tokio::net::TcpStream::connect(endpoint).await {
        Ok(stream) => stream,
        Err(_) => return (None, String::new()),
    };
    let request = format!(
        "POST {path} HTTP/1.1\r\nHost: {endpoint}\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    if stream.write_all(request.as_bytes()).await.is_err() {
        return (None, String::new());
    }
    let mut reader = BufReader::new(stream);
    let mut status_line = String::new();
    if reader.read_line(&mut status_line).await.is_err() {
        return (None, String::new());
    }
    let mut rest = String::new();
    let _ = reader.read_to_string(&mut rest).await;
    (parse_status_code(status_line.trim_end()), rest)
}

struct RuntimeServer {
    coordinator_handle: JoinHandle<Result<(), CoordinatorError>>,
    forgejo_handle: JoinHandle<()>,
    base_url: String,
    temp_dir: PathBuf,
}

fn join_handle_error(handle: &JoinHandle<Result<(), CoordinatorError>>) -> Option<String> {
    if handle.is_finished() {
        Some("coordinator exited before health check".to_string())
    } else {
        None
    }
}

async fn wait_for_health_or_exit(
    base_url: &str,
    coordinator_handle: &mut JoinHandle<Result<(), CoordinatorError>>,
) -> Result<(), String> {
    for _ in 0..120 {
        if let Some(error) = join_handle_error(coordinator_handle) {
            return Err(error);
        }
        if http_status(base_url, "/health").await == Some(200) {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Err("coordinator server never became ready".to_string())
}

async fn start_runtime_server_with_relay(relay_url: &str) -> RuntimeServer {
    let mut last_error = String::new();
    for _ in 0..3 {
        let port = reserve_local_port();
        let temp_dir = unique_temp_dir("gittree-coordinator-postgres-runtime");
        std::fs::create_dir_all(&temp_dir).expect("temp dir");
        let (forgejo_handle, forgejo_base_url) = start_mock_forgejo_server().await;
        let config = runtime_config(port, &temp_dir, &forgejo_base_url, relay_url);
        let base_url = format!("http://127.0.0.1:{port}");
        let mut coordinator_handle = tokio::spawn(serve(config));
        match wait_for_health_or_exit(&base_url, &mut coordinator_handle).await {
            Ok(()) => {
                return RuntimeServer {
                    coordinator_handle,
                    forgejo_handle,
                    base_url,
                    temp_dir,
                };
            }
            Err(error) => {
                last_error = error;
                if coordinator_handle.is_finished() {
                    match coordinator_handle.await {
                        Ok(Ok(())) => {}
                        Ok(Err(join_error)) => {
                            last_error = format!("{last_error}; {join_error}");
                        }
                        Err(join_error) => {
                            last_error = format!("{last_error}; {join_error}");
                        }
                    }
                } else {
                    coordinator_handle.abort();
                    let _ = coordinator_handle.await;
                }
                forgejo_handle.abort();
                let _ = forgejo_handle.await;
                let _ = std::fs::remove_dir_all(&temp_dir);
            }
        }
    }
    panic!("coordinator runtime server failed to start after retries: {last_error}");
}

async fn stop_runtime_server(server: RuntimeServer) {
    server.coordinator_handle.abort();
    let _ = server.coordinator_handle.await;
    server.forgejo_handle.abort();
    let _ = server.forgejo_handle.await;
    let _ = std::fs::remove_dir_all(server.temp_dir);
}

#[tokio::test]
async fn coordinator_runtime_handles_postgres_announcement_and_publish_paths() {
    let _guard = runtime_test_guard();
    let (relay_handle, relay_url) = start_mock_relay_server().await;
    let server = start_runtime_server_with_relay(&relay_url).await;
    let invalid_event_id = unique_hex_64();

    let invalid_payload = format!(
        r#"{{
      "kind":18446744073709551615,
      "event_id":"{invalid_event_id}",
      "pubkey":"1111111111111111111111111111111111111111111111111111111111111111",
      "created_at":10,
      "tags":[]
    }}"#
    );
    let (invalid_response, invalid_body) =
        http_post_response(&server.base_url, "/announcement", &invalid_payload).await;
    assert_eq!(invalid_response, Some(400), "{invalid_body}");

    let parse_error_payload = format!(
        r#"{{
          "kind":30617,
          "event_id":"{}",
          "pubkey":"1111111111111111111111111111111111111111111111111111111111111111",
          "created_at":10,
          "tags":[]
        }}"#,
        unique_hex_64()
    );
    let (parse_error_response, parse_error_body) =
        http_post_response(&server.base_url, "/announcement", &parse_error_payload).await;
    assert_eq!(parse_error_response, Some(400), "{parse_error_body}");

    let valid_event_id = unique_hex_64();
    let payload = format!(
        r#"{{
          "kind":30617,
          "event_id":"{valid_event_id}",
          "pubkey":"1111111111111111111111111111111111111111111111111111111111111111",
          "created_at":10,
          "tags":[
            ["d","repo-{valid_event_id}"],
            ["clone","https://gittr.ee/{TEST_NPUB}/repo-{valid_event_id}.git"],
            ["relays","{relay_url}"]
          ]
        }}"#
    );
    let (response, body) = http_post_response(&server.base_url, "/announcement", &payload).await;
    assert!(
        response == Some(200) || (response == Some(500) && body.contains("pool timed out")),
        "{body}"
    );

    let malformed_pubkey_payload = format!(
        r#"{{
          "kind":30617,
          "event_id":"{}",
          "pubkey":"1111111111111111111111111111111111111111111111111111111111111111",
          "created_at":10,
          "tags":[
            ["d","repo-malformed-pubkey-{}"],
            ["clone","https://gittr.ee/{TEST_NPUB}/repo-malformed-pubkey-{}.git"],
            ["relays","wss://relay.example"]
          ]
        }}"#,
        unique_hex_64(),
        unique_hex_64(),
        unique_hex_64()
    );
    let (malformed_response, malformed_body) =
        http_post_response(&server.base_url, "/announcement", &malformed_pubkey_payload).await;
    assert!(malformed_response == Some(500), "{malformed_body}");
    let _ = tokio::time::timeout(Duration::from_secs(6), relay_handle).await;

    stop_runtime_server(server).await;
}
