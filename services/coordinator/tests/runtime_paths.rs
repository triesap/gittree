use axum::extract::Path;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use gittree_config::ForgejoConfig;
use gittree_coordinator::{
    CoordinatorConfig, CoordinatorConfigError, CoordinatorError, HookInstallConfig,
    StorageConfigError, serve,
};
use gittree_storage::StorageConfig;
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::Mutex as AsyncMutex;

fn reserve_local_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind local port");
    let port = listener.local_addr().expect("listener addr").port();
    drop(listener);
    port
}

fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn async_test_lock() -> &'static AsyncMutex<()> {
    static LOCK: OnceLock<AsyncMutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| AsyncMutex::new(()))
}

struct EnvRestore {
    vars: Vec<(String, Option<String>)>,
}

impl EnvRestore {
    fn capture(keys: &[&str]) -> Self {
        let vars = keys
            .iter()
            .map(|key| ((*key).to_string(), std::env::var(key).ok()))
            .collect();
        Self { vars }
    }
}

impl Drop for EnvRestore {
    fn drop(&mut self) {
        for (key, value) in &self.vars {
            if let Some(value) = value {
                set_env_var(key, value);
            } else {
                remove_env_var(key);
            }
        }
    }
}

fn set_env_var(key: &str, value: impl AsRef<std::ffi::OsStr>) {
    // SAFETY: tests hold a process-local env mutex and restore values on drop.
    unsafe {
        std::env::set_var(key, value);
    }
}

fn remove_env_var(key: &str) {
    // SAFETY: tests hold a process-local env mutex and restore values on drop.
    unsafe {
        std::env::remove_var(key);
    }
}

fn runtime_storage_url() -> String {
    std::env::var("GITTREE_STORAGE_TEST_DATABASE_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "postgres://gittree:gittree@127.0.0.1:5432/gittree".to_string())
}

fn new_runtime_dir() -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "gittree-coordinator-runtime-{}-{}-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos(),
        std::process::id(),
        counter
    ))
}

fn direct_serve_config(
    bind: &str,
    runtime_dir: &PathBuf,
    forgejo_base_url: &str,
) -> CoordinatorConfig {
    let repo_root = runtime_dir.join("repos");
    std::fs::create_dir_all(&repo_root).expect("create repo root");
    let pre_hook = write_hook_source(runtime_dir, "pre-direct-hook");
    let post_hook = write_hook_source(runtime_dir, "post-direct-hook");

    CoordinatorConfig {
        bind: bind.to_string(),
        storage: StorageConfig {
            read_connection: runtime_storage_url(),
            write_connection: None,
            max_connections: 5,
            min_connections: 1,
            idle_timeout_secs: Some(30),
            max_lifetime_secs: Some(300),
            application_name: Some("gittree-coordinator-runtime-direct".to_string()),
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
            webhook_url: "http://localhost:8080/webhook".to_string(),
            webhook_secret: "secret".to_string(),
            repo_private: true,
        },
    }
}

fn write_hook_source(path: &PathBuf, name: &str) -> PathBuf {
    let hook_path = path.join(name);
    std::fs::write(&hook_path, "#!/bin/sh\nexit 0\n").expect("write hook");
    hook_path
}

fn start_coordinator_server(port: u16) -> (Child, String, PathBuf) {
    start_coordinator_server_with(
        port,
        "http://127.0.0.1:1",
        "wss://relay.example",
    )
}

