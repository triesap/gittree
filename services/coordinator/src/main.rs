use gittree_coordinator::{CoordinatorConfig, CoordinatorError, serve};
use std::future::Future;
use std::io::Write;

#[tokio::main]
async fn main() {
    let mut stderr = std::io::stderr();
    let exit_code = main_impl(&mut stderr).await;
    exit_if_needed(exit_code, std::process::exit);
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
    RunFut: Future<Output = Result<(), CoordinatorError>>,
{
    load_dotenv();
    handle_main_result(run_fn().await, stderr)
}

async fn run() -> Result<(), CoordinatorError> {
    run_with(
        || CoordinatorConfig::from_env().map_err(CoordinatorError::Config),
        serve,
    )
    .await
}

async fn run_with<Config, LoadFn, ServeFn, ServeFut>(
    load_config: LoadFn,
    serve_fn: ServeFn,
) -> Result<(), CoordinatorError>
where
    LoadFn: FnOnce() -> Result<Config, CoordinatorError>,
    ServeFn: FnOnce(Config) -> ServeFut,
    ServeFut: Future<Output = Result<(), CoordinatorError>>,
{
    let config = load_config()?;
    serve_fn(config).await
}

fn handle_main_result(result: Result<(), CoordinatorError>, stderr: &mut impl Write) -> i32 {
    match result {
        Ok(()) => 0,
        Err(err) => {
            let _ = writeln!(stderr, "coordinator service failed: {err}");
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
    use super::{exit_if_needed, handle_main_result, main_impl_with, run_with};
    use gittree_coordinator::CoordinatorError;

    async fn serve_ok(_: ()) -> Result<(), CoordinatorError> {
        Ok(())
    }

    #[tokio::test]
    async fn run_with_returns_config_errors() {
        let err = run_with(
            || Err::<(), CoordinatorError>(CoordinatorError::Serve("config failed".to_string())),
            serve_ok,
        )
        .await
        .expect_err("config error");
        assert!(matches!(err, CoordinatorError::Serve(message) if message == "config failed"));
    }

    #[tokio::test]
    async fn run_with_returns_serve_errors() {
        let err = run_with(
            || Ok::<_, CoordinatorError>("config"),
            |_| async {
                Err::<(), CoordinatorError>(CoordinatorError::Serve("serve failed".to_string()))
            },
        )
        .await
        .expect_err("serve error");
        assert!(matches!(err, CoordinatorError::Serve(message) if message == "serve failed"));
    }

    #[tokio::test]
    async fn run_with_succeeds_when_serve_succeeds() {
        let result = run_with(|| Ok::<_, CoordinatorError>(()), serve_ok).await;
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
        let exit_code = handle_main_result(
            Err(CoordinatorError::Serve("boom".to_string())),
            &mut stderr,
        );
        assert_eq!(exit_code, 1);
        assert_eq!(
            String::from_utf8(stderr).expect("utf8"),
            "coordinator service failed: coordinator serve error: boom\n"
        );
    }

    #[tokio::test]
    async fn main_impl_with_reports_errors() {
        let mut stderr = Vec::new();
        let exit_code = main_impl_with(
            || {},
            || async { Err::<(), CoordinatorError>(CoordinatorError::Serve("boom".to_string())) },
            &mut stderr,
        )
        .await;
        assert_eq!(exit_code, 1);
        let message = String::from_utf8(stderr).expect("utf8");
        assert!(message.contains("coordinator serve error: boom"));
    }

    #[test]
    fn exit_if_needed_skips_exit_when_code_is_zero() {
        exit_if_needed(0, |_| ());
    }

    #[test]
    fn exit_if_needed_calls_exit_when_code_is_non_zero() {
        let mut seen = None;
        exit_if_needed(17, |code| seen = Some(code));
        assert_eq!(seen, Some(17));
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
            || async { Ok::<(), CoordinatorError>(()) },
            &mut stderr,
        )
        .await;
        assert!(called.load(std::sync::atomic::Ordering::Relaxed));
    }
}
