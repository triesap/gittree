use gittree_sync::{SyncConfig, SyncError, serve};
use std::future::Future;
use std::io::Write;
use std::process::ExitCode;

#[cfg(not(test))]
fn main() -> ExitCode {
    let mut stderr = std::io::stderr();
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    let exit_code = runtime.block_on(main_impl(&mut stderr));
    let mut status = ExitCode::SUCCESS;
    maybe_exit(exit_code, |code| {
        status = exit_status(code);
    });
    status
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
    RunFut: Future<Output = Result<(), SyncError>>,
{
    load_dotenv();
    handle_main_result(run_fn().await, stderr)
}

async fn run() -> Result<(), SyncError> {
    run_with(|| SyncConfig::from_env().map_err(SyncError::Config), serve).await
}

async fn run_with<Config, LoadFn, ServeFn, ServeFut>(
    load_config: LoadFn,
    serve_fn: ServeFn,
) -> Result<(), SyncError>
where
    LoadFn: FnOnce() -> Result<Config, SyncError>,
    ServeFn: FnOnce(Config) -> ServeFut,
    ServeFut: Future<Output = Result<(), SyncError>>,
{
    let config = load_config()?;
    serve_fn(config).await
}

fn handle_main_result(result: Result<(), SyncError>, stderr: &mut impl Write) -> i32 {
    match result {
        Ok(()) => 0,
        Err(err) => {
            let _ = writeln!(stderr, "sync service failed: {err}");
            1
        }
    }
}

fn maybe_exit(exit_code: i32, exit_fn: impl FnOnce(i32)) {
    if exit_code != 0 {
        exit_fn(exit_code);
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
    use super::{exit_status, handle_main_result, main_impl_with, maybe_exit, run_with};
    use gittree_storage::StorageConfig;
    use gittree_sync::{SyncConfig, SyncError};
    use std::sync::Mutex;

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: Mutex<()> = Mutex::new(());
        &LOCK
    }

    fn with_env_var(key: &str, value: &str, run: impl FnOnce()) {
        let _guard = env_lock().lock().expect("env lock");
        let previous = std::env::var_os(key);
        // SAFETY: test-only env mutation guarded by a process-wide lock.
        unsafe {
            std::env::set_var(key, value);
        }
        run();
        match previous {
            Some(previous) => {
                // SAFETY: restore previous value under the same lock.
                unsafe { std::env::set_var(key, previous) };
            }
            None => {
                // SAFETY: restore missing state under the same lock.
                unsafe { std::env::remove_var(key) };
            }
        }
    }

    async fn serve_ok<T>(_config: T) -> Result<(), SyncError> {
        Ok(())
    }

    #[tokio::test]
    async fn run_with_returns_config_errors() {
        let err = run_with(
            || Err::<(), SyncError>(SyncError::Serve("config failed".to_string())),
            serve_ok,
        )
        .await
        .expect_err("config error");
        assert!(matches!(err, SyncError::Serve(message) if message == "config failed"));
    }

    #[tokio::test]
    async fn run_with_returns_serve_errors() {
        let err = run_with(
            || Ok::<_, SyncError>("config"),
            |_| async { Err::<(), SyncError>(SyncError::Serve("serve failed".to_string())) },
        )
        .await
        .expect_err("serve error");
        assert!(matches!(err, SyncError::Serve(message) if message == "serve failed"));
    }

    #[tokio::test]
    async fn run_with_succeeds_when_serve_succeeds() {
        let result = run_with(|| Ok::<_, SyncError>("config"), serve_ok).await;
        assert!(result.is_ok());
    }

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

    #[tokio::test]
    async fn run_with_covers_production_serve_monomorphization() {
        let config = SyncConfig {
            bind: "127.0.0.1:1".to_string(),
            storage: invalid_storage_config(),
            relay_urls: vec!["wss://relay.example".to_string()],
            repo_root: std::env::temp_dir().join("gittree-sync-main-runtime"),
        };
        let _err = run_with(|| Ok::<_, SyncError>(config), super::serve)
            .await
            .expect_err("serve error");
    }

    #[test]
    fn with_env_var_restores_existing_value() {
        const KEY: &str = "GITTREE_TEST_SYNC_MAIN_ENV";
        // SAFETY: test-only env mutation for a unique key.
        unsafe { std::env::set_var(KEY, "before") };
        with_env_var(KEY, "after", || {
            assert_eq!(std::env::var(KEY).expect("set"), "after");
        });
        assert_eq!(std::env::var(KEY).expect("restored"), "before");
        // SAFETY: test-only env cleanup for a unique key.
        unsafe { std::env::remove_var(KEY) };
    }

    #[test]
    fn with_env_var_restores_missing_value() {
        const KEY: &str = "GITTREE_TEST_SYNC_MAIN_ENV_MISSING";
        // SAFETY: test-only env cleanup for a unique key.
        unsafe { std::env::remove_var(KEY) };
        with_env_var(KEY, "set", || {
            assert_eq!(std::env::var(KEY).expect("set"), "set");
        });
        assert!(std::env::var(KEY).is_err());
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
        let exit_code = handle_main_result(Err(SyncError::Serve("boom".to_string())), &mut stderr);
        assert_eq!(exit_code, 1);
        assert_eq!(
            String::from_utf8(stderr).expect("utf8"),
            "sync service failed: sync serve error: boom\n"
        );
    }

    #[test]
    fn maybe_exit_skips_exit_for_zero_code() {
        maybe_exit(0, std::mem::drop);
    }

    #[test]
    fn maybe_exit_invokes_exit_for_non_zero_code() {
        let seen = std::sync::Arc::new(std::sync::Mutex::new(None));
        let seen_code = seen.clone();
        maybe_exit(7, move |code| {
            *seen_code.lock().expect("seen code") = Some(code);
        });
        assert_eq!(*seen.lock().expect("seen code"), Some(7));
    }

    #[test]
    fn maybe_exit_invokes_drop_for_non_zero_code() {
        maybe_exit(7, std::mem::drop);
    }

    #[test]
    fn exit_status_maps_codes() {
        assert_eq!(exit_status(0), std::process::ExitCode::SUCCESS);
        assert_eq!(exit_status(1), std::process::ExitCode::from(1));
        assert_eq!(exit_status(999), std::process::ExitCode::from(u8::MAX));
        assert_eq!(exit_status(-1), std::process::ExitCode::from(1));
    }

    #[tokio::test]
    async fn main_impl_with_reports_errors() {
        let mut stderr = Vec::new();
        let exit_code = main_impl_with(
            || {},
            || async { Err::<(), SyncError>(SyncError::Serve("boom".to_string())) },
            &mut stderr,
        )
        .await;

        assert_eq!(exit_code, 1);
        let message = String::from_utf8(stderr).expect("utf8");
        assert!(message.contains("sync serve error: boom"));
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
            || async { Ok::<(), SyncError>(()) },
            &mut stderr,
        )
        .await;

        assert!(called.load(std::sync::atomic::Ordering::Relaxed));
    }

    #[test]
    fn run_reports_config_error_for_invalid_bind_env() {
        with_env_var("GITTREE_SYNC_BIND", "not-a-socket", || {
            let runtime = tokio::runtime::Runtime::new().expect("runtime");
            let err = runtime.block_on(super::run()).expect_err("config error");
            assert!(err.to_string().contains("sync config error"));
        });
    }

    #[test]
    fn main_impl_reports_config_error_for_invalid_bind_env() {
        with_env_var("GITTREE_SYNC_BIND", "not-a-socket", || {
            let runtime = tokio::runtime::Runtime::new().expect("runtime");
            let mut stderr = Vec::new();
            let exit_code = runtime.block_on(super::main_impl(&mut stderr));
            assert_eq!(exit_code, 1);
            let message = String::from_utf8(stderr).expect("utf8");
            assert!(message.contains("sync config error"));
        });
    }
}
