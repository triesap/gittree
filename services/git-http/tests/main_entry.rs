use std::process::Command;

#[test]
fn git_http_binary_invalid_bind_exits_with_config_error() {
    let output = Command::new(env!("CARGO_BIN_EXE_gittree-git-http"))
        .env_clear()
        .env(
            "PATH",
            std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".to_string()),
        )
        .env(
            "GITTREE_STORAGE_READ_URL",
            "postgres://user:pass@localhost:5432/gittree",
        )
        .env("GITTREE_GIT_HTTP_UPSTREAM_URL", "https://git.example")
        .env("GITTREE_GIT_HTTP_BIND", "invalid-bind")
        .output()
        .expect("run git-http binary");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("git-http service failed"));
    assert!(stderr.contains("git-http config error"));
}
