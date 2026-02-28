use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn webhook_binary_invalid_bind_exits_with_error() {
    let run_dir = std::env::temp_dir().join(format!(
        "gittree-webhook-main-entry-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    ));
    std::fs::create_dir_all(&run_dir).expect("create temp run dir");

    let output = Command::new(env!("CARGO_BIN_EXE_gittree-webhook"))
        .current_dir(&run_dir)
        .env(
            "GITTREE_STORAGE_READ_URL",
            "postgres://user:pass@localhost:5432/gittree",
        )
        .env("GITTREE_SYNC_URL", "http://localhost:8084")
        .env("GITTREE_FORGEJO_WEBHOOK_SECRET", "secret")
        .env("GITTREE_WEBHOOK_BIND", "invalid-bind")
        .output()
        .expect("run webhook binary");

    std::fs::remove_dir_all(&run_dir).ok();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("webhook service failed:"));
}

#[test]
fn webhook_binary_occupied_bind_exits_with_serve_error() {
    let run_dir = std::env::temp_dir().join(format!(
        "gittree-webhook-main-entry-occupied-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    ));
    std::fs::create_dir_all(&run_dir).expect("create temp run dir");
    let occupied = std::net::TcpListener::bind("127.0.0.1:0").expect("bind occupied listener");
    let bind = occupied.local_addr().expect("occupied addr").to_string();

    let output = Command::new(env!("CARGO_BIN_EXE_gittree-webhook"))
        .current_dir(&run_dir)
        .env(
            "GITTREE_STORAGE_READ_URL",
            "postgres://user:pass@localhost:5432/gittree",
        )
        .env("GITTREE_SYNC_URL", "http://localhost:8084")
        .env("GITTREE_FORGEJO_WEBHOOK_SECRET", "secret")
        .env("GITTREE_WEBHOOK_BIND", bind)
        .output()
        .expect("run webhook binary");

    std::fs::remove_dir_all(&run_dir).ok();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("webhook service failed:"));
    assert!(stderr.contains("webhook serve error"));
}
