use std::process::Command;

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
