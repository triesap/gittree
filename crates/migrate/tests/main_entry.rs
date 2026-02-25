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
    for key in [
        "GITTREE_STORAGE_READ_URL",
        "GITTREE_STORAGE_WRITE_URL",
        "GITTREE_STORAGE_MAX_CONNECTIONS",
        "GITTREE_STORAGE_MIN_CONNECTIONS",
        "GITTREE_STORAGE_IDLE_TIMEOUT_SECS",
        "GITTREE_STORAGE_MAX_LIFETIME_SECS",
        "GITTREE_STORAGE_APPLICATION_NAME",
        "GITTREE_LOG_JSON",
        "GITTREE_LOG_DIR",
    ] {
        command.env_remove(key);
    }
    for (key, value) in envs {
        command.env(key, value);
    }
    let output = command.output().expect("run migrate binary");
    std::fs::remove_dir_all(&run_dir).ok();
    output
}

fn push_unique_candidate(candidates: &mut Vec<String>, value: Option<String>) {
    if let Some(value) = value {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return;
        }
        if candidates.iter().any(|candidate| candidate == trimmed) {
            return;
        }
        candidates.push(trimmed.to_string());
    }
}

fn migration_test_database_candidates() -> Vec<String> {
    let mut candidates = Vec::new();
    push_unique_candidate(
        &mut candidates,
        std::env::var("GITTREE_STORAGE_TEST_DATABASE_URL").ok(),
    );
    push_unique_candidate(
        &mut candidates,
        std::env::var("GITTREE_STORAGE_WRITE_URL").ok(),
    );
    push_unique_candidate(
        &mut candidates,
        std::env::var("GITTREE_STORAGE_READ_URL").ok(),
    );
    push_unique_candidate(
        &mut candidates,
        Some("postgres://gittree:gittree@127.0.0.1:5432/gittree".to_string()),
    );
    candidates
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

#[test]
fn migrate_binary_succeeds_with_reachable_database() {
    for database_url in migration_test_database_candidates() {
        let output = run_migrate_with_env(&[
            ("GITTREE_STORAGE_READ_URL", &database_url),
            ("GITTREE_STORAGE_WRITE_URL", &database_url),
            ("GITTREE_METRICS_ENABLED", "false"),
            ("GITTREE_LOG_STDOUT", "false"),
        ]);
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            assert!(stdout.contains("migrations complete: version"));
            return;
        }
    }
}
