use axum::extract::Path;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use gittree_config::ForgejoConfig;
use gittree_coordinator::{CoordinatorConfig, HookInstallConfig, serve};
use gittree_storage::StorageConfig;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::task::JoinHandle;

const TEST_NPUB: &str = "npub1gjttreegkzys8jlhdnfm3qe39h2gka79cpndd0jsms5fk7tuhcnsdw56jq";

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

fn runtime_config(port: u16, root: &PathBuf, forgejo_base_url: &str) -> CoordinatorConfig {
    let repo_root = root.join("repos");
    std::fs::create_dir_all(&repo_root).expect("repo root");
    let pre_hook = write_hook(root, "pre-receive");
    let post_hook = write_hook(root, "post-receive");

    CoordinatorConfig {
        bind: format!("127.0.0.1:{port}"),
        storage: StorageConfig {
            read_connection: "postgres://gittree:gittree@127.0.0.1:5432/gittree".to_string(),
            write_connection: None,
            max_connections: 4,
            min_connections: 1,
            idle_timeout_secs: Some(5),
            max_lifetime_secs: Some(60),
            application_name: Some("gittree-coordinator-runtime-test".to_string()),
        },
        relay_urls: vec!["wss://relay.example".to_string()],
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

async fn wait_for_health(base_url: &str) {
    for _ in 0..60 {
        if http_status(base_url, "/health").await == Some(200) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("coordinator server never became ready");
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

async fn start_runtime_server() -> (
    JoinHandle<Result<(), gittree_coordinator::CoordinatorError>>,
    String,
    PathBuf,
) {
    let port = reserve_local_port();
    let temp_dir = unique_temp_dir("gittree-coordinator-postgres-runtime");
    std::fs::create_dir_all(&temp_dir).expect("temp dir");
    let (forgejo_handle, forgejo_base_url) = start_mock_forgejo_server().await;
    let config = runtime_config(port, &temp_dir, &forgejo_base_url);
    let base_url = format!("http://127.0.0.1:{port}");
    let handle = tokio::spawn(async move {
        let result = serve(config).await;
        forgejo_handle.abort();
        let _ = forgejo_handle.await;
        result
    });
    wait_for_health(&base_url).await;
    (handle, base_url, temp_dir)
}

async fn stop_runtime_server(
    handle: JoinHandle<Result<(), gittree_coordinator::CoordinatorError>>,
    temp_dir: PathBuf,
) {
    handle.abort();
    let _ = handle.await;
    let _ = std::fs::remove_dir_all(temp_dir);
}

#[tokio::test]
async fn coordinator_serve_handles_postgres_announcement_runtime_paths() {
    let (handle, base_url, temp_dir) = start_runtime_server().await;
    let invalid_event_id = unique_hex_64();
    let valid_event_id = unique_hex_64();

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
        http_post_response(&base_url, "/announcement", &invalid_payload).await;
    assert_eq!(invalid_response, Some(400), "{invalid_body}");

    let payload = format!(
        r#"{{
          "kind":30617,
          "event_id":"{valid_event_id}",
          "pubkey":"1111111111111111111111111111111111111111111111111111111111111111",
          "created_at":10,
          "tags":[
            ["d","repo"],
            ["clone","https://gittr.ee/{TEST_NPUB}/repo.git"],
            ["relays","wss://relay.example"]
          ]
        }}"#
    );
    let (response, body) = http_post_response(&base_url, "/announcement", &payload).await;
    assert_eq!(response, Some(200), "{body}");

    stop_runtime_server(handle, temp_dir).await;
}
