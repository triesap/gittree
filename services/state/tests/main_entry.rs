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
