use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn state_binary_invalid_bind_exits_with_error() {
    let run_dir = std::env::temp_dir().join(format!(
        "gittree-state-main-entry-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    ));
    std::fs::create_dir_all(&run_dir).expect("create temp run dir");

    let output = Command::new(env!("CARGO_BIN_EXE_gittree-state"))
        .current_dir(&run_dir)
        .env(
            "GITTREE_STORAGE_READ_URL",
            "postgres://user:pass@localhost:5432/gittree",
        )
        .env("GITTREE_RELAY_URLS", "wss://relay.example")
        .env("GITTREE_STATE_BIND", "invalid-bind")
        .output()
        .expect("run state binary");

    std::fs::remove_dir_all(&run_dir).ok();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("state service failed:"));
}

#[test]
fn state_binary_occupied_bind_exits_with_serve_error() {
    let run_dir = std::env::temp_dir().join(format!(
        "gittree-state-main-entry-occupied-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    ));
    std::fs::create_dir_all(&run_dir).expect("create temp run dir");
    let occupied = std::net::TcpListener::bind("127.0.0.1:0").expect("bind occupied listener");
    let bind = occupied.local_addr().expect("occupied addr").to_string();

    let output = Command::new(env!("CARGO_BIN_EXE_gittree-state"))
        .current_dir(&run_dir)
        .env(
            "GITTREE_STORAGE_READ_URL",
            "postgres://user:pass@localhost:5432/gittree",
        )
        .env("GITTREE_RELAY_URLS", "wss://relay.example")
        .env("GITTREE_STATE_BIND", bind)
        .output()
        .expect("run state binary");

    std::fs::remove_dir_all(&run_dir).ok();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("state service failed:"));
    assert!(stderr.contains("state serve error"));
}
