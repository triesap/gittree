use std::process::Command;

#[test]
fn migrate_binary_invalid_storage_url_exits_with_error() {
    let output = Command::new(env!("CARGO_BIN_EXE_gittree-migrate"))
        .env("GITTREE_STORAGE_READ_URL", "://invalid")
        .output()
        .expect("run migrate binary");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("migration failed:"));
}
