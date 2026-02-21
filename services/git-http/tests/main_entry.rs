use std::process::Command;

#[test]
fn git_http_binary_invalid_bind_exits_with_config_error() {
    let mut command = Command::new(env!("CARGO_BIN_EXE_gittree-git-http"));
    command.current_dir("/tmp");
    for key in [
        "GITTREE_STORAGE_READ_URL",
        "GITTREE_GIT_HTTP_UPSTREAM_URL",
        "GITTREE_GIT_HTTP_BIND",
    ] {
        command.env_remove(key);
    }
    let output = command
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
