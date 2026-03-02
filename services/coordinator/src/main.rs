use gittree_coordinator::{CoordinatorConfig, CoordinatorError, serve};
use std::future::Future;
use std::io::Write;
use std::pin::Pin;

type MainRunFuture = Pin<Box<dyn Future<Output = Result<(), CoordinatorError>>>>;
type LoadConfigFn = fn() -> Result<CoordinatorConfig, CoordinatorError>;
type ServeFn = fn(CoordinatorConfig) -> MainRunFuture;

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

async fn run() -> Result<(), CoordinatorError> {
    run_with(load_config, serve_boxed).await
}

fn load_config() -> Result<CoordinatorConfig, CoordinatorError> {
    CoordinatorConfig::from_env().map_err(CoordinatorError::Config)
}

fn serve_boxed(config: CoordinatorConfig) -> MainRunFuture {
    Box::pin(serve(config))
}

async fn run_with(load_config: LoadConfigFn, serve_fn: ServeFn) -> Result<(), CoordinatorError> {
    let config = load_config()?;
    serve_fn(config).await
}

fn handle_main_result(result: Result<(), CoordinatorError>, stderr: &mut dyn Write) -> i32 {
    match result {
        Ok(()) => 0,
        Err(err) => {
            let _ = writeln!(stderr, "coordinator service failed: {err}");
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
    use gittree_config::ForgejoConfig;
    use gittree_coordinator::{CoordinatorConfig, CoordinatorError, HookInstallConfig};
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

    fn runtime_config() -> CoordinatorConfig {
        CoordinatorConfig {
            bind: "127.0.0.1:1".to_string(),
            storage: invalid_storage_config(),
            relay_urls: vec!["wss://relay.example".to_string()],
            repo_root: std::env::temp_dir().join("gittree-coordinator-main-runtime"),
            hooks: HookInstallConfig {
                pre_receive_source: std::env::temp_dir().join("pre-receive"),
                post_receive_source: std::env::temp_dir().join("post-receive"),
            },
            forgejo: ForgejoConfig {
                base_url: "https://gittr.ee".to_string(),
                api_token: "token".to_string(),
                owner: "owner".to_string(),
                webhook_url: "https://gittr.ee/hook".to_string(),
                webhook_secret: "secret".to_string(),
                repo_private: true,
            },
        }
    }

    fn load_runtime_config() -> Result<CoordinatorConfig, CoordinatorError> {
        Ok(runtime_config())
    }

    fn load_config_error() -> Result<CoordinatorConfig, CoordinatorError> {
        Err(CoordinatorError::Serve("config failed".to_string()))
    }

    fn serve_ok(_: CoordinatorConfig) -> super::MainRunFuture {
        Box::pin(async { Ok::<(), CoordinatorError>(()) })
    }

    fn serve_err(_: CoordinatorConfig) -> super::MainRunFuture {
        Box::pin(async {
            Err::<(), CoordinatorError>(CoordinatorError::Serve("serve failed".to_string()))
        })
    }

    #[tokio::test]
    async fn run_with_returns_config_errors() {
        let err = run_with(load_config_error, serve_ok)
            .await
            .expect_err("config error");
        assert!(matches!(err, CoordinatorError::Serve(message) if message == "config failed"));
    }

    #[tokio::test]
    async fn run_with_returns_serve_errors() {
        let err = run_with(load_runtime_config, serve_err)
            .await
            .expect_err("serve error");
        assert!(matches!(err, CoordinatorError::Serve(message) if message == "serve failed"));
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
            .expect_err("serve error");
    }

    #[tokio::test]
    async fn run_with_covers_library_probe_paths_for_runtime_instantiation() {
        let err = gittree_coordinator::__coverage_probe_map_serve_result(Err(
            std::io::Error::other("main test probe io error"),
        ))
        .expect_err("probe should map io error");
        assert!(matches!(err, CoordinatorError::Serve(_)));

        let polled = tokio::time::timeout(
            std::time::Duration::from_millis(1),
            gittree_coordinator::__coverage_probe_shutdown_signal(),
        )
        .await;
        assert!(polled.is_err());
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
        let mut load_dotenv = || {};
        let mut run_fn = || -> super::MainRunFuture {
            Box::pin(async {
                Err::<(), CoordinatorError>(CoordinatorError::Serve("boom".to_string()))
            })
        };
        let exit_code = main_impl_with(&mut load_dotenv, &mut run_fn, &mut stderr).await;
        assert_eq!(exit_code, 1);
        let message = String::from_utf8(stderr).expect("utf8");
        assert!(message.contains("coordinator serve error: boom"));
    }

    #[test]
    fn exit_if_needed_skips_exit_when_code_is_zero() {
        let mut exit_fn = |_| ();
        exit_if_needed(0, &mut exit_fn);
    }

    #[test]
    fn exit_if_needed_calls_exit_when_code_is_non_zero() {
        let mut seen = None;
        let mut exit_fn = |code| seen = Some(code);
        exit_if_needed(17, &mut exit_fn);
        assert_eq!(seen, Some(17));
    }

    #[tokio::test]
    async fn main_impl_with_executes_loader() {
        let called = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let loader_called = called.clone();
        let mut stderr = Vec::new();
        let mut load_dotenv = move || {
            loader_called.store(true, std::sync::atomic::Ordering::Relaxed);
        };
        let mut run_fn =
            || -> super::MainRunFuture { Box::pin(async { Ok::<(), CoordinatorError>(()) }) };
        let _ = main_impl_with(&mut load_dotenv, &mut run_fn, &mut stderr).await;
        assert!(called.load(std::sync::atomic::Ordering::Relaxed));
    }
}
