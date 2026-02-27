use gittree_git_hook::{HookMode, run_hook_from_env};
use std::io::Write;
use std::process::ExitCode;

fn init_observability(service: &str) -> Result<gittree_observability::ObservabilityHandle, String> {
    let config = match gittree_observability::ObservabilityConfig::from_env(service) {
        Ok(config) => config,
        Err(err) => return Err(err.to_string()),
    };
    match gittree_observability::init(&config) {
        Ok(handle) => Ok(handle),
        Err(err) => Err(err.to_string()),
    }
}

fn init_observability_unit(service: &str) -> Result<(), String> {
    let _ = init_observability(service)?;
    Ok(())
}

fn main() -> ExitCode {
    let mut stderr = std::io::stderr();
    let exit_code = main_impl(&mut stderr);
    if exit_code == 0 {
        ExitCode::SUCCESS
    } else {
        exit_status(exit_code)
    }
}

fn main_impl(stderr: &mut impl Write) -> i32 {
    dotenvy::dotenv().ok();
    let result = run_with(
        "gittree-post-receive",
        HookMode::PostReceive,
        init_observability_unit,
        run_hook_from_env,
    );
    handle_main_result(result, stderr)
}

fn run_with(
    service: &str,
    mode: HookMode,
    init_observability_fn: fn(&str) -> Result<(), String>,
    run_hook_fn: fn(HookMode) -> Result<(), gittree_git_hook::HookServiceError>,
) -> Result<(), String>
{
    let _observability = match init_observability_fn(service) {
        Ok(observability) => observability,
        Err(err) => return Err(format!("git hook observability failed: {err}")),
    };
    match run_hook_fn(mode) {
        Ok(()) => Ok(()),
        Err(err) => Err(format!("git hook failed: {err}")),
    }
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

fn exit_status(exit_code: i32) -> ExitCode {
    if exit_code == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(exit_code.clamp(1, u8::MAX as i32) as u8)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        HookMode, exit_status, handle_main_result, init_observability, run_with,
    };
    use std::sync::{Mutex, OnceLock};

    fn noop_hook(_mode: HookMode) -> Result<(), gittree_git_hook::HookServiceError> {
        Ok(())
    }

    fn noop_exit(_code: i32) {}

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    fn with_env_var(key: &str, value: Option<&str>, run: &mut dyn FnMut()) {
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
            HookMode::PostReceive,
            |_| Ok::<(), String>(()),
            noop_hook,
        );
        assert!(result.is_ok());
    }

    #[test]
    fn run_with_reports_observability_error() {
        let err = run_with(
            "svc",
            HookMode::PostReceive,
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
            HookMode::PostReceive,
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
        with_env_var("GITTREE_LOG_JSON", Some("invalid-bool"), &mut || {
            let result = init_observability("gittree-post-receive");
            assert!(result.is_err());
        });
    }

    #[test]
    fn init_observability_invokes_runtime_init_path() {
        with_env_var("GITTREE_LOG_JSON", Some("false"), &mut || {
            let _ = init_observability("gittree-post-receive");
        });
    }

    #[test]
    fn init_observability_second_call_reports_error_path() {
        with_env_var("GITTREE_LOG_JSON", Some("false"), &mut || {
            let _ = init_observability("gittree-post-receive");
            let second = init_observability("gittree-post-receive");
            assert!(second.is_err());
        });
    }

    #[test]
    fn with_env_var_covers_none_and_restore_branches() {
        const KEY: &str = "GITTREE_TEST_POST_HOOK_ENV";

        // SAFETY: test-only env mutation for a unique key.
        unsafe { std::env::set_var(KEY, "before") };
        with_env_var(KEY, None, &mut || {
            assert!(std::env::var(KEY).is_err());
        });
        assert_eq!(std::env::var(KEY).expect("restored"), "before");
        // SAFETY: test-only env cleanup for a unique key.
        unsafe { std::env::remove_var(KEY) };

        with_env_var(KEY, None, &mut || {
            assert!(std::env::var(KEY).is_err());
        });
        assert!(std::env::var(KEY).is_err());
    }

    #[test]
    fn exit_status_maps_codes() {
        assert_eq!(exit_status(0), std::process::ExitCode::SUCCESS);
        assert_eq!(exit_status(1), std::process::ExitCode::from(1));
        assert_eq!(exit_status(999), std::process::ExitCode::from(u8::MAX));
        assert_eq!(exit_status(-1), std::process::ExitCode::from(1));
    }

    #[test]
    fn main_paths_cover_stderr_instantiation() {
        with_env_var("GITTREE_SYNC_URL", None, &mut || {
            let mut stderr = std::io::stderr();
            assert_eq!(super::main_impl(&mut stderr), 1);
            assert_eq!(super::main(), std::process::ExitCode::from(1));
        });
    }
}
