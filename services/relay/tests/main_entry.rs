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
