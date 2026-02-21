use clap::Parser;
use gittree_git_hook::{HookServiceError, run_hook};
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
    if exit_code != 0 {
        std::process::exit(exit_code);
    }
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

#[cfg(test)]
mod tests {
    use super::{HookCli, HookServiceError, handle_main_result, run_with_cli, try_main};
    use clap::Parser;
    use std::path::PathBuf;

    fn noop_run() -> Result<(), HookServiceError> {
        Ok(())
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
        assert!(matches!(err, HookServiceError::Core(_)));
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
}
