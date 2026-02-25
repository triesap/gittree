use clap::Parser;
use gittree_git_hook::{run_hook, HookServiceError};
use std::io::Write;
use std::path::Path;

mod cli;

use cli::{HookCli, HookRunConfig};

fn init_observability() -> Result<gittree_observability::ObservabilityHandle, String> {
    let config = gittree_observability::ObservabilityConfig::from_env("gittree-git-hook")
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
    handle_main_result(try_main(init_observability, run), stderr)
}

fn run() -> Result<(), HookServiceError> {
    run_with_cli(HookCli::parse(), run_hook)
}

fn run_with_cli<F>(cli: HookCli, run_hook_fn: F) -> Result<(), HookServiceError>
where
    F: FnOnce(gittree_git_hook::HookConfig, Option<&Path>) -> Result<(), HookServiceError>,
{
    let config = HookRunConfig::from_env(cli).map_err(HookServiceError::Config)?;
    run_hook_fn(config.hook, config.stdin_file.as_deref())
}

fn try_main<FInit, FRun, T>(init_observability_fn: FInit, run_fn: FRun) -> Result<(), String>
where
    FInit: FnOnce() -> Result<T, String>,
    FRun: FnOnce() -> Result<(), HookServiceError>,
{
    let _observability =
        init_observability_fn().map_err(|err| format!("git hook observability failed: {err}"))?;
    run_fn().map_err(|err| format!("git hook failed: {err}"))
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
    use super::{
        exit_if_needed, handle_main_result, init_observability, run_with_cli, try_main, HookCli,
        HookServiceError,
    };
    use clap::Parser;
    use std::path::PathBuf;
    use std::sync::{Mutex, OnceLock};

    fn noop_run() -> Result<(), HookServiceError> {
        Ok(())
    }

    fn noop_exit(_code: i32) {}

    fn hook_service_error_kind(err: &HookServiceError) -> &'static str {
        match err {
            HookServiceError::Config(_) => "config",
            HookServiceError::Parse(_) => "parse",
            HookServiceError::Core(_) => "core",
            HookServiceError::State(_) => "state",
            HookServiceError::Reject(_) => "reject",
        }
    }

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
    fn try_main_reports_observability_error() {
        let err = try_main(|| Err::<(), _>("obs boom".to_string()), noop_run)
            .expect_err("expected error");
        assert!(err.contains("git hook observability failed"));
    }

    #[test]
    fn try_main_reports_hook_error() {
        let err = try_main(
            || Ok::<(), String>(()),
            || Err(HookServiceError::Core("hook boom".to_string())),
        )
        .expect_err("expected error");
        assert!(err.contains("git hook failed"));
    }

    #[test]
    fn try_main_returns_ok_on_success() {
        let result = try_main(|| Ok::<(), String>(()), noop_run);
        assert!(result.is_ok());
    }

    #[test]
    fn run_with_cli_propagates_runner_error() {
        let cli = HookCli::try_parse_from([
            "gittree-git-hook",
            "--mode",
            "pre-receive",
            "--state-url",
            "http://127.0.0.1:8082",
            "--sync-url",
            "http://127.0.0.1:8088",
        ])
        .expect("parse");
        let err = run_with_cli(cli, |_, _| {
            Err(HookServiceError::Core("runner boom".to_string()))
        })
        .expect_err("runner should fail");
        assert_eq!(hook_service_error_kind(&err), "core");
    }

    #[test]
    fn hook_service_error_kind_covers_all_variants() {
        let config_err = HookServiceError::Config(gittree_git_hook::HookConfigError::MissingEnv(
            "GITTREE_STATE_URL",
        ));
        assert_eq!(hook_service_error_kind(&config_err), "config");

        let parse_err =
            HookServiceError::Parse(gittree_git_hook::HookError::InvalidPayload("bad".to_string()));
        assert_eq!(hook_service_error_kind(&parse_err), "parse");

        let state_err = HookServiceError::State("state".to_string());
        assert_eq!(hook_service_error_kind(&state_err), "state");

        let reject_err = HookServiceError::Reject("reject".to_string());
        assert_eq!(hook_service_error_kind(&reject_err), "reject");
    }

    #[test]
    fn run_with_cli_passes_stdin_file_to_runner() {
        let cli = HookCli::try_parse_from([
            "gittree-git-hook",
            "--mode",
            "pre-receive",
            "--state-url",
            "http://127.0.0.1:8082",
            "--sync-url",
            "http://127.0.0.1:8088",
            "--stdin-file",
            "updates.txt",
        ])
        .expect("parse");
        let mut seen_path: Option<PathBuf> = None;
        run_with_cli(cli, |_, stdin_file| {
            seen_path = stdin_file.map(|path| path.to_path_buf());
            Ok(())
        })
        .expect("runner");
        assert_eq!(
            seen_path.as_deref(),
            Some(std::path::Path::new("updates.txt"))
        );
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
            let result = init_observability();
            assert!(result.is_err());
        });
    }

    #[test]
    fn init_observability_invokes_runtime_init_path() {
        with_env_var("GITTREE_LOG_JSON", Some("false"), &mut || {
            let _ = init_observability();
        });
    }

    #[test]
    fn init_observability_second_call_reports_error_path() {
        with_env_var("GITTREE_LOG_JSON", Some("false"), &mut || {
            let _ = init_observability();
            let second = init_observability();
            assert!(second.is_err());
        });
    }

    #[test]
    fn with_env_var_covers_none_and_restore_branches() {
        const KEY: &str = "GITTREE_TEST_MAIN_HOOK_ENV";

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
