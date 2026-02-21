use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn relay_probe_binary_help_exits_successfully() {
    let run_dir = std::env::temp_dir().join(format!(
        "gittree-relay-probe-main-entry-help-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    ));
    std::fs::create_dir_all(&run_dir).expect("create temp run dir");

    let output = Command::new(env!("CARGO_BIN_EXE_gittree-relay-probe"))
        .current_dir(&run_dir)
        .arg("--help")
        .output()
        .expect("run relay probe binary");

    std::fs::remove_dir_all(&run_dir).ok();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("gittree-relay-probe --relay"));
}

#[test]
fn relay_probe_binary_missing_args_exits_with_error() {
    let run_dir = std::env::temp_dir().join(format!(
        "gittree-relay-probe-main-entry-error-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    ));
    std::fs::create_dir_all(&run_dir).expect("create temp run dir");

    let output = Command::new(env!("CARGO_BIN_EXE_gittree-relay-probe"))
        .current_dir(&run_dir)
        .output()
        .expect("run relay probe binary");

    std::fs::remove_dir_all(&run_dir).ok();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("relay probe failed: invalid relay url: missing --relay or --all"));
}