fn start_coordinator_server_with(
    port: u16,
    forgejo_base_url: &str,
    relay_urls: &str,
) -> (Child, String, PathBuf) {
    let runtime_dir = new_runtime_dir();
    let repo_root = runtime_dir.join("repos");
    std::fs::create_dir_all(&repo_root).expect("create repo root");

    let pre_hook = write_hook_source(&runtime_dir, "pre-receive");
    let post_hook = write_hook_source(&runtime_dir, "post-receive");
    let base_url = format!("http://127.0.0.1:{port}");

    let mut command = Command::new(env!("CARGO_BIN_EXE_gittree-coordinator"));
    command
        .current_dir(&runtime_dir)
        .env("GITTREE_COORDINATOR_BIND", format!("127.0.0.1:{port}"))
        .env("GITTREE_STORAGE_READ_URL", runtime_storage_url())
        .env("GITTREE_STORAGE_MAX_CONNECTIONS", "5")
        .env("GITTREE_STORAGE_MIN_CONNECTIONS", "1")
        .env("GITTREE_STORAGE_IDLE_TIMEOUT_SECS", "30")
        .env("GITTREE_STORAGE_MAX_LIFETIME_SECS", "300")
        .env("GITTREE_COORDINATOR_REPO_ROOT", &repo_root)
        .env("GITTREE_COORDINATOR_PRE_RECEIVE_HOOK", &pre_hook)
        .env("GITTREE_COORDINATOR_POST_RECEIVE_HOOK", &post_hook)
        .env("GITTREE_FORGEJO_BASE_URL", forgejo_base_url)
        .env("GITTREE_FORGEJO_API_TOKEN", "token")
        .env("GITTREE_FORGEJO_OWNER", "owner")
        .env("GITTREE_FORGEJO_WEBHOOK_URL", "http://127.0.0.1:1/webhook")
        .env("GITTREE_FORGEJO_WEBHOOK_SECRET", "secret")
        .env("GITTREE_RELAY_URLS", relay_urls)
        .env("GITTREE_LOG_STDOUT", "false")
        .env("GITTREE_METRICS_ENABLED", "false")
        .env("GITTREE_LOG_DIR", runtime_dir.join("logs"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());

    let child = command.spawn().expect("spawn coordinator");
    (child, base_url, runtime_dir)
}

async fn get_repo_not_found(Path((_owner, _repo)): Path<(String, String)>) -> StatusCode {
    StatusCode::NOT_FOUND
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

async fn start_mock_forgejo_server() -> (tokio::task::JoinHandle<()>, String) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("forgejo bind");
    let addr = listener.local_addr().expect("forgejo addr");
    let app = Router::new()
        .route("/api/v1/repos/:owner/:repo", get(get_repo_not_found))
        .route("/api/v1/orgs/:owner/repos", post(create_repo_for_org))
        .route("/api/v1/user/repos", post(create_repo_for_user))
        .route("/api/v1/admin/users/:owner/repos", post(create_repo_for_org))
        .route(
            "/api/v1/repos/:owner/:repo/hooks",
            get(list_hooks_for_repo).post(create_hook_for_repo),
        );
    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (handle, format!("http://{addr}"))
}

fn parse_status_code(response: &str) -> Option<u16> {
    let status_line = response.lines().next().unwrap_or_default();
    let code = status_line.split_whitespace().nth(1).unwrap_or_default();
    code.parse::<u16>().ok()
}

fn read_status_code(stream: &mut TcpStream) -> Option<u16> {
    let mut reader = BufReader::new(stream);
    let mut status_line = String::new();
    reader.read_line(&mut status_line).ok()?;
    parse_status_code(status_line.trim_end())
}

fn http_status(base_url: &str, path: &str) -> Option<u16> {
    let endpoint = base_url.trim_start_matches("http://");
    let mut stream = TcpStream::connect(endpoint).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(2))).ok()?;
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .ok()?;
    let request = format!("GET {path} HTTP/1.1\r\nHost: {endpoint}\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).ok()?;
    read_status_code(&mut stream)
}

fn http_post_status(base_url: &str, path: &str, body: &str) -> Option<u16> {
    let endpoint = base_url.trim_start_matches("http://");
    let mut stream = TcpStream::connect(endpoint).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(2))).ok()?;
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .ok()?;
    let request = format!(
        "POST {path} HTTP/1.1\r\nHost: {endpoint}\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(request.as_bytes()).ok()?;
    read_status_code(&mut stream)
}

