use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

fn run_migrate_with_env(envs: &[(&str, &str)]) -> Output {
    let run_dir = std::env::temp_dir().join(format!(
        "gittree-migrate-main-entry-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    ));
    std::fs::create_dir_all(&run_dir).expect("create temp run dir");
    let mut command = Command::new(env!("CARGO_BIN_EXE_gittree-migrate"));
    command.current_dir(&run_dir);
    for (key, value) in envs {
        command.env(key, value);
    }
    let output = command.output().expect("run migrate binary");
    std::fs::remove_dir_all(&run_dir).ok();
    output
}

#[test]
fn migrate_binary_invalid_storage_url_exits_with_error() {
    let output = run_migrate_with_env(&[("GITTREE_STORAGE_READ_URL", "://invalid")]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("migration failed:"));
}

#[test]
fn migrate_binary_invalid_observability_env_exits_with_observability_error() {
    let output = run_migrate_with_env(&[
        ("GITTREE_STORAGE_READ_URL", "://invalid"),
        ("GITTREE_LOG_JSON", "not-a-bool"),
    ]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("migration observability failed:"));
    assert!(stderr.contains("invalid env GITTREE_LOG_JSON"));
}

#[test]
fn migrate_binary_invalid_log_dir_exits_with_observability_error() {
    let output = run_migrate_with_env(&[
        ("GITTREE_STORAGE_READ_URL", "://invalid"),
        ("GITTREE_LOG_DIR", "/dev/null/gittree-migrate"),
    ]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("migration observability failed:"));
    assert!(stderr.contains("observability log init failed"));
}
