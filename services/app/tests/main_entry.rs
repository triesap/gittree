use std::process::Command;

#[test]
fn app_binary_invalid_bind_exits_with_config_error() {
    let output = Command::new(env!("CARGO_BIN_EXE_gittree-app"))
        .env("GITTREE_APP_BIND", "not-a-socket")
        .env(
            "GITTREE_STORAGE_READ_URL",
            "postgres://user:pass@localhost:5432/gittree",
        )
        .env("GITTREE_UI_REPO_ROOT", "/tmp/gittree")
        .env("GITTREE_UI_PUBLIC_GIT_URL", "http://localhost:3000")
        .output()
        .expect("run app binary");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("app service failed: app error:"));
    assert!(stderr.contains("GITTREE_APP_BIND"));
}

#[test]
fn app_binary_occupied_bind_exits_with_serve_error() {
    let occupied = std::net::TcpListener::bind("127.0.0.1:0").expect("bind occupied listener");
    let bind = occupied.local_addr().expect("occupied addr").to_string();

    let output = Command::new(env!("CARGO_BIN_EXE_gittree-app"))
        .env("GITTREE_APP_BIND", bind)
        .env(
            "GITTREE_STORAGE_READ_URL",
            "postgres://user:pass@localhost:5432/gittree",
        )
        .env("GITTREE_UI_REPO_ROOT", "/tmp/gittree")
        .env("GITTREE_UI_PUBLIC_GIT_URL", "http://localhost:3000")
        .output()
        .expect("run app binary");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("app service failed: app serve error:"));
}
