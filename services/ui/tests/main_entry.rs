use std::process::Command;

#[test]
fn ui_binary_invalid_bind_exits_with_config_error() {
    let output = Command::new(env!("CARGO_BIN_EXE_gittree-ui"))
        .env("GITTREE_UI_BIND", "not-a-socket")
        .env(
            "GITTREE_STORAGE_READ_URL",
            "postgres://user:pass@localhost:5432/gittree",
        )
        .env("GITTREE_UI_REPO_ROOT", "/tmp/gittree")
        .env("GITTREE_UI_PUBLIC_GIT_URL", "http://localhost:3000")
        .output()
        .expect("run ui binary");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("ui service failed: ui error: ui config error:"));
    assert!(stderr.contains("not-a-socket"));
}
