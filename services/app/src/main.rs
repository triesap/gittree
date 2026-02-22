#![forbid(unsafe_code)]

use gittree_app::{AppError, AppServiceConfig, serve};
use std::future::Future;
use std::io::Write;

#[tokio::main]
async fn main() {
    let mut stderr = std::io::stderr();
    let exit_code = main_impl(&mut stderr).await;
    let mut exit_process = |code| {
        std::process::exit(code);
    };
    exit_if_needed(exit_code, &mut exit_process);
}

async fn main_impl(stderr: &mut impl Write) -> i32 {
    main_impl_with(
        || {
            dotenvy::dotenv().ok();
        },
        run,
        stderr,
    )
    .await
}

async fn main_impl_with<DotenvFn, RunFn, RunFut>(
    load_dotenv: DotenvFn,
    run_fn: RunFn,
    stderr: &mut impl Write,
) -> i32
where
    DotenvFn: FnOnce(),
    RunFn: FnOnce() -> RunFut,
    RunFut: Future<Output = Result<(), AppError>>,
{
    load_dotenv();
    handle_main_result(run_fn().await, stderr)
}

async fn run() -> Result<(), AppError> {
    run_with(
        || AppServiceConfig::from_env().map_err(AppError::Config),
        serve,
    )
    .await
}

async fn run_with<Config, LoadFn, ServeFn, ServeFut>(
    load_config: LoadFn,
    serve_fn: ServeFn,
) -> Result<(), AppError>
where
    LoadFn: FnOnce() -> Result<Config, AppError>,
    ServeFn: FnOnce(Config) -> ServeFut,
    ServeFut: Future<Output = Result<(), AppError>>,
{
    let config = load_config()?;
    serve_fn(config).await
}

fn handle_main_result(result: Result<(), AppError>, stderr: &mut impl Write) -> i32 {
    match result {
        Ok(()) => 0,
        Err(err) => {
            let _ = writeln!(stderr, "app service failed: {err}");
            1
        }
    }
}

fn exit_if_needed(exit_code: i32, exit: &mut dyn FnMut(i32)) {
    if exit_code != 0 {
        exit(exit_code);
    }
}

#[cfg(test)]
mod tests {
    use super::{exit_if_needed, handle_main_result, main_impl_with, run_with};
    use gittree_app::AppError;

    async fn serve_should_not_run(_: ()) -> Result<(), AppError> {
        panic!("serve should not run when config loading fails");
    }

    fn noop_exit(_: i32) {}

    #[tokio::test]
    async fn run_with_returns_config_errors() {
        let err = run_with(
            || Err::<(), AppError>(AppError::Serve("config failed".to_string())),
            serve_should_not_run,
        )
        .await
        .expect_err("config error");
        assert!(matches!(err, AppError::Serve(message) if message == "config failed"));
    }

    #[tokio::test]
    async fn run_with_returns_serve_errors() {
        let err = run_with(
            || Ok::<_, AppError>("config"),
            |_| async { Err::<(), AppError>(AppError::Serve("serve failed".to_string())) },
        )
        .await
        .expect_err("serve error");
        assert!(matches!(err, AppError::Serve(message) if message == "serve failed"));
    }

    #[tokio::test]
    async fn run_with_succeeds_when_serve_succeeds() {
        let result = run_with(
            || Ok::<_, AppError>("config"),
            |_| async { Ok::<(), AppError>(()) },
        )
        .await;
        assert!(result.is_ok());
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
        let exit_code = handle_main_result(Err(AppError::Serve("boom".to_string())), &mut stderr);
        assert_eq!(exit_code, 1);
        assert_eq!(
            String::from_utf8(stderr).expect("utf8"),
            "app service failed: app serve error: boom\n"
        );
    }

    #[tokio::test]
    async fn main_impl_with_reports_errors() {
        let mut stderr = Vec::new();

        let exit_code = main_impl_with(
            || {},
            || async { Err::<(), AppError>(AppError::Serve("boom".to_string())) },
            &mut stderr,
        )
        .await;

        assert_eq!(exit_code, 1);
        let message = String::from_utf8(stderr).expect("utf8");
        assert!(message.contains("app serve error: boom"));
    }

    #[test]
    fn exit_if_needed_skips_zero_exit_code() {
        noop_exit(0);
        let mut exit = noop_exit;
        exit_if_needed(0, &mut exit);
    }

    #[test]
    fn exit_if_needed_forwards_non_zero_exit_code() {
        let mut observed = None;
        let mut exit = |code| observed = Some(code);
        exit_if_needed(7, &mut exit);
        assert_eq!(observed, Some(7));
    }

    #[tokio::test]
    #[should_panic(expected = "serve should not run when config loading fails")]
    async fn serve_should_not_run_panics_when_called() {
        let _ = serve_should_not_run(()).await;
    }

    #[tokio::test]
    async fn main_impl_with_executes_loader() {
        let called = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let loader_called = called.clone();
        let mut stderr = Vec::new();

        let _ = main_impl_with(
            move || {
                loader_called.store(true, std::sync::atomic::Ordering::Relaxed);
            },
            || async { Ok::<(), AppError>(()) },
            &mut stderr,
        )
        .await;

        assert!(called.load(std::sync::atomic::Ordering::Relaxed));
    }
}
