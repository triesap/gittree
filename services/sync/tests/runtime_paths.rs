use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::json;

fn reserve_local_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind local port");
    let port = listener.local_addr().expect("listener addr").port();
    drop(listener);
    port
}

fn runtime_storage_url() -> String {
    std::env::var("GITTREE_STORAGE_TEST_DATABASE_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "postgres://user:pass@127.0.0.1:1/gittree".to_string())
}

fn spawn_sync_server(port: u16) -> (Child, String, PathBuf) {
    let temp_dir = std::env::temp_dir().join(format!(
        "gittree-sync-runtime-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    ));
    let repo_root = temp_dir.join("repos");
    std::fs::create_dir_all(&repo_root).expect("create repo root");

    let bind = format!("127.0.0.1:{port}");
    let base_url = format!("http://{bind}");

    let mut command = Command::new(env!("CARGO_BIN_EXE_gittree-sync"));
    command
        .current_dir(&temp_dir)
        .env("GITTREE_SYNC_BIND", bind)
        .env("GITTREE_STORAGE_READ_URL", runtime_storage_url())
        .env("GITTREE_RELAY_URLS", "wss://relay.example")
        .env("GITTREE_SYNC_REPO_ROOT", &repo_root)
        .env("GITTREE_LOG_STDOUT", "false")
        .env("GITTREE_METRICS_ENABLED", "false")
        .env("GITTREE_LOG_DIR", temp_dir.join("logs"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    let child = command.spawn().expect("spawn sync server");
    (child, base_url, temp_dir)
}

async fn wait_for_health(base_url: &str) {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_millis(500))
        .build()
        .expect("http client");
    for _ in 0..60 {
        if let Ok(response) = client.get(format!("{base_url}/health")).send().await
            && response.status() == reqwest::StatusCode::OK
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("sync server never became ready");
}

fn stop_sync_server(child: &mut Child) {
    let _ = Command::new("kill")
        .arg("-TERM")
        .arg(child.id().to_string())
        .status();
    for _ in 0..20 {
        if child.try_wait().ok().flatten().is_some() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = child.kill();
    let _ = child.wait();
}

#[tokio::test]
async fn sync_binary_runtime_routes_cover_non_test_monomorphizations() {
    let port = reserve_local_port();
    let (mut child, base_url, temp_dir) = spawn_sync_server(port);
    wait_for_health(&base_url).await;

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .expect("http client");

    let missing = client
        .get(format!("{base_url}/missing"))
        .send()
        .await
        .expect("missing route response");
    assert_eq!(missing.status(), reqwest::StatusCode::NOT_FOUND);

    let valid = client
        .post(format!("{base_url}/"))
        .header("content-type", "application/json")
        .json(&json!({
            "pubkey": "11".repeat(32),
            "identifier": "repo",
            "updates": []
        }))
        .send()
        .await
        .expect("valid post-receive response");
    assert_eq!(valid.status(), reqwest::StatusCode::OK);

    let invalid = client
        .post(format!("{base_url}/"))
        .header("content-type", "application/json")
        .json(&json!({
            "pubkey": "11".repeat(32),
            "identifier": "",
            "updates": []
        }))
        .send()
        .await
        .expect("invalid post-receive response");
    assert_eq!(invalid.status(), reqwest::StatusCode::BAD_REQUEST);

    stop_sync_server(&mut child);
    std::fs::remove_dir_all(temp_dir).ok();
}

#[test]
fn sync_binary_missing_repo_root_exits_with_config_error() {
    let temp_dir = std::env::temp_dir().join(format!(
        "gittree-sync-missing-repo-root-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    ));
    std::fs::create_dir_all(&temp_dir).expect("create temp dir");

    let output = Command::new(env!("CARGO_BIN_EXE_gittree-sync"))
        .current_dir(&temp_dir)
        .env_remove("GITTREE_SYNC_REPO_ROOT")
        .env("GITTREE_SYNC_BIND", "127.0.0.1:0")
        .env("GITTREE_STORAGE_READ_URL", runtime_storage_url())
        .env("GITTREE_RELAY_URLS", "wss://relay.example")
        .output()
        .expect("run sync binary");

    std::fs::remove_dir_all(temp_dir).ok();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("sync service failed:"));
    assert!(stderr.contains("missing env GITTREE_SYNC_REPO_ROOT"));
}
