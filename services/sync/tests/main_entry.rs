use std::net::TcpListener;
use std::process::Command;

fn base_command() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_gittree-sync"));
    command.current_dir("/tmp");
    for key in [
        "GITTREE_SYNC_BIND",
        "GITTREE_STORAGE_READ_URL",
        "GITTREE_SYNC_REPO_ROOT",
    ] {
        command.env_remove(key);
    }
    command
}

#[test]
fn sync_binary_invalid_bind_exits_with_config_error() {
    let output = base_command()
        .env("GITTREE_SYNC_BIND", "invalid-bind")
        .env(
            "GITTREE_STORAGE_READ_URL",
            "postgres://user:pass@localhost:5432/gittree",
        )
        .env("GITTREE_SYNC_REPO_ROOT", "/tmp/gittree-sync")
        .output()
        .expect("run sync binary");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("sync service failed"));
    assert!(stderr.contains("sync config error"));
}

#[test]
fn sync_binary_occupied_bind_exits_with_serve_error() {
    let occupied = TcpListener::bind("127.0.0.1:0").expect("bind occupied listener");
    let bind = occupied.local_addr().expect("occupied addr").to_string();

    let output = base_command()
        .env("GITTREE_SYNC_BIND", bind)
        .env(
            "GITTREE_STORAGE_READ_URL",
            "postgres://user:pass@localhost:5432/gittree",
        )
        .env("GITTREE_SYNC_REPO_ROOT", "/tmp/gittree-sync")
        .output()
        .expect("run sync binary");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("sync service failed"));
    assert!(stderr.contains("sync serve error"));
}
