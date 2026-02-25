use std::process::{Command, Stdio};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

const SAMPLE_NPUB: &str = "npub1gjttreegkzys8jlhdnfm3qe39h2gka79cpndd0jsms5fk7tuhcnsdw56jq";

fn run_hook_binary_with_env(
    binary_path: &str,
    args: &[&str],
    extra_env: &[(&str, String)],
) -> std::process::Output {
    let mut command = Command::new(binary_path);
    command.args(args).current_dir("/tmp");
    for key in [
        "GIT_DIR",
        "GITTREE_HOOK_MODE",
        "GITTREE_HOOK_REPO_PATH",
        "GITTREE_HOOK_STDIN_FILE",
        "GITTREE_STATE_URL",
        "GITTREE_SYNC_URL",
        "GITTREE_RELAY_BIND",
        "GITTREE_ADMISSION_BIND",
        "GITTREE_STATE_BIND",
        "GITTREE_COORDINATOR_BIND",
        "GITTREE_SYNC_BIND",
        "GITTREE_GIT_HTTP_BIND",
        "GITTREE_UI_BIND",
        "GITTREE_WEBHOOK_BIND",
        "GITTREE_CONTROL_BIND",
        "GITTREE_AUTH_BIND",
    ] {
        command.env_remove(key);
    }
    for (key, value) in extra_env {
        command.env(key, value);
    }
    command.output().expect("run hook binary")
}

fn run_hook_binary(binary_path: &str, args: &[&str]) -> std::process::Output {
    run_hook_binary_with_env(binary_path, args, &[])
}

fn run_hook_binary_with_env_and_stdin(
    binary_path: &str,
    args: &[&str],
    extra_env: &[(&str, String)],
    stdin_input: &str,
) -> std::process::Output {
    let mut command = Command::new(binary_path);
    command
        .args(args)
        .current_dir("/tmp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for key in [
        "GIT_DIR",
        "GITTREE_HOOK_MODE",
        "GITTREE_HOOK_REPO_PATH",
        "GITTREE_HOOK_STDIN_FILE",
        "GITTREE_STATE_URL",
        "GITTREE_SYNC_URL",
        "GITTREE_RELAY_BIND",
        "GITTREE_ADMISSION_BIND",
        "GITTREE_STATE_BIND",
        "GITTREE_COORDINATOR_BIND",
        "GITTREE_SYNC_BIND",
        "GITTREE_GIT_HTTP_BIND",
        "GITTREE_UI_BIND",
        "GITTREE_WEBHOOK_BIND",
        "GITTREE_CONTROL_BIND",
        "GITTREE_AUTH_BIND",
    ] {
        command.env_remove(key);
    }
    for (key, value) in extra_env {
        command.env(key, value);
    }

    let mut child = command.spawn().expect("spawn hook binary");
    if let Some(stdin) = child.stdin.as_mut() {
        stdin
            .write_all(stdin_input.as_bytes())
            .expect("write stdin input");
    }
    child.wait_with_output().expect("wait hook binary")
}

fn write_updates_file(contents: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "gittree-hook-updates-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    ));
    std::fs::write(&path, contents).expect("write updates");
    path
}

fn sample_repo_path() -> String {
    format!("/tmp/{SAMPLE_NPUB}/repo.git")
}

fn nostr_updates_input() -> String {
    format!(
        "{} {} refs/nostr/{}\n",
        "0".repeat(40),
        "1".repeat(40),
        "2".repeat(64)
    )
}

fn start_sync_server() -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind sync server");
    let addr = listener.local_addr().expect("local addr");
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept");
        let mut request_buf = [0_u8; 2048];
        let _ = stream.read(&mut request_buf);
        let body = r#"{"status":"ok"}"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        stream.write_all(response.as_bytes()).expect("write response");
        stream.flush().expect("flush response");
    });
    (format!("http://{addr}"), handle)
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
fn git_hook_binary_pre_receive_succeeds_with_cli_args() {
    let updates_path = write_updates_file(&nostr_updates_input());
    let updates_path_str = updates_path.display().to_string();
    let output = run_hook_binary_with_env(
        env!("CARGO_BIN_EXE_gittree-git-hook"),
        &[
            "--mode",
            "pre-receive",
            "--state-url",
            "http://127.0.0.1:8082",
            "--stdin-file",
            &updates_path_str,
        ],
        &[("GITTREE_HOOK_REPO_PATH", sample_repo_path())],
    );
    std::fs::remove_file(&updates_path).ok();
    assert!(output.status.success());
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
fn pre_receive_binary_succeeds_with_env_config() {
    let updates_path = write_updates_file(&nostr_updates_input());
    let output = run_hook_binary_with_env(
        env!("CARGO_BIN_EXE_gittree-pre-receive"),
        &[],
        &[
            ("GITTREE_STATE_URL", "http://127.0.0.1:8082".to_string()),
            ("GITTREE_HOOK_REPO_PATH", sample_repo_path()),
            (
                "GITTREE_HOOK_STDIN_FILE",
                updates_path.display().to_string(),
            ),
        ],
    );
    std::fs::remove_file(&updates_path).ok();
    assert!(output.status.success());
}

#[test]
fn pre_receive_binary_succeeds_with_piped_stdin() {
    let output = run_hook_binary_with_env_and_stdin(
        env!("CARGO_BIN_EXE_gittree-pre-receive"),
        &[],
        &[
            ("GITTREE_STATE_URL", "http://127.0.0.1:8082".to_string()),
            ("GITTREE_HOOK_REPO_PATH", sample_repo_path()),
        ],
        &nostr_updates_input(),
    );
    assert!(output.status.success());
}

#[test]
fn post_receive_binary_reports_missing_config() {
    let output = run_hook_binary(env!("CARGO_BIN_EXE_gittree-post-receive"), &[]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("git hook failed:"));
    assert!(stderr.contains("hook config error:"));
}

#[test]
fn post_receive_binary_succeeds_with_env_config() {
    let updates_path = write_updates_file(&nostr_updates_input());
    let (sync_url, handle) = start_sync_server();
    let output = run_hook_binary_with_env(
        env!("CARGO_BIN_EXE_gittree-post-receive"),
        &[],
        &[
            ("GITTREE_STATE_URL", "http://127.0.0.1:8082".to_string()),
            ("GITTREE_SYNC_URL", sync_url),
            ("GITTREE_HOOK_REPO_PATH", sample_repo_path()),
            (
                "GITTREE_HOOK_STDIN_FILE",
                updates_path.display().to_string(),
            ),
        ],
    );
    handle.join().expect("sync server join");
    std::fs::remove_file(&updates_path).ok();
    assert!(output.status.success());
}
