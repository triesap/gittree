use std::process::Command;
use std::process::Stdio;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

fn new_runtime_dir() -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "gittree-coordinator-main-entry-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    ))
}

fn write_hook_source(path: &std::path::Path, name: &str) -> std::path::PathBuf {
    let hook_path = path.join(name);
    std::fs::write(&hook_path, "#!/bin/sh\nexit 0\n").expect("write hook");
    hook_path
}

fn command_output_with_timeout(
    mut command: Command,
    timeout: Duration,
) -> std::process::Output {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command.spawn().expect("spawn coordinator binary");
    let deadline = Instant::now() + timeout;
    loop {
        if child.try_wait().expect("check coordinator status").is_some() {
            return child.wait_with_output().expect("read coordinator output");
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let output = child.wait_with_output().expect("read timeout output");
            panic!(
                "coordinator binary timed out after {:?}; stdout: {}; stderr: {}",
                timeout,
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn coordinator_binary_invalid_bind_exits_with_config_error() {
    let mut command = Command::new(env!("CARGO_BIN_EXE_gittree-coordinator"));
    command
        .env("GITTREE_COORDINATOR_BIND", "not-a-socket")
        .env(
            "GITTREE_STORAGE_READ_URL",
            "postgres://user:pass@localhost:5432/gittree",
        )
        .env("GITTREE_COORDINATOR_REPO_ROOT", "/tmp/gittree")
        .env("GITTREE_COORDINATOR_PRE_RECEIVE_HOOK", "/tmp/pre-receive")
        .env("GITTREE_COORDINATOR_POST_RECEIVE_HOOK", "/tmp/post-receive")
        .env("GITTREE_FORGEJO_BASE_URL", "http://localhost:3000")
        .env("GITTREE_FORGEJO_API_TOKEN", "token")
        .env("GITTREE_FORGEJO_OWNER", "owner")
        .env(
            "GITTREE_FORGEJO_WEBHOOK_URL",
            "http://localhost:3000/webhook",
        )
        .env("GITTREE_FORGEJO_WEBHOOK_SECRET", "secret");
    let output = command_output_with_timeout(command, Duration::from_secs(15));

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("coordinator service failed:"));
    assert!(stderr.contains("invalid coordinator bind address"));
}

#[test]
fn coordinator_binary_occupied_bind_exits_with_serve_error() {
    let occupied = std::net::TcpListener::bind("127.0.0.1:0").expect("bind occupied listener");
    let bind = occupied.local_addr().expect("occupied addr").to_string();

    let mut command = Command::new(env!("CARGO_BIN_EXE_gittree-coordinator"));
    command
        .env("GITTREE_COORDINATOR_BIND", bind)
        .env(
            "GITTREE_STORAGE_READ_URL",
            "postgres://user:pass@localhost:5432/gittree",
        )
        .env("GITTREE_COORDINATOR_REPO_ROOT", "/tmp/gittree")
        .env("GITTREE_COORDINATOR_PRE_RECEIVE_HOOK", "/tmp/pre-receive")
        .env("GITTREE_COORDINATOR_POST_RECEIVE_HOOK", "/tmp/post-receive")
        .env("GITTREE_FORGEJO_BASE_URL", "http://localhost:3000")
        .env("GITTREE_FORGEJO_API_TOKEN", "token")
        .env("GITTREE_FORGEJO_OWNER", "owner")
        .env(
            "GITTREE_FORGEJO_WEBHOOK_URL",
            "http://localhost:3000/webhook",
        )
        .env("GITTREE_FORGEJO_WEBHOOK_SECRET", "secret");
    let output = command_output_with_timeout(command, Duration::from_secs(15));

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("coordinator service failed:"));
    assert!(stderr.contains("coordinator serve error"));
}

#[test]
fn coordinator_binary_config_errors_cover_from_env_paths() {
    let runtime_dir = new_runtime_dir();
    std::fs::create_dir_all(&runtime_dir).expect("create runtime dir");
    let repo_root = runtime_dir.join("repos");
    std::fs::create_dir_all(&repo_root).expect("create repo root");
    let pre_hook = write_hook_source(&runtime_dir, "pre-receive");
    let post_hook = write_hook_source(&runtime_dir, "post-receive");

    let scenarios: &[(&[(&str, Option<&str>)], &str)] = &[
        (
            &[("GITTREE_STORAGE_MAX_CONNECTIONS", Some("not-a-number"))],
            "coordinator storage config error",
        ),
        (
            &[("GITTREE_STORAGE_MIN_CONNECTIONS", Some("not-a-number"))],
            "coordinator storage config error",
        ),
        (
            &[("GITTREE_STORAGE_IDLE_TIMEOUT_SECS", Some("not-a-number"))],
            "coordinator storage config error",
        ),
        (
            &[("GITTREE_STORAGE_MAX_LIFETIME_SECS", Some("not-a-number"))],
            "coordinator storage config error",
        ),
        (
            &[
                ("GITTREE_STORAGE_MAX_CONNECTIONS", Some("1")),
                ("GITTREE_STORAGE_MIN_CONNECTIONS", Some("2")),
            ],
            "coordinator storage config error",
        ),
        (&[("GITTREE_RELAY_URLS", Some("not-a-url"))], "coordinator config error"),
        (
            &[("GITTREE_COORDINATOR_REPO_ROOT", Some("   "))],
            "invalid env GITTREE_COORDINATOR_REPO_ROOT",
        ),
        (
            &[("GITTREE_COORDINATOR_PRE_RECEIVE_HOOK", Some("   "))],
            "invalid env GITTREE_COORDINATOR_PRE_RECEIVE_HOOK",
        ),
        (
            &[("GITTREE_COORDINATOR_POST_RECEIVE_HOOK", Some("   "))],
            "invalid env GITTREE_COORDINATOR_POST_RECEIVE_HOOK",
        ),
        (
            &[("GITTREE_FORGEJO_BASE_URL", Some("not-a-url"))],
            "coordinator config error",
        ),
    ];

    for (override_envs, expected_stderr) in scenarios {
        let mut command = Command::new(env!("CARGO_BIN_EXE_gittree-coordinator"));
        command
            .env("GITTREE_COORDINATOR_BIND", "127.0.0.1:9091")
            .env(
                "GITTREE_STORAGE_READ_URL",
                "postgres://user:pass@localhost:5432/gittree",
            )
            .env("GITTREE_RELAY_URLS", "wss://relay.example")
            .env("GITTREE_COORDINATOR_REPO_ROOT", &repo_root)
            .env("GITTREE_COORDINATOR_PRE_RECEIVE_HOOK", &pre_hook)
            .env("GITTREE_COORDINATOR_POST_RECEIVE_HOOK", &post_hook)
            .env("GITTREE_FORGEJO_BASE_URL", "http://localhost:3000")
            .env("GITTREE_FORGEJO_API_TOKEN", "token")
            .env("GITTREE_FORGEJO_OWNER", "owner")
            .env(
                "GITTREE_FORGEJO_WEBHOOK_URL",
                "http://localhost:3000/webhook",
            )
            .env("GITTREE_FORGEJO_WEBHOOK_SECRET", "secret");
        for (key, value) in *override_envs {
            if let Some(value) = value {
                command.env(key, value);
            } else {
                command.env_remove(key);
            }
        }

        let output = command_output_with_timeout(command, Duration::from_secs(15));
        assert!(!output.status.success());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("coordinator service failed:"));
        assert!(
            stderr.contains(expected_stderr),
            "missing expected stderr for overrides {:?}: {}",
            override_envs,
            stderr
        );
    }

    let _ = std::fs::remove_dir_all(runtime_dir);
}
