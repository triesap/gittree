use std::process::Command;

#[test]
fn auth_binary_invalid_bind_exits_with_config_error() {
    let output = Command::new(env!("CARGO_BIN_EXE_gittree-auth"))
        .env("GITTREE_AUTH_BIND", "not-a-socket")
        .output()
        .expect("run auth binary");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("auth service failed: auth error: auth config error:"));
}
