use gittree_webhook::{WebhookConfig, WebhookError, serve};
use std::future::Future;
use std::io::Write;
use std::pin::Pin;

type MainRunFuture = Pin<Box<dyn Future<Output = Result<(), WebhookError>>>>;

fn main() {
    let mut stderr = std::io::stderr();
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    let exit_code = runtime.block_on(main_impl(&mut stderr));
    let mut exit_fn = |code| std::process::exit(code);
    exit_if_needed(exit_code, &mut exit_fn);
}

async fn main_impl(stderr: &mut dyn Write) -> i32 {
    let mut load_dotenv = || {
        dotenvy::dotenv().ok();
    };
    let mut run_fn = || -> MainRunFuture { Box::pin(run()) };
    main_impl_with(
        &mut load_dotenv,
        &mut run_fn,
        stderr,
    )
    .await
}

async fn main_impl_with(
    load_dotenv: &mut dyn FnMut(),
    run_fn: &mut dyn FnMut() -> MainRunFuture,
    stderr: &mut dyn Write,
) -> i32 {
    load_dotenv();
    handle_main_result(run_fn().await, stderr)
}

async fn run() -> Result<(), WebhookError> {
    run_with(
        || WebhookConfig::from_env().map_err(WebhookError::Config),
        serve,
    )
    .await
}

async fn run_with<Config, LoadFn, ServeFn, ServeFut>(
    load_config: LoadFn,
    serve_fn: ServeFn,
) -> Result<(), WebhookError>
where
    LoadFn: FnOnce() -> Result<Config, WebhookError>,
    ServeFn: FnOnce(Config) -> ServeFut,
    ServeFut: Future<Output = Result<(), WebhookError>>,
{
    let config = load_config()?;
    serve_fn(config).await
}

fn handle_main_result(result: Result<(), WebhookError>, stderr: &mut dyn Write) -> i32 {
    match result {
        Ok(()) => 0,
        Err(err) => {
            let _ = writeln!(stderr, "webhook service failed: {err}");
            1
        }
    }
}

fn exit_if_needed(exit_code: i32, exit_fn: &mut dyn FnMut(i32)) {
    if exit_code != 0 {
        exit_fn(exit_code);
    }
}

#[cfg(test)]
mod tests {
    use super::{exit_if_needed, handle_main_result, main_impl_with, run_with};
    use gittree_webhook::WebhookError;

    async fn serve_ok(_: ()) -> Result<(), WebhookError> {
        Ok(())
    }

    #[tokio::test]
    async fn run_with_returns_config_errors() {
        let err = run_with(
            || Err::<(), WebhookError>(WebhookError::Serve("config failed".to_string())),
            serve_ok,
        )
        .await
        .expect_err("config error");
        assert_eq!(err.to_string(), "webhook serve error: config failed");
    }

    #[tokio::test]
    async fn run_with_returns_serve_errors() {
        let err = run_with(
            || Ok::<_, WebhookError>("config"),
            |_| async { Err::<(), WebhookError>(WebhookError::Serve("serve failed".to_string())) },
        )
        .await
        .expect_err("serve error");
        assert_eq!(err.to_string(), "webhook serve error: serve failed");
    }

    #[tokio::test]
    async fn run_with_succeeds_when_serve_succeeds() {
        let result = run_with(|| Ok::<_, WebhookError>(()), serve_ok).await;
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
        let exit_code =
            handle_main_result(Err(WebhookError::Serve("boom".to_string())), &mut stderr);
        assert_eq!(exit_code, 1);
        assert_eq!(
            String::from_utf8(stderr).expect("utf8"),
            "webhook service failed: webhook serve error: boom\n"
        );
    }

    #[tokio::test]
    async fn main_impl_with_reports_errors() {
        let mut stderr = Vec::new();
        let mut load_dotenv = || {};
        let mut run_fn = || -> super::MainRunFuture {
            Box::pin(async { Err::<(), WebhookError>(WebhookError::Serve("boom".to_string())) })
        };
        let exit_code = main_impl_with(
            &mut load_dotenv,
            &mut run_fn,
            &mut stderr,
        )
        .await;
        assert_eq!(exit_code, 1);
        let message = String::from_utf8(stderr).expect("utf8");
        assert!(message.contains("webhook serve error: boom"));
    }

    #[tokio::test]
    async fn main_impl_with_executes_loader() {
        let called = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let loader_called = called.clone();
        let mut stderr = Vec::new();
        let mut load_dotenv = move || {
            loader_called.store(true, std::sync::atomic::Ordering::Relaxed);
        };
        let mut run_fn = || -> super::MainRunFuture { Box::pin(async { Ok::<(), WebhookError>(()) }) };
        let _ = main_impl_with(&mut load_dotenv, &mut run_fn, &mut stderr).await;
        assert!(called.load(std::sync::atomic::Ordering::Relaxed));
    }

    #[test]
    fn exit_if_needed_calls_exit_for_non_zero_code() {
        let mut captured = None;
        let mut exit_fn = |code| captured = Some(code);
        exit_if_needed(3, &mut exit_fn);
        assert_eq!(captured, Some(3));
    }

    #[test]
    fn exit_if_needed_skips_exit_for_zero_code() {
        let mut exit_fn = |_code| ();
        exit_if_needed(0, &mut exit_fn);
    }
}
