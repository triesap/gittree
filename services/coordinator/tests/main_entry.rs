use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

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

#[test]
fn coordinator_binary_invalid_bind_exits_with_config_error() {
    let output = Command::new(env!("CARGO_BIN_EXE_gittree-coordinator"))
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
        .env("GITTREE_FORGEJO_WEBHOOK_SECRET", "secret")
        .output()
        .expect("run coordinator binary");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("coordinator service failed:"));
    assert!(stderr.contains("invalid coordinator bind address"));
}

#[test]
fn coordinator_binary_occupied_bind_exits_with_serve_error() {
    let occupied = std::net::TcpListener::bind("127.0.0.1:0").expect("bind occupied listener");
    let bind = occupied.local_addr().expect("occupied addr").to_string();

    let output = Command::new(env!("CARGO_BIN_EXE_gittree-coordinator"))
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
        .env("GITTREE_FORGEJO_WEBHOOK_SECRET", "secret")
        .output()
        .expect("run coordinator binary");

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

    let scenarios = [
        (
            Some(("GITTREE_STORAGE_MAX_CONNECTIONS", Some("not-a-number"))),
            "coordinator storage config error",
        ),
        (
            Some(("GITTREE_RELAY_URLS", Some("not-a-url"))),
            "coordinator config error",
        ),
        (
            Some(("GITTREE_COORDINATOR_REPO_ROOT", Some("   "))),
            "invalid env GITTREE_COORDINATOR_REPO_ROOT",
        ),
        (
            Some(("GITTREE_COORDINATOR_PRE_RECEIVE_HOOK", Some("   "))),
            "invalid env GITTREE_COORDINATOR_PRE_RECEIVE_HOOK",
        ),
        (
            Some(("GITTREE_COORDINATOR_POST_RECEIVE_HOOK", Some("   "))),
            "invalid env GITTREE_COORDINATOR_POST_RECEIVE_HOOK",
        ),
        (
            Some(("GITTREE_FORGEJO_BASE_URL", Some("not-a-url"))),
            "coordinator config error",
        ),
    ];

    for (override_env, expected_stderr) in scenarios {
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
        if let Some((key, value)) = override_env {
            if let Some(value) = value {
                command.env(key, value);
            } else {
                command.env_remove(key);
            }
        }

        let output = command.output().expect("run coordinator binary");
        assert!(!output.status.success());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("coordinator service failed:"));
        assert!(stderr.contains(expected_stderr));
    }

    let _ = std::fs::remove_dir_all(runtime_dir);
}
