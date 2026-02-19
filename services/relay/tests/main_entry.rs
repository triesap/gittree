use std::process::Command;

#[test]
fn relay_binary_help_exits_successfully() {
    let output = Command::new(env!("CARGO_BIN_EXE_gittree-relay"))
        .arg("--help")
        .output()
        .expect("run relay binary");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("gittree-relay [--config <path>] [--bind <addr>]"));
}

#[test]
fn relay_binary_unknown_flag_exits_with_error() {
    let output = Command::new(env!("CARGO_BIN_EXE_gittree-relay"))
        .arg("--unknown")
        .output()
        .expect("run relay binary");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("relay service failed: relay cli error: unknown flag --unknown"));
}

#[test]
fn relay_binary_missing_config_exits_with_error() {
    let output = Command::new(env!("CARGO_BIN_EXE_gittree-relay"))
        .arg("--config")
        .arg("/definitely/missing/gittree-relay.toml")
        .output()
        .expect("run relay binary");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("relay service failed: relay config error:"));
}

#[test]
fn relay_binary_invalid_storage_url_exits_with_error() {
    let output = Command::new(env!("CARGO_BIN_EXE_gittree-relay"))
        .env("GITTREE_STORAGE_READ_URL", "://not-a-valid-postgres-url")
        .env("GITTREE_STORAGE_MAX_CONNECTIONS", "0")
        .output()
        .expect("run relay binary");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("relay service failed: relay config error:"));
}

#[test]
fn relay_binary_invalid_bind_exits_with_error() {
    let output = Command::new(env!("CARGO_BIN_EXE_gittree-relay"))
        .env(
            "GITTREE_STORAGE_READ_URL",
            "postgres://user:pass@localhost:5432/gittree",
        )
        .arg("--bind")
        .arg("invalid-bind")
        .output()
        .expect("run relay binary");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("relay service failed: relay serve error:"));
}
