use std::process::Command;

fn run_hook_binary(binary_path: &str, args: &[&str]) -> std::process::Output {
    let mut command = Command::new(binary_path);
    command.args(args).current_dir("/tmp");
    for key in [
        "GIT_DIR",
        "GITTREE_HOOK_MODE",
        "GITTREE_HOOK_REPO_PATH",
        "GITTREE_HOOK_STDIN_FILE",
        "GITTREE_STATE_URL",
        "GITTREE_SYNC_URL",
    ] {
        command.env_remove(key);
    }
    command.output().expect("run hook binary")
}

#[test]
fn git_hook_binary_reports_missing_config() {
    let output = run_hook_binary(
        env!("CARGO_BIN_EXE_gittree-git-hook"),
        &["--mode", "pre-receive"],
    );
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("git hook failed:"));
    assert!(stderr.contains("hook config error:"));
}

#[test]
fn pre_receive_binary_reports_missing_config() {
    let output = run_hook_binary(env!("CARGO_BIN_EXE_gittree-pre-receive"), &[]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("git hook failed:"));
    assert!(stderr.contains("hook config error:"));
}

#[test]
fn post_receive_binary_reports_missing_config() {
    let output = run_hook_binary(env!("CARGO_BIN_EXE_gittree-post-receive"), &[]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("git hook failed:"));
    assert!(stderr.contains("hook config error:"));
}
