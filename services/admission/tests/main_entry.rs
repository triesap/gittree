use std::process::Command;

#[test]
fn admission_binary_invalid_bind_exits_with_config_error() {
    let output = Command::new(env!("CARGO_BIN_EXE_gittree-admission"))
        .env("GITTREE_ADMISSION_BIND", "not-a-socket")
        .env(
            "GITTREE_STORAGE_READ_URL",
            "postgres://user:pass@localhost:5432/gittree",
        )
        .output()
        .expect("run admission binary");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("admission service failed:"));
    assert!(stderr.contains("invalid admission bind address"));
}

#[test]
fn admission_binary_invalid_storage_url_exits_with_storage_error() {
    let output = Command::new(env!("CARGO_BIN_EXE_gittree-admission"))
        .env("GITTREE_ADMISSION_BIND", "127.0.0.1:0")
        .env("GITTREE_STORAGE_READ_URL", "://invalid")
        .output()
        .expect("run admission binary");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("admission service failed:"));
    assert!(stderr.contains("admission storage error"));
}

#[test]
fn admission_binary_occupied_bind_exits_with_serve_error() {
    let occupied = std::net::TcpListener::bind("127.0.0.1:0").expect("bind occupied");
    let bind = occupied.local_addr().expect("occupied addr");

    let output = Command::new(env!("CARGO_BIN_EXE_gittree-admission"))
        .env("GITTREE_ADMISSION_BIND", bind.to_string())
        .env(
            "GITTREE_STORAGE_READ_URL",
            "postgres://user:pass@localhost:5432/gittree",
        )
        .output()
        .expect("run admission binary");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("admission service failed:"));
    assert!(stderr.contains("admission serve error"));
}
