use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::error::Error;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use hmac::Mac;
use gittree_webhook::{StorageConfigError, WebhookConfig, WebhookConfigError, WebhookError};
use gittree_storage::StorageConfig;

fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn with_env_vars(vars: &[(&str, Option<&str>)], run: impl FnOnce()) {
    let _guard = env_lock().lock().expect("lock env");
    let previous: Vec<(&str, Option<std::ffi::OsString>)> = vars
        .iter()
        .map(|(key, _)| (*key, std::env::var_os(key)))
        .collect();

    for (key, value) in vars {
        match value {
            Some(value) => {
                // SAFETY: tests serialize environment mutation with a process-wide mutex.
                unsafe { std::env::set_var(key, value) };
            }
            None => {
                // SAFETY: tests serialize environment mutation with a process-wide mutex.
                unsafe { std::env::remove_var(key) };
            }
        }
    }

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(run));

    for (key, value) in previous {
        match value {
            Some(value) => {
                // SAFETY: tests serialize environment mutation with a process-wide mutex.
                unsafe { std::env::set_var(key, value) };
            }
            None => {
                // SAFETY: tests serialize environment mutation with a process-wide mutex.
                unsafe { std::env::remove_var(key) };
            }
        }
    }

    if let Err(payload) = result {
        std::panic::resume_unwind(payload);
    }
}

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
    let _guard = env_lock().lock().expect("lock env");
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

#[test]
fn webhook_runtime_error_traits_cover_additional_paths() {
    let storage_missing = StorageConfigError::MissingEnv("GITTREE_STORAGE_READ_URL");
    assert_eq!(
        storage_missing.to_string(),
        "missing env GITTREE_STORAGE_READ_URL"
    );
    let storage_missing_error: &dyn std::error::Error = &storage_missing;
    assert!(storage_missing_error.source().is_none());

    let config_storage = WebhookConfigError::Storage(StorageConfigError::InvalidConfig(
        "invalid pool".to_string(),
    ));
    assert!(config_storage.source().is_some());

    let webhook_notify = WebhookError::Notify("sync failed".to_string());
    assert_eq!(webhook_notify.to_string(), "webhook notify error: sync failed");
    assert!(webhook_notify.source().is_none());
}

#[test]
fn webhook_runtime_config_rejects_invalid_storage_numeric_env() {
    with_env_vars(
        &[
            ("GITTREE_WEBHOOK_BIND", Some("127.0.0.1:9099")),
            (
                "GITTREE_STORAGE_READ_URL",
                Some("postgres://user:pass@localhost:5432/gittree"),
            ),
            ("GITTREE_STORAGE_MAX_CONNECTIONS", Some("not-a-number")),
            ("GITTREE_SYNC_URL", Some("http://localhost:8084")),
            ("GITTREE_FORGEJO_WEBHOOK_SECRET", Some("secret")),
        ],
        || {
            let err = WebhookConfig::from_env().expect_err("invalid storage env");
            match err {
                WebhookConfigError::Storage(StorageConfigError::InvalidEnv { key, value }) => {
                    assert_eq!(key, "GITTREE_STORAGE_MAX_CONNECTIONS");
                    assert_eq!(value, "not-a-number");
                }
                other => panic!("unexpected config error: {other:?}"),
            }
        },
    );
}

#[test]
fn webhook_runtime_config_rejects_invalid_bind() {
    with_env_vars(
        &[
            ("GITTREE_WEBHOOK_BIND", Some("not-a-socket")),
            (
                "GITTREE_STORAGE_READ_URL",
                Some("postgres://user:pass@localhost:5432/gittree"),
            ),
            ("GITTREE_SYNC_URL", Some("http://localhost:8084")),
            ("GITTREE_FORGEJO_WEBHOOK_SECRET", Some("secret")),
        ],
        || {
            let err = WebhookConfig::from_env().expect_err("invalid bind");
            match err {
                WebhookConfigError::Config(_) => {}
                other => panic!("unexpected config error: {other:?}"),
            }
        },
    );
}

