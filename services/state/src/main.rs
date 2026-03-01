use gittree_state::{StateConfig, StateError, serve};
use std::future::Future;
use std::io::Write;
use std::pin::Pin;
use std::process::ExitCode;

type MainRunFuture = Pin<Box<dyn Future<Output = Result<(), StateError>>>>;
type LoadConfigFn = fn() -> Result<StateConfig, StateError>;
type ServeFn = fn(StateConfig) -> MainRunFuture;

fn main() -> ExitCode {
    let mut stderr = std::io::stderr();
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    let exit_code = runtime.block_on(main_impl(&mut stderr));
    exit_status(exit_code)
}

async fn main_impl(stderr: &mut dyn Write) -> i32 {
    let mut load_dotenv = || {
        dotenvy::dotenv().ok();
    };
    let mut run_fn = || -> MainRunFuture { Box::pin(run()) };
    main_impl_with(&mut load_dotenv, &mut run_fn, stderr).await
}

async fn main_impl_with(
    load_dotenv: &mut dyn FnMut(),
    run_fn: &mut dyn FnMut() -> MainRunFuture,
    stderr: &mut dyn Write,
) -> i32 {
    load_dotenv();
    handle_main_result(run_fn().await, stderr)
}

async fn run() -> Result<(), StateError> {
    run_with(load_config, serve_boxed).await
}

fn load_config() -> Result<StateConfig, StateError> {
    StateConfig::from_env().map_err(StateError::Config)
}

fn serve_boxed(config: StateConfig) -> MainRunFuture {
    Box::pin(serve(config))
}

async fn run_with(load_config: LoadConfigFn, serve_fn: ServeFn) -> Result<(), StateError> {
    let config = load_config()?;
    serve_fn(config).await
}

fn handle_main_result(result: Result<(), StateError>, stderr: &mut dyn Write) -> i32 {
    match result {
        Ok(()) => 0,
        Err(err) => {
            let _ = writeln!(stderr, "state service failed: {err}");
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
    use super::{exit_status, handle_main_result, main_impl_with, run_with};
    use gittree_state::{StateConfig, StateError};
    use gittree_storage::StorageConfig;

    fn invalid_storage_config() -> StorageConfig {
        StorageConfig {
            read_connection: "not-a-postgres-url".to_string(),
            write_connection: None,
            max_connections: 10,
            min_connections: 2,
            idle_timeout_secs: None,
            max_lifetime_secs: None,
            application_name: None,
        }
    }

    fn runtime_config() -> StateConfig {
        StateConfig {
            bind: "127.0.0.1:18092".to_string(),
            storage: invalid_storage_config(),
            relay_urls: vec!["wss://relay.example".to_string()],
        }
    }

    fn load_runtime_config() -> Result<StateConfig, StateError> {
        Ok(runtime_config())
    }

    fn load_config_error() -> Result<StateConfig, StateError> {
        Err(StateError::Serve("config failed".to_string()))
    }

    fn serve_ok(_: StateConfig) -> super::MainRunFuture {
        Box::pin(async { Ok::<(), StateError>(()) })
    }

    fn serve_err(_: StateConfig) -> super::MainRunFuture {
        Box::pin(async { Err::<(), StateError>(StateError::Serve("serve failed".to_string())) })
    }

    #[tokio::test]
    async fn run_with_returns_config_errors() {
        let err = run_with(load_config_error, serve_ok)
            .await
            .expect_err("config error");
        assert!(matches!(err, StateError::Serve(message) if message == "config failed"));
    }

    #[tokio::test]
    async fn run_with_returns_serve_errors() {
        let err = run_with(load_runtime_config, serve_err)
            .await
            .expect_err("serve error");
        assert!(matches!(err, StateError::Serve(message) if message == "serve failed"));
    }

    #[tokio::test]
    async fn run_with_succeeds_when_serve_succeeds() {
        let result = run_with(load_runtime_config, serve_ok).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn run_with_covers_production_serve_monomorphization() {
        let _err = run_with(load_runtime_config, super::serve_boxed)
            .await
            .expect_err("storage error");
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
        let exit_code = handle_main_result(Err(StateError::Serve("boom".to_string())), &mut stderr);
        assert_eq!(exit_code, 1);
        assert_eq!(
            String::from_utf8(stderr).expect("utf8"),
            "state service failed: state serve error: boom\n"
        );
    }

    #[tokio::test]
    async fn main_impl_with_reports_errors() {
        let mut stderr = Vec::new();
        let mut load_dotenv = || {};
        let mut run_fn = || -> super::MainRunFuture {
            Box::pin(async { Err::<(), StateError>(StateError::Serve("boom".to_string())) })
        };
        let exit_code = main_impl_with(&mut load_dotenv, &mut run_fn, &mut stderr).await;

        assert_eq!(exit_code, 1);
        let message = String::from_utf8(stderr).expect("utf8");
        assert!(message.contains("state serve error: boom"));
    }

    #[tokio::test]
    async fn main_impl_with_executes_loader() {
        let called = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let loader_called = called.clone();
        let mut stderr = Vec::new();
        let mut load_dotenv = move || {
            loader_called.store(true, std::sync::atomic::Ordering::Relaxed);
        };
        let mut run_fn = || -> super::MainRunFuture { Box::pin(async { Ok::<(), StateError>(()) }) };

        let _ = main_impl_with(&mut load_dotenv, &mut run_fn, &mut stderr).await;

        assert!(called.load(std::sync::atomic::Ordering::Relaxed));
    }

    #[test]
    fn exit_status_maps_codes() {
        assert_eq!(exit_status(0), std::process::ExitCode::SUCCESS);
        assert_eq!(exit_status(1), std::process::ExitCode::from(1));
        assert_eq!(exit_status(999), std::process::ExitCode::from(u8::MAX));
        assert_eq!(exit_status(-1), std::process::ExitCode::from(1));
    }
}
