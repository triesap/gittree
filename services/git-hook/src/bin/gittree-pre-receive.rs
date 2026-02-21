use gittree_git_hook::{HookMode, run_hook_from_env};
use std::io::Write;

fn init_observability(service: &str) -> Result<gittree_observability::ObservabilityHandle, String> {
    let config = gittree_observability::ObservabilityConfig::from_env(service)
        .map_err(|err| err.to_string())?;
    gittree_observability::init(&config).map_err(|err| err.to_string())
}

fn main() {
    let mut stderr = std::io::stderr();
    let exit_code = main_impl(&mut stderr);
    exit_if_needed(exit_code, std::process::exit);
}

fn main_impl(stderr: &mut impl Write) -> i32 {
    dotenvy::dotenv().ok();
    let result = run_with(
        "gittree-pre-receive",
        HookMode::PreReceive,
        init_observability,
        run_hook_from_env,
    );
    handle_main_result(result, stderr)
}

fn run_with<T, FInit, FHook>(
    service: &str,
    mode: HookMode,
    init_observability_fn: FInit,
    run_hook_fn: FHook,
) -> Result<(), String>
where
    FInit: FnOnce(&str) -> Result<T, String>,
    FHook: FnOnce(HookMode) -> Result<(), gittree_git_hook::HookServiceError>,
{
    let _observability = init_observability_fn(service)
        .map_err(|err| format!("git hook observability failed: {err}"))?;
    run_hook_fn(mode).map_err(|err| format!("git hook failed: {err}"))
}

fn handle_main_result(result: Result<(), String>, stderr: &mut impl Write) -> i32 {
    match result {
        Ok(()) => 0,
        Err(err) => {
            let _ = writeln!(stderr, "{err}");
            1
        }
    }
}

fn exit_if_needed<F, R>(exit_code: i32, exit_fn: F)
where
    F: FnOnce(i32) -> R,
{
    if exit_code != 0 {
        let _ = exit_fn(exit_code);
    }
}

#[cfg(test)]
mod tests {
    use super::{HookMode, exit_if_needed, handle_main_result, init_observability, run_with};
    use std::sync::{Mutex, OnceLock};

    fn noop_hook(_mode: HookMode) -> Result<(), gittree_git_hook::HookServiceError> {
        Ok(())
    }

    fn noop_exit(_code: i32) {}

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    fn with_env_var<F>(key: &str, value: Option<&str>, run: F)
    where
        F: FnOnce(),
    {
        let _guard = env_lock().lock().expect("lock env");
        let previous = std::env::var(key).ok();
        match value {
            // SAFETY: tests mutate process env under a global lock and always restore state.
            Some(value) => unsafe { std::env::set_var(key, value) },
            // SAFETY: tests mutate process env under a global lock and always restore state.
            None => unsafe { std::env::remove_var(key) },
        }
        run();
        if let Some(previous) = previous {
            // SAFETY: tests mutate process env under a global lock and always restore state.
            unsafe { std::env::set_var(key, previous) };
        } else {
            // SAFETY: tests mutate process env under a global lock and always restore state.
            unsafe { std::env::remove_var(key) };
        }
    }

    #[test]
    fn run_with_returns_ok_on_success() {
        let result = run_with(
            "svc",
            HookMode::PreReceive,
            |_| Ok::<(), String>(()),
            noop_hook,
        );
        assert!(result.is_ok());
    }

    #[test]
    fn run_with_reports_observability_error() {
        let err = run_with(
            "svc",
            HookMode::PreReceive,
            |_| Err::<(), _>("obs boom".to_string()),
            noop_hook,
        )
        .expect_err("expected error");
        assert!(err.contains("git hook observability failed"));
    }

    #[test]
    fn run_with_reports_hook_error() {
        let err = run_with(
            "svc",
            HookMode::PreReceive,
            |_| Ok::<(), String>(()),
            |_| {
                Err(gittree_git_hook::HookServiceError::Core(
                    "hook boom".to_string(),
                ))
            },
        )
        .expect_err("expected error");
        assert!(err.contains("git hook failed"));
    }

    #[test]
    fn handle_main_result_returns_zero_on_success() {
        let mut stderr = Vec::new();
        let exit_code = handle_main_result(Ok(()), &mut stderr);
        assert_eq!(exit_code, 0);
        assert!(stderr.is_empty());
    }

    #[test]
    fn handle_main_result_writes_error_on_failure() {
        let mut stderr = Vec::new();
        let exit_code = handle_main_result(Err("boom".to_string()), &mut stderr);
        assert_eq!(exit_code, 1);
        assert_eq!(String::from_utf8(stderr).expect("utf8"), "boom\n");
    }

    #[test]
    fn noop_exit_is_noop() {
        noop_exit(0);
    }

    #[test]
    fn init_observability_reports_invalid_log_env() {
        with_env_var("GITTREE_LOG_JSON", Some("invalid-bool"), || {
            let result = init_observability("gittree-pre-receive");
            assert!(result.is_err());
        });
    }

    #[test]
    fn init_observability_invokes_runtime_init_path() {
        with_env_var("GITTREE_LOG_JSON", Some("false"), || {
            let _ = init_observability("gittree-pre-receive");
        });
    }

    #[test]
    fn init_observability_second_call_reports_error_path() {
        with_env_var("GITTREE_LOG_JSON", Some("false"), || {
            let _ = init_observability("gittree-pre-receive");
            let second = init_observability("gittree-pre-receive");
            assert!(second.is_err());
        });
    }

    #[test]
    fn with_env_var_covers_none_and_restore_branches() {
        const KEY: &str = "GITTREE_TEST_PRE_HOOK_ENV";

        // SAFETY: test-only env mutation for a unique key.
        unsafe { std::env::set_var(KEY, "before") };
        with_env_var(KEY, None, || {
            assert!(std::env::var(KEY).is_err());
        });
        assert_eq!(std::env::var(KEY).expect("restored"), "before");
        // SAFETY: test-only env cleanup for a unique key.
        unsafe { std::env::remove_var(KEY) };

        with_env_var(KEY, None, || {
            assert!(std::env::var(KEY).is_err());
        });
        assert!(std::env::var(KEY).is_err());
    }

    #[test]
    fn exit_if_needed_skips_exit_when_code_is_zero() {
        exit_if_needed(0, noop_exit);
    }

    #[test]
    fn exit_if_needed_calls_exit_when_code_is_non_zero() {
        let mut seen = None;
        exit_if_needed(17, |code| seen = Some(code));
        assert_eq!(seen, Some(17));
    }
}
