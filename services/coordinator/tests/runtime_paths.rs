use gittree_coordinator::{CoordinatorConfig, CoordinatorConfigError, StorageConfigError};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

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
        .unwrap_or_else(|| "postgres://user:pass@127.0.0.1:1/gittree".to_string())
}

fn new_runtime_dir() -> PathBuf {
    std::env::temp_dir().join(format!(
        "gittree-coordinator-runtime-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    ))
}

fn write_hook_source(path: &PathBuf, name: &str) -> PathBuf {
    let hook_path = path.join(name);
    std::fs::write(&hook_path, "#!/bin/sh\nexit 0\n").expect("write hook");
    hook_path
}

fn start_coordinator_server(port: u16) -> (Child, String, PathBuf) {
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
        .env("GITTREE_COORDINATOR_REPO_ROOT", &repo_root)
        .env("GITTREE_COORDINATOR_PRE_RECEIVE_HOOK", &pre_hook)
        .env("GITTREE_COORDINATOR_POST_RECEIVE_HOOK", &post_hook)
        .env("GITTREE_FORGEJO_BASE_URL", "http://127.0.0.1:1")
        .env("GITTREE_FORGEJO_API_TOKEN", "token")
        .env("GITTREE_FORGEJO_OWNER", "owner")
        .env("GITTREE_FORGEJO_WEBHOOK_URL", "http://127.0.0.1:1/webhook")
        .env("GITTREE_FORGEJO_WEBHOOK_SECRET", "secret")
        .env("GITTREE_LOG_STDOUT", "false")
        .env("GITTREE_METRICS_ENABLED", "false")
        .env("GITTREE_LOG_DIR", runtime_dir.join("logs"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());

    let child = command.spawn().expect("spawn coordinator");
    (child, base_url, runtime_dir)
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
    let port = reserve_local_port();
    let (mut child, base_url, runtime_dir) = start_coordinator_server(port);
    wait_for_health(&base_url, &mut child).await;

    assert_eq!(http_status(&base_url, "/health"), Some(200));
    assert!(matches!(
        http_post_status(&base_url, "/announcement", "{}"),
        Some(400 | 422)
    ));

    stop_server(&mut child);
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