async fn wait_for_health(base_url: &str, child: &mut Child) {
    for _ in 0..60 {
        if http_status(base_url, "/health") == Some(200) {
            return;
        }
        if let Some(status) = child.try_wait().expect("check child status") {
            let mut stderr = String::new();
            if let Some(mut stderr_pipe) = child.stderr.take() {
                let _ = stderr_pipe.read_to_string(&mut stderr);
            }
            panic!("coordinator exited early ({status}): {stderr}");
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("coordinator server never became ready");
}

fn stop_server(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

#[tokio::test]
async fn coordinator_binary_runtime_routes_cover_non_test_monomorphizations() {
    let _guard = async_test_lock().lock().await;
    let port = reserve_local_port();
    let (mut child, base_url, runtime_dir) = start_coordinator_server(port);
    wait_for_health(&base_url, &mut child).await;

    let invalid_kind_payload = r#"{
      "kind":18446744073709551615,
      "event_id":"4444444444444444444444444444444444444444444444444444444444444444",
      "pubkey":"1111111111111111111111111111111111111111111111111111111111111111",
      "created_at":10,
      "tags":[]
    }"#;
    assert_eq!(http_status(&base_url, "/health"), Some(200));
    let announcement_status = http_post_status(&base_url, "/announcement", invalid_kind_payload);
    assert_eq!(announcement_status, Some(400));

    stop_server(&mut child);
    let _ = std::fs::remove_dir_all(runtime_dir);
}

#[tokio::test]
async fn coordinator_binary_runtime_valid_announcement_exercises_postgres_and_outbox_paths() {
    let _guard = async_test_lock().lock().await;
    let (forgejo_handle, forgejo_base_url) = start_mock_forgejo_server().await;
    let port = reserve_local_port();
    let (mut child, base_url, runtime_dir) =
        start_coordinator_server_with(port, &forgejo_base_url, "ws://127.0.0.1:1");
    wait_for_health(&base_url, &mut child).await;

    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    let event_id = format!("{unique:064x}");
    let identifier = format!("repo-{unique}");
    let payload = json!({
        "kind": 30617_u32,
        "event_id": event_id,
        "pubkey": "1111111111111111111111111111111111111111111111111111111111111111",
        "created_at": 10_u64,
        "tags": [
            ["d", identifier.as_str()],
            ["clone", format!("https://gittr.ee/npub1gjttreegkzys8jlhdnfm3qe39h2gka79cpndd0jsms5fk7tuhcnsdw56jq/{identifier}.git")],
            ["relays", "ws://127.0.0.1:1"]
        ]
    });

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .expect("client");
    let response = client
        .post(format!("{base_url}/announcement"))
        .json(&payload)
        .send()
        .await
        .expect("announcement request");
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    assert_eq!(status, reqwest::StatusCode::OK, "announcement body: {body}");

    // Let the outbox worker run at least one poll cycle to execute runtime publish paths.
    tokio::time::sleep(Duration::from_millis(2500)).await;

    stop_server(&mut child);
    let _ = std::fs::remove_dir_all(runtime_dir);
    forgejo_handle.abort();
    let _ = forgejo_handle.await;
}

#[tokio::test]
async fn coordinator_binary_runtime_announcement_parse_error_returns_bad_request() {
    let _guard = async_test_lock().lock().await;
    let (forgejo_handle, forgejo_base_url) = start_mock_forgejo_server().await;
    let port = reserve_local_port();
    let (mut child, base_url, runtime_dir) =
        start_coordinator_server_with(port, &forgejo_base_url, "wss://relay.example");
    wait_for_health(&base_url, &mut child).await;

    let payload = r#"{
      "kind":30617,
      "event_id":"9999999999999999999999999999999999999999999999999999999999999999",
      "pubkey":"1111111111111111111111111111111111111111111111111111111111111111",
      "created_at":10,
      "tags":[]
    }"#;
    let status = http_post_status(&base_url, "/announcement", payload);
    assert_eq!(status, Some(400));

    stop_server(&mut child);
    let _ = std::fs::remove_dir_all(runtime_dir);
    forgejo_handle.abort();
    let _ = forgejo_handle.await;
}

#[tokio::test]
async fn coordinator_binary_runtime_announcement_missing_npub_returns_bad_request() {
    let _guard = async_test_lock().lock().await;
    let (forgejo_handle, forgejo_base_url) = start_mock_forgejo_server().await;
    let port = reserve_local_port();
    let (mut child, base_url, runtime_dir) =
        start_coordinator_server_with(port, &forgejo_base_url, "wss://relay.example");
    wait_for_health(&base_url, &mut child).await;

    let payload = r#"{
      "kind":30617,
      "event_id":"8888888888888888888888888888888888888888888888888888888888888888",
      "pubkey":"1111111111111111111111111111111111111111111111111111111111111111",
      "created_at":10,
      "tags":[["d","repo"]]
    }"#;
    let status = http_post_status(&base_url, "/announcement", payload);
    assert_eq!(status, Some(400));

    stop_server(&mut child);
    let _ = std::fs::remove_dir_all(runtime_dir);
    forgejo_handle.abort();
    let _ = forgejo_handle.await;
}