#[test]
fn webhook_runtime_config_loads_valid_storage_values() {
    with_env_vars(
        &[
            ("GITTREE_WEBHOOK_BIND", Some("127.0.0.1:9099")),
            (
                "GITTREE_STORAGE_READ_URL",
                Some("postgres://user:pass@localhost:5432/gittree"),
            ),
            ("GITTREE_STORAGE_WRITE_URL", Some("postgres://user:pass@localhost:5432/gittree")),
            ("GITTREE_STORAGE_MAX_CONNECTIONS", Some("16")),
            ("GITTREE_STORAGE_MIN_CONNECTIONS", Some("4")),
            ("GITTREE_STORAGE_IDLE_TIMEOUT_SECS", Some("30")),
            ("GITTREE_STORAGE_MAX_LIFETIME_SECS", Some("120")),
            ("GITTREE_STORAGE_APP_NAME", Some("gittree-webhook-tests")),
            ("GITTREE_SYNC_URL", Some("http://localhost:8084")),
            ("GITTREE_FORGEJO_WEBHOOK_SECRET", Some("secret")),
        ],
        || {
            let config = WebhookConfig::from_env().expect("valid webhook config");
            assert_eq!(config.storage.max_connections, 16);
            assert_eq!(config.storage.min_connections, 4);
            assert_eq!(config.storage.idle_timeout_secs, Some(30));
            assert_eq!(config.storage.max_lifetime_secs, Some(120));
            assert_eq!(
                config.storage.application_name.as_deref(),
                Some("gittree-webhook-tests")
            );
        },
    );
}

#[test]
fn webhook_runtime_config_rejects_missing_sync_after_storage_parse() {
    with_env_vars(
        &[
            ("GITTREE_WEBHOOK_BIND", Some("127.0.0.1:9099")),
            (
                "GITTREE_STORAGE_READ_URL",
                Some("postgres://user:pass@localhost:5432/gittree"),
            ),
            ("GITTREE_SYNC_URL", None),
            ("GITTREE_FORGEJO_WEBHOOK_SECRET", Some("secret")),
        ],
        || {
            let err = WebhookConfig::from_env().expect_err("missing sync url");
            match err {
                WebhookConfigError::MissingEnv(key) => assert_eq!(key, "GITTREE_SYNC_URL"),
                other => panic!("unexpected config error: {other:?}"),
            }
        },
    );
}

#[test]
fn webhook_runtime_config_rejects_missing_secret_after_sync_parse() {
    with_env_vars(
        &[
            ("GITTREE_WEBHOOK_BIND", Some("127.0.0.1:9099")),
            (
                "GITTREE_STORAGE_READ_URL",
                Some("postgres://user:pass@localhost:5432/gittree"),
            ),
            ("GITTREE_SYNC_URL", Some("http://localhost:8084")),
            ("GITTREE_FORGEJO_WEBHOOK_SECRET", None),
        ],
        || {
            let err = WebhookConfig::from_env().expect_err("missing webhook secret");
            match err {
                WebhookConfigError::MissingEnv(key) => {
                    assert_eq!(key, "GITTREE_FORGEJO_WEBHOOK_SECRET")
                }
                other => panic!("unexpected config error: {other:?}"),
            }
        },
    );
}

#[tokio::test]
async fn webhook_runtime_serve_covers_notifier_builder_success_path() {
    let config = WebhookConfig {
        bind: "not-a-socket".to_string(),
        storage: StorageConfig {
            read_connection: runtime_storage_url(),
            write_connection: None,
            max_connections: 10,
            min_connections: 2,
            idle_timeout_secs: None,
            max_lifetime_secs: None,
            application_name: Some("gittree-webhook-runtime".to_string()),
        },
        sync_url: "http://127.0.0.1:1".to_string(),
        forgejo_secret: "secret".to_string(),
    };

    let error = gittree_webhook::serve(config)
        .await
        .expect_err("invalid bind should fail serve");
    assert!(error.to_string().contains("webhook serve error"));
}
