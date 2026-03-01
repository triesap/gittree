use gittree_config::ForgejoConfig;
use gittree_coordinator::{CoordinatorConfig, HookInstallConfig, serve};
use gittree_storage::StorageConfig;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

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

fn write_hook(path: &PathBuf, name: &str) -> PathBuf {
    let hook = path.join(name);
    std::fs::write(&hook, "#!/bin/sh\nexit 0\n").expect("write hook");
    hook
}

fn runtime_config(port: u16, root: &PathBuf) -> CoordinatorConfig {
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
            base_url: "http://127.0.0.1:1".to_string(),
            api_token: "token".to_string(),
            owner: "owner".to_string(),
            webhook_url: "http://127.0.0.1:1/webhook".to_string(),
            webhook_secret: "secret".to_string(),
            repo_private: true,
        },
    }
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

async fn http_post_status(base_url: &str, path: &str, body: &str) -> Option<u16> {
    let endpoint = base_url.trim_start_matches("http://");
    let mut stream = tokio::net::TcpStream::connect(endpoint).await.ok()?;
    let request = format!(
        "POST {path} HTTP/1.1\r\nHost: {endpoint}\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(request.as_bytes()).await.ok()?;
    let mut reader = BufReader::new(stream);
    let mut status_line = String::new();
    reader.read_line(&mut status_line).await.ok()?;
    parse_status_code(status_line.trim_end())
}

#[tokio::test]
async fn coordinator_serve_rejects_invalid_kind_with_postgres_runtime_state() {
    let port = reserve_local_port();
    let temp_dir = unique_temp_dir("gittree-coordinator-postgres-runtime");
    std::fs::create_dir_all(&temp_dir).expect("temp dir");
    let config = runtime_config(port, &temp_dir);
    let base_url = format!("http://127.0.0.1:{port}");

    let handle = tokio::spawn(async move { serve(config).await });
    wait_for_health(&base_url).await;

    let payload = r#"{
      "kind":18446744073709551615,
      "event_id":"4444444444444444444444444444444444444444444444444444444444444444",
      "pubkey":"1111111111111111111111111111111111111111111111111111111111111111",
      "created_at":10,
      "tags":[]
    }"#;
    let response = http_post_status(&base_url, "/announcement", payload).await;
    assert_eq!(response, Some(400));

    handle.abort();
    let _ = handle.await;
    let _ = std::fs::remove_dir_all(temp_dir);
}
