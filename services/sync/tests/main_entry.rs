use std::process::Command;

#[test]
fn sync_binary_invalid_bind_exits_with_config_error() {
    let output = Command::new(env!("CARGO_BIN_EXE_gittree-sync"))
        .env("GITTREE_SYNC_BIND", "not-a-socket")
        .env(
            "GITTREE_STORAGE_READ_URL",
            "postgres://user:pass@localhost:5432/gittree",
        )
        .env("GITTREE_RELAY_URLS", "wss://relay.example")
        .env("GITTREE_SYNC_REPO_ROOT", "/tmp/gittree")
        .output()
        .expect("run sync binary");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("sync service failed: sync error: sync config error:"));
    assert!(stderr.contains("not-a-socket"));
}