#[tokio::test]
async fn coordinator_binary_runtime_announcement_forgejo_error_returns_internal_error() {
    let _guard = async_test_lock().lock().await;
    let port = reserve_local_port();
    let (mut child, base_url, runtime_dir) =
        start_coordinator_server_with(port, "http://127.0.0.1:1", "wss://relay.example");
    wait_for_health(&base_url, &mut child).await;

    let payload = r#"{
      "kind":30617,
      "event_id":"7777777777777777777777777777777777777777777777777777777777777777",
      "pubkey":"1111111111111111111111111111111111111111111111111111111111111111",
      "created_at":10,
      "tags":[
        ["d","repo"],
        ["clone","https://gittr.ee/npub1gjttreegkzys8jlhdnfm3qe39h2gka79cpndd0jsms5fk7tuhcnsdw56jq/repo.git"],
        ["relays","wss://relay.example"]
      ]
    }"#;
    let status = http_post_status(&base_url, "/announcement", payload);
    assert_eq!(status, Some(500));

    stop_server(&mut child);
    let _ = std::fs::remove_dir_all(runtime_dir);
}

#[tokio::test]
async fn coordinator_serve_maps_invalid_forgejo_for_runtime_instantiation() {
    let _guard = async_test_lock().lock().await;
    let runtime_dir = new_runtime_dir();
    let _ = std::fs::create_dir_all(&runtime_dir);
    let mut config = direct_serve_config("127.0.0.1:0", &runtime_dir, "http://localhost:3000");
    config.forgejo.api_token = " ".to_string();
    let err = tokio::time::timeout(Duration::from_secs(3), serve(config))
        .await
        .expect("serve should return quickly")
        .expect_err("invalid forgejo config");
    assert!(matches!(
        err,
        CoordinatorError::Forgejo(_) | CoordinatorError::Observability(_)
    ));
    let _ = std::fs::remove_dir_all(runtime_dir);
}

#[tokio::test]
async fn coordinator_serve_maps_invalid_storage_for_runtime_instantiation() {
    let _guard = async_test_lock().lock().await;
    let runtime_dir = new_runtime_dir();
    let _ = std::fs::create_dir_all(&runtime_dir);
    let mut config = direct_serve_config("127.0.0.1:0", &runtime_dir, "http://localhost:3000");
    config.storage.max_connections = 0;
    config.storage.min_connections = 0;
    let err = tokio::time::timeout(Duration::from_secs(3), serve(config))
        .await
        .expect("serve should return quickly")
        .expect_err("invalid storage config");
    assert!(matches!(err, CoordinatorError::Storage(_)));
    let _ = std::fs::remove_dir_all(runtime_dir);
}

#[tokio::test]
async fn coordinator_serve_maps_observability_reinit_for_runtime_instantiation() {
    let _guard = async_test_lock().lock().await;
    let runtime_dir = new_runtime_dir();
    let _ = std::fs::create_dir_all(&runtime_dir);
    let occupied = std::net::TcpListener::bind("127.0.0.1:0").expect("bind occupied listener");
    let bind = occupied.local_addr().expect("occupied addr").to_string();

    let first = direct_serve_config(&bind, &runtime_dir, "http://localhost:3000");
    let _first_err = tokio::time::timeout(Duration::from_secs(3), serve(first))
        .await
        .expect("first serve should return quickly")
        .expect_err("occupied bind should fail");

    let second = direct_serve_config("127.0.0.1:0", &runtime_dir, "http://localhost:3000");
    let err = tokio::time::timeout(Duration::from_secs(3), serve(second))
        .await
        .expect("serve should return quickly")
        .expect_err("observability reinit error");
    assert!(matches!(err, CoordinatorError::Observability(_)));
    let _ = std::fs::remove_dir_all(runtime_dir);
}

