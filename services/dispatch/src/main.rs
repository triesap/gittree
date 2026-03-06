use gittree_dispatch::{DispatchConfig, DispatchError, serve};
use std::future::Future;
use std::io::Write;
use std::pin::Pin;
use std::process::ExitCode;

type MainRunFuture = Pin<Box<dyn Future<Output = Result<(), DispatchError>>>>;
type LoadConfigFn = fn() -> Result<DispatchConfig, DispatchError>;
type ServeFn = fn(DispatchConfig) -> MainRunFuture;

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

async fn run() -> Result<(), DispatchError> {
    run_with(load_config, serve_boxed).await
}

fn load_config() -> Result<DispatchConfig, DispatchError> {
    DispatchConfig::from_env()
}

fn serve_boxed(config: DispatchConfig) -> MainRunFuture {
    Box::pin(serve(config))
}

async fn run_with(load_config: LoadConfigFn, serve_fn: ServeFn) -> Result<(), DispatchError> {
    let config = load_config()?;
    serve_fn(config).await
}

fn handle_main_result(result: Result<(), DispatchError>, stderr: &mut dyn Write) -> i32 {
    match result {
        Ok(()) => 0,
        Err(err) => {
            let _ = writeln!(stderr, "dispatch service failed: {err}");
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
    use gittree_dispatch::{DispatchConfig, DispatchError};
    use gittree_storage::StorageConfig;

    fn config() -> DispatchConfig {
        DispatchConfig {
            bind: "127.0.0.1:19091".to_string(),
            admin_pubkey: "npub1admin".to_string(),
            relay_urls: vec!["wss://gittr.ee".to_string()],
            storage: StorageConfig {
                read_connection: "postgres://gittree:gittree@127.0.0.1:5432/gittree".to_string(),
                write_connection: None,
                max_connections: 10,
                min_connections: 2,
                idle_timeout_secs: None,
                max_lifetime_secs: None,
                application_name: None,
            },
        }
    }

    fn load_ok() -> Result<DispatchConfig, DispatchError> {
        Ok(config())
    }

    fn load_err() -> Result<DispatchConfig, DispatchError> {
        Err(DispatchError::Config("missing config".to_string()))
    }

    fn serve_ok(_: DispatchConfig) -> super::MainRunFuture {
        Box::pin(async { Ok::<(), DispatchError>(()) })
    }

    fn serve_err(_: DispatchConfig) -> super::MainRunFuture {
        Box::pin(async {
            Err::<(), DispatchError>(DispatchError::Config("serve failed".to_string()))
        })
    }

    #[tokio::test]
    async fn run_with_returns_config_error() {
        let err = run_with(load_err, serve_ok)
            .await
            .expect_err("config error");
        assert!(matches!(err, DispatchError::Config(message) if message == "missing config"));
    }

    #[tokio::test]
    async fn run_with_returns_serve_error() {
        let err = run_with(load_ok, serve_err).await.expect_err("serve error");
        assert!(matches!(err, DispatchError::Config(message) if message == "serve failed"));
    }

    #[tokio::test]
    async fn main_impl_with_reports_error() {
        let mut stderr = Vec::new();
        let mut load_dotenv = || {};
        let mut run_fn = || -> super::MainRunFuture {
            Box::pin(async {
                Err::<(), DispatchError>(DispatchError::Config("serve failed".to_string()))
            })
        };

        let exit_code = main_impl_with(&mut load_dotenv, &mut run_fn, &mut stderr).await;
        assert_eq!(exit_code, 1);
        let output = String::from_utf8(stderr).expect("utf8");
        assert!(output.contains("dispatch service failed"));
    }

    #[test]
    fn handle_main_result_success() {
        let mut stderr = Vec::new();
        let code = handle_main_result(Ok(()), &mut stderr);
        assert_eq!(code, 0);
        assert!(stderr.is_empty());
    }

    #[test]
    fn exit_status_maps_values() {
        assert_eq!(exit_status(0), std::process::ExitCode::SUCCESS);
        assert_eq!(exit_status(1), std::process::ExitCode::from(1));
        assert_eq!(exit_status(255), std::process::ExitCode::from(255));
        assert_eq!(exit_status(-1), std::process::ExitCode::from(1));
    }
}
