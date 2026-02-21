use gittree_admission::{AdmissionConfig, AdmissionError, serve};
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
    RunFut: Future<Output = Result<(), AdmissionError>>,
{
    load_dotenv();
    handle_main_result(run_fn().await, stderr)
}

async fn run() -> Result<(), AdmissionError> {
    run_with(
        || AdmissionConfig::from_env().map_err(AdmissionError::Config),
        serve,
    )
    .await
}

async fn run_with<Config, LoadFn, ServeFn, ServeFut>(
    load_config: LoadFn,
    serve_fn: ServeFn,
) -> Result<(), AdmissionError>
where
    LoadFn: FnOnce() -> Result<Config, AdmissionError>,
    ServeFn: FnOnce(Config) -> ServeFut,
    ServeFut: Future<Output = Result<(), AdmissionError>>,
{
    let config = load_config()?;
    serve_fn(config).await
}

fn handle_main_result(result: Result<(), AdmissionError>, stderr: &mut impl Write) -> i32 {
    match result {
        Ok(()) => 0,
        Err(err) => {
            let _ = writeln!(stderr, "admission service failed: {err}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{handle_main_result, main_impl_with, run_with};
    use gittree_admission::AdmissionError;
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
            || Err::<(), AdmissionError>(AdmissionError::Serve("config failed".to_string())),
            |_| async { Ok::<(), AdmissionError>(()) },
        )
        .await
        .expect_err("config error");
        assert!(matches!(err, AdmissionError::Serve(message) if message == "config failed"));
    }

    #[tokio::test]
    async fn run_with_returns_serve_errors() {
        let err = run_with(
            || Ok::<_, AdmissionError>("config"),
            |_| async {
                Err::<(), AdmissionError>(AdmissionError::Serve("serve failed".to_string()))
            },
        )
        .await
        .expect_err("serve error");
        assert!(matches!(err, AdmissionError::Serve(message) if message == "serve failed"));
    }

    #[tokio::test]
    async fn run_with_succeeds_when_serve_succeeds() {
        let result = run_with(
            || Ok::<_, AdmissionError>("config"),
            |_| async { Ok::<(), AdmissionError>(()) },
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
        let exit_code =
            handle_main_result(Err(AdmissionError::Serve("boom".to_string())), &mut stderr);
        assert_eq!(exit_code, 1);
        assert_eq!(
            String::from_utf8(stderr).expect("utf8"),
            "admission service failed: admission serve error: boom\n"
        );
    }

    #[tokio::test]
    async fn main_impl_with_reports_errors() {
        let mut stderr = Vec::new();
        let exit_code = main_impl_with(
            || {},
            || async { Err::<(), AdmissionError>(AdmissionError::Serve("boom".to_string())) },
            &mut stderr,
        )
        .await;
        assert_eq!(exit_code, 1);
        let message = String::from_utf8(stderr).expect("utf8");
        assert!(message.contains("admission serve error: boom"));
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
            || async { Ok::<(), AdmissionError>(()) },
            &mut stderr,
        )
        .await;
        assert!(called.load(std::sync::atomic::Ordering::Relaxed));
    }

    #[test]
    fn run_reports_config_error_for_invalid_bind_env() {
        with_env_var("GITTREE_ADMISSION_BIND", "not-a-socket", || {
            let runtime = tokio::runtime::Runtime::new().expect("runtime");
            let err = runtime.block_on(super::run()).expect_err("config error");
            assert!(matches!(err, AdmissionError::Config(_)));
        });
    }

    #[test]
    fn main_impl_reports_config_error_for_invalid_bind_env() {
        with_env_var("GITTREE_ADMISSION_BIND", "not-a-socket", || {
            let runtime = tokio::runtime::Runtime::new().expect("runtime");
            let mut stderr = Vec::new();
            let exit_code = runtime.block_on(super::main_impl(&mut stderr));
            assert_eq!(exit_code, 1);
            let message = String::from_utf8(stderr).expect("utf8");
            assert!(message.contains("admission config error"));
        });
    }
}
