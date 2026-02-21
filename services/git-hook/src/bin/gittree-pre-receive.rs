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
    if exit_code != 0 {
        std::process::exit(exit_code);
    }
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

#[cfg(test)]
mod tests {
    use super::{HookMode, handle_main_result, run_with};

    fn noop_hook(_mode: HookMode) -> Result<(), gittree_git_hook::HookServiceError> {
        Ok(())
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
}
