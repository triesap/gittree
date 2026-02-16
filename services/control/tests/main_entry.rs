use std::process::Command;

#[test]
fn control_binary_invalid_bind_exits_with_config_error() {
    let output = Command::new(env!("CARGO_BIN_EXE_gittree-control"))
        .env("GITTREE_CONTROL_BIND", "not-a-socket")
        .output()
        .expect("run control binary");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("control service failed: control error: control config error:"));
}
