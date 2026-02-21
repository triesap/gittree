use gittree_sync::{SyncConfig, SyncError, serve};
use std::future::Future;
use std::io::Write;

#[tokio::main]
async fn main() {
    let mut stderr = std::io::stderr();
    let exit_code = main_impl(&mut stderr).await;
    if exit_code != 0 {
        std::process::exit(exit_code);
    }
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

#[cfg(test)]
mod tests {
    use super::{handle_main_result, main_impl_with, run_with};
    use gittree_sync::SyncError;
    use std::sync::{Mutex, OnceLock};

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
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

    #[tokio::test]
    async fn run_with_returns_config_errors() {
        let err = run_with(
            || Err::<(), SyncError>(SyncError::Serve("config failed".to_string())),
            |_| async { Ok::<(), SyncError>(()) },
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
        let result = run_with(
            || Ok::<_, SyncError>("config"),
            |_| async { Ok::<(), SyncError>(()) },
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
        let exit_code = handle_main_result(Err(SyncError::Serve("boom".to_string())), &mut stderr);
        assert_eq!(exit_code, 1);
        assert_eq!(
            String::from_utf8(stderr).expect("utf8"),
            "sync service failed: sync serve error: boom\n"
        );
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
            assert!(matches!(err, SyncError::Config(_)));
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
