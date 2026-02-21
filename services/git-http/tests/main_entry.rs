use std::net::TcpListener;
use std::process::Command;

fn base_command() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_gittree-git-http"));
    command.current_dir("/tmp");
    for key in [
        "GITTREE_STORAGE_READ_URL",
        "GITTREE_GIT_HTTP_UPSTREAM_URL",
        "GITTREE_GIT_HTTP_BIND",
        "GITTREE_LOG_JSON",
        "GITTREE_LOG_STDOUT",
        "GITTREE_LOG_DIR",
        "GITTREE_METRICS_ENABLED",
    ] {
        command.env_remove(key);
    }
    command
}

#[test]
fn git_http_binary_invalid_bind_exits_with_config_error() {
    let output = base_command()
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

#[test]
fn git_http_binary_occupied_bind_exits_with_serve_error() {
    let occupied = TcpListener::bind("127.0.0.1:0").expect("bind occupied listener");
    let bind = occupied.local_addr().expect("occupied addr").to_string();

    let output = base_command()
        .env(
            "GITTREE_STORAGE_READ_URL",
            "postgres://user:pass@localhost:5432/gittree",
        )
        .env("GITTREE_GIT_HTTP_UPSTREAM_URL", "https://git.example")
        .env("GITTREE_GIT_HTTP_BIND", bind)
        .output()
        .expect("run git-http binary");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("git-http service failed"));
    assert!(stderr.contains("git-http serve error"));
}
