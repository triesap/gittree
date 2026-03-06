#![forbid(unsafe_code)]

use gittree_app::{AppError, AppServiceConfig, serve};
use std::future::Future;
use std::io::Write;
use std::pin::Pin;
use std::process::ExitCode;

type MainRunFuture = Pin<Box<dyn Future<Output = Result<(), AppError>>>>;
type LoadConfigFn = fn() -> Result<AppServiceConfig, AppError>;
type ServeFn = fn(AppServiceConfig) -> MainRunFuture;

#[tokio::main]
async fn main() -> ExitCode {
    let mut stderr = std::io::stderr();
    let exit_code = main_impl(&mut stderr).await;
    let mut status = ExitCode::SUCCESS;
    let mut set_status = |code| {
        status = exit_status(code);
    };
    exit_if_needed(exit_code, &mut set_status);
    status
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

async fn run() -> Result<(), AppError> {
    run_with(load_config, serve_boxed).await
}

fn load_config() -> Result<AppServiceConfig, AppError> {
    AppServiceConfig::from_env().map_err(AppError::Config)
}

fn serve_boxed(config: AppServiceConfig) -> MainRunFuture {
    Box::pin(serve(config))
}

async fn run_with(load_config: LoadConfigFn, serve_fn: ServeFn) -> Result<(), AppError> {
    let config = load_config()?;
    serve_fn(config).await
}

fn handle_main_result(result: Result<(), AppError>, stderr: &mut dyn Write) -> i32 {
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

fn exit_status(exit_code: i32) -> ExitCode {
    if exit_code == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(exit_code.clamp(1, u8::MAX as i32) as u8)
    }
}

#[cfg(test)]
mod tests {
    use super::{exit_if_needed, exit_status, handle_main_result, main_impl_with, run_with};
    use gittree_app::{AppError, AppServiceConfig};
    use gittree_config::UiConfig;
    use gittree_storage::StorageConfig;
    use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
    use std::path::PathBuf;

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

    fn noop_exit(_: i32) {}

    fn runtime_config() -> AppServiceConfig {
        AppServiceConfig {
            bind: SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 18090)),
            base_path: "/ui".to_string(),
            site_root: PathBuf::from("crates/app-ui/dist"),
            site_pkg_dir: "pkg".to_string(),
            command_only: false,
            storage: invalid_storage_config(),
            ui: UiConfig {
                repo_root: PathBuf::from("."),
                public_git_url: "https://gittr.ee".to_string(),
                auth_url: "https://auth.gittr.ee".to_string(),
                app_url: "https://app.gittr.ee".to_string(),
                control_url: "https://api.gittr.ee".to_string(),
            },
        }
    }

    fn load_runtime_config() -> Result<AppServiceConfig, AppError> {
        Ok(runtime_config())
    }

    fn load_config_error() -> Result<AppServiceConfig, AppError> {
        Err(AppError::Serve("config failed".to_string()))
    }

    fn serve_ok(_: AppServiceConfig) -> super::MainRunFuture {
        Box::pin(async { Ok::<(), AppError>(()) })
    }

    fn serve_err(_: AppServiceConfig) -> super::MainRunFuture {
        Box::pin(async { Err::<(), AppError>(AppError::Serve("serve failed".to_string())) })
    }

    #[tokio::test]
    async fn run_with_returns_config_errors() {
        let err = run_with(load_config_error, serve_ok)
        .await
        .expect_err("config error");
        assert_eq!(err.to_string(), "app serve error: config failed");
    }

    #[tokio::test]
    async fn run_with_returns_serve_errors() {
        let err = run_with(load_runtime_config, serve_err)
        .await
        .expect_err("serve error");
        assert_eq!(err.to_string(), "app serve error: serve failed");
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
        let mut load_dotenv = || {};
        let mut run_fn = || -> super::MainRunFuture {
            Box::pin(async { Err::<(), AppError>(AppError::Serve("boom".to_string())) })
        };

        let exit_code = main_impl_with(&mut load_dotenv, &mut run_fn, &mut stderr).await;

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

    #[test]
    fn exit_status_maps_codes() {
        assert_eq!(exit_status(0), std::process::ExitCode::SUCCESS);
        assert_eq!(exit_status(1), std::process::ExitCode::from(1));
        assert_eq!(exit_status(999), std::process::ExitCode::from(u8::MAX));
        assert_eq!(exit_status(-1), std::process::ExitCode::from(1));
    }

    #[tokio::test]
    async fn main_impl_with_executes_loader() {
        let called = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let loader_called = called.clone();
        let mut stderr = Vec::new();
        let mut load_dotenv = move || {
            loader_called.store(true, std::sync::atomic::Ordering::Relaxed);
        };
        let mut run_fn = || -> super::MainRunFuture { Box::pin(async { Ok::<(), AppError>(()) }) };

        let _ = main_impl_with(&mut load_dotenv, &mut run_fn, &mut stderr).await;

        assert!(called.load(std::sync::atomic::Ordering::Relaxed));
    }
}