#[test]
fn coordinator_config_from_env_covers_runtime_error_paths() {
    let _lock = env_lock().lock().expect("env lock");

    let keys = [
        "GITTREE_COORDINATOR_BIND",
        "GITTREE_STORAGE_READ_URL",
        "GITTREE_STORAGE_MAX_CONNECTIONS",
        "GITTREE_RELAY_URLS",
        "GITTREE_COORDINATOR_REPO_ROOT",
        "GITTREE_COORDINATOR_PRE_RECEIVE_HOOK",
        "GITTREE_COORDINATOR_POST_RECEIVE_HOOK",
        "GITTREE_FORGEJO_BASE_URL",
        "GITTREE_FORGEJO_API_TOKEN",
        "GITTREE_FORGEJO_OWNER",
        "GITTREE_FORGEJO_WEBHOOK_URL",
        "GITTREE_FORGEJO_WEBHOOK_SECRET",
    ];
    let _restore = EnvRestore::capture(&keys);

    let runtime_dir = new_runtime_dir();
    let _ = std::fs::create_dir_all(&runtime_dir);
    let repo_root = runtime_dir.join("repos");
    let _ = std::fs::create_dir_all(&repo_root);
    let pre_hook = write_hook_source(&runtime_dir, "pre-env-hook");
    let post_hook = write_hook_source(&runtime_dir, "post-env-hook");

    set_env_var("GITTREE_COORDINATOR_BIND", "127.0.0.1:9091");
    set_env_var(
        "GITTREE_STORAGE_READ_URL",
        "postgres://user:pass@localhost:5432/gittree",
    );
    set_env_var("GITTREE_RELAY_URLS", "wss://relay.example");
    set_env_var("GITTREE_COORDINATOR_REPO_ROOT", &repo_root);
    set_env_var("GITTREE_COORDINATOR_PRE_RECEIVE_HOOK", &pre_hook);
    set_env_var("GITTREE_COORDINATOR_POST_RECEIVE_HOOK", &post_hook);
    set_env_var("GITTREE_FORGEJO_BASE_URL", "http://localhost:3000");
    set_env_var("GITTREE_FORGEJO_API_TOKEN", "token");
    set_env_var("GITTREE_FORGEJO_OWNER", "gittree");
    set_env_var("GITTREE_FORGEJO_WEBHOOK_URL", "http://localhost:3000/webhook");
    set_env_var("GITTREE_FORGEJO_WEBHOOK_SECRET", "secret");

    let config = CoordinatorConfig::from_env().expect("valid coordinator env");
    assert_eq!(config.bind, "127.0.0.1:9091");

    set_env_var("GITTREE_COORDINATOR_BIND", "not-an-addr");
    let err = CoordinatorConfig::from_env().expect_err("invalid bind should fail");
    assert!(matches!(err, CoordinatorConfigError::Config(_)));
    set_env_var("GITTREE_COORDINATOR_BIND", "127.0.0.1:9091");

    set_env_var("GITTREE_STORAGE_MAX_CONNECTIONS", "not-a-number");
    let err = CoordinatorConfig::from_env().expect_err("invalid storage env should fail");
    assert!(matches!(
        err,
        CoordinatorConfigError::Storage(StorageConfigError::InvalidEnv { .. })
    ));
    remove_env_var("GITTREE_STORAGE_MAX_CONNECTIONS");

    set_env_var("GITTREE_RELAY_URLS", "not-a-url");
    let err = CoordinatorConfig::from_env().expect_err("invalid relay target should fail");
    assert!(matches!(err, CoordinatorConfigError::Config(_)));
    set_env_var("GITTREE_RELAY_URLS", "wss://relay.example");

    remove_env_var("GITTREE_COORDINATOR_REPO_ROOT");
    let err = CoordinatorConfig::from_env().expect_err("missing repo root should fail");
    assert!(matches!(
        err,
        CoordinatorConfigError::MissingEnv("GITTREE_COORDINATOR_REPO_ROOT")
    ));
    set_env_var("GITTREE_COORDINATOR_REPO_ROOT", &repo_root);

    remove_env_var("GITTREE_COORDINATOR_PRE_RECEIVE_HOOK");
    let err = CoordinatorConfig::from_env().expect_err("missing pre-receive hook should fail");
    assert!(matches!(
        err,
        CoordinatorConfigError::MissingEnv("GITTREE_COORDINATOR_PRE_RECEIVE_HOOK")
    ));
    set_env_var("GITTREE_COORDINATOR_PRE_RECEIVE_HOOK", &pre_hook);

    remove_env_var("GITTREE_COORDINATOR_POST_RECEIVE_HOOK");
    let err = CoordinatorConfig::from_env().expect_err("missing post-receive hook should fail");
    assert!(matches!(
        err,
        CoordinatorConfigError::MissingEnv("GITTREE_COORDINATOR_POST_RECEIVE_HOOK")
    ));
    set_env_var("GITTREE_COORDINATOR_POST_RECEIVE_HOOK", &post_hook);

    remove_env_var("GITTREE_FORGEJO_OWNER");
    let err = CoordinatorConfig::from_env().expect_err("missing forgejo owner should fail");
    assert!(matches!(err, CoordinatorConfigError::Config(_)));

    let _ = std::fs::remove_dir_all(runtime_dir);
}
