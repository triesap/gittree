use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use hmac::Mac;

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

fn spawn_webhook_server(port: u16) -> (Child, String, PathBuf) {
    let temp_dir = std::env::temp_dir().join(format!(
        "gittree-webhook-runtime-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    ));
    std::fs::create_dir_all(&temp_dir).expect("create temp dir");

    let bind = format!("127.0.0.1:{port}");
    let base_url = format!("http://{bind}");
    let storage_url = runtime_storage_url();

    let mut command = Command::new(env!("CARGO_BIN_EXE_gittree-webhook"));
    command
        .current_dir(&temp_dir)
        .env("GITTREE_WEBHOOK_BIND", bind)
        .env("GITTREE_STORAGE_READ_URL", storage_url)
        .env("GITTREE_SYNC_URL", "http://127.0.0.1:1")
        .env("GITTREE_FORGEJO_WEBHOOK_SECRET", "secret")
        .env("GITTREE_LOG_STDOUT", "false")
        .env("GITTREE_METRICS_ENABLED", "false")
        .env("GITTREE_LOG_DIR", temp_dir.join("logs"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    let child = command.spawn().expect("spawn webhook server");
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
    panic!("webhook server never became ready");
}

fn stop_webhook_server(child: &mut Child) {
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

fn sign_payload(secret: &[u8], payload: &[u8]) -> String {
    let mut mac = hmac::Hmac::<sha2::Sha256>::new_from_slice(secret).expect("mac");
    mac.update(payload);
    hex::encode(mac.finalize().into_bytes())
}

#[tokio::test]
async fn webhook_binary_runtime_routes_cover_non_test_monomorphizations() {
    let port = reserve_local_port();
    let (mut child, base_url, temp_dir) = spawn_webhook_server(port);
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

    // Runtime request path without a forgejo signature should fail authorization.
    let no_sig = client
        .post(format!("{base_url}/"))
        .header("content-type", "application/json")
        .body("{}")
        .send()
        .await
        .expect("webhook response");
    assert_eq!(no_sig.status(), reqwest::StatusCode::UNAUTHORIZED);

    let bad_sig = client
        .post(format!("{base_url}/"))
        .header("content-type", "application/json")
        .header("x-gitea-signature", "sha256=not-hex")
        .body("{}")
        .send()
        .await
        .expect("bad signature response");
    assert_eq!(bad_sig.status(), reqwest::StatusCode::UNAUTHORIZED);

    let invalid_utf8 = [0xff, 0xfe, 0xfd];
    let invalid_utf8_signature = sign_payload(b"secret", &invalid_utf8);
    let invalid_utf8_resp = client
        .post(format!("{base_url}/"))
        .header("x-gitea-signature", format!("sha256={invalid_utf8_signature}"))
        .body(invalid_utf8.to_vec())
        .send()
        .await
        .expect("invalid utf8 response");
    assert_eq!(invalid_utf8_resp.status(), reqwest::StatusCode::BAD_REQUEST);

    let invalid_json = br#"{"ref":"refs/heads/main"}"#;
    let invalid_json_signature = sign_payload(b"secret", invalid_json);
    let invalid_json_resp = client
        .post(format!("{base_url}/"))
        .header("content-type", "application/json")
        .header("x-gitea-signature", format!("sha256={invalid_json_signature}"))
        .body(invalid_json.to_vec())
        .send()
        .await
        .expect("invalid json response");
    assert_eq!(invalid_json_resp.status(), reqwest::StatusCode::BAD_REQUEST);

    let valid_signature = sign_payload(b"secret", invalid_json);
    let forgejo_header_only = client
        .post(format!("{base_url}/"))
        .header("content-type", "application/json")
        .header("x-forgejo-signature", format!("sha256={valid_signature}"))
        .body(invalid_json.to_vec())
        .send()
        .await
        .expect("forgejo signature header response");
    assert_eq!(forgejo_header_only.status(), reqwest::StatusCode::BAD_REQUEST);

    let hub_header_only = client
        .post(format!("{base_url}/"))
        .header("content-type", "application/json")
        .header("x-hub-signature-256", format!("sha256={valid_signature}"))
        .body(invalid_json.to_vec())
        .send()
        .await
        .expect("hub signature header response");
    assert_eq!(hub_header_only.status(), reqwest::StatusCode::BAD_REQUEST);

    stop_webhook_server(&mut child);
    std::fs::remove_dir_all(temp_dir).ok();
}

#[test]
fn webhook_binary_missing_secret_exits_with_config_error() {
    let temp_dir = std::env::temp_dir().join(format!(
        "gittree-webhook-missing-secret-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    ));
    std::fs::create_dir_all(&temp_dir).expect("create temp dir");

    let output = Command::new(env!("CARGO_BIN_EXE_gittree-webhook"))
        .current_dir(&temp_dir)
        .env_remove("GITTREE_FORGEJO_WEBHOOK_SECRET")
        .env("GITTREE_WEBHOOK_BIND", "127.0.0.1:0")
        .env("GITTREE_STORAGE_READ_URL", runtime_storage_url())
        .env("GITTREE_SYNC_URL", "http://127.0.0.1:1")
        .output()
        .expect("run webhook binary");

    std::fs::remove_dir_all(temp_dir).ok();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("webhook service failed:"));
    assert!(stderr.contains("missing env GITTREE_FORGEJO_WEBHOOK_SECRET"));
}
