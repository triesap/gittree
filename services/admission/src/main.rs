use gittree_admission::{AdmissionConfig, AdmissionError, serve};
use std::future::Future;
use std::io::Write;
use std::pin::Pin;
use std::process::ExitCode;

type MainRunFuture = Pin<Box<dyn Future<Output = Result<(), AdmissionError>>>>;

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

fn handle_main_result(result: Result<(), AdmissionError>, stderr: &mut dyn Write) -> i32 {
    match result {
        Ok(()) => 0,
        Err(err) => {
            let _ = writeln!(stderr, "admission service failed: {err}");
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
    use gittree_admission::AdmissionError;
    use std::sync::Mutex;

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: Mutex<()> = Mutex::new(());
        &LOCK
    }

    fn with_env_var(key: &str, value: &str, run: &mut dyn FnMut()) {
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

    async fn serve_ok<T>(_config: T) -> Result<(), AdmissionError> {
        Ok(())
    }

    #[tokio::test]
    async fn run_with_returns_config_errors() {
        let err = run_with(
            || Err::<(), AdmissionError>(AdmissionError::Serve("config failed".to_string())),
            serve_ok,
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
        let result = run_with(|| Ok::<_, AdmissionError>("config"), serve_ok).await;
        assert!(result.is_ok());
    }

    #[test]
    fn with_env_var_restores_existing_value() {
        const KEY: &str = "GITTREE_TEST_ADMISSION_MAIN_ENV";
        // SAFETY: test-only env mutation for a unique key.
        unsafe { std::env::set_var(KEY, "before") };
        with_env_var(KEY, "after", &mut || {
            assert_eq!(std::env::var(KEY).expect("set"), "after");
        });
        assert_eq!(std::env::var(KEY).expect("restored"), "before");
        // SAFETY: test-only env cleanup for a unique key.
        unsafe { std::env::remove_var(KEY) };
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
        let mut load_dotenv = || {};
        let mut run_fn = || -> super::MainRunFuture {
            Box::pin(async { Err::<(), AdmissionError>(AdmissionError::Serve("boom".to_string())) })
        };
        let exit_code = main_impl_with(&mut load_dotenv, &mut run_fn, &mut stderr).await;
        assert_eq!(exit_code, 1);
        let message = String::from_utf8(stderr).expect("utf8");
        assert!(message.contains("admission serve error: boom"));
    }

    #[tokio::test]
    async fn main_impl_with_executes_loader() {
        let called = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let loader_called = called.clone();
        let mut stderr = Vec::new();
        let mut load_dotenv = move || {
            loader_called.store(true, std::sync::atomic::Ordering::Relaxed);
        };
        let mut run_fn = || -> super::MainRunFuture { Box::pin(async { Ok::<(), AdmissionError>(()) }) };
        let _ = main_impl_with(&mut load_dotenv, &mut run_fn, &mut stderr).await;
        assert!(called.load(std::sync::atomic::Ordering::Relaxed));
    }

    #[test]
    fn run_reports_config_error_for_invalid_bind_env() {
        with_env_var("GITTREE_ADMISSION_BIND", "not-a-socket", &mut || {
            let runtime = tokio::runtime::Runtime::new().expect("runtime");
            let err = runtime.block_on(super::run()).expect_err("config error");
            assert!(err.to_string().contains("admission config error"));
        });
    }

    #[test]
    fn main_impl_reports_config_error_for_invalid_bind_env() {
        with_env_var("GITTREE_ADMISSION_BIND", "not-a-socket", &mut || {
            let runtime = tokio::runtime::Runtime::new().expect("runtime");
            let mut stderr = Vec::new();
            let exit_code = runtime.block_on(super::main_impl(&mut stderr));
            assert_eq!(exit_code, 1);
            let message = String::from_utf8(stderr).expect("utf8");
            assert!(message.contains("admission config error"));
        });
    }

    #[test]
    fn exit_status_maps_codes() {
        assert_eq!(exit_status(0), std::process::ExitCode::SUCCESS);
        assert_eq!(exit_status(1), std::process::ExitCode::from(1));
        assert_eq!(exit_status(999), std::process::ExitCode::from(u8::MAX));
        assert_eq!(exit_status(-1), std::process::ExitCode::from(1));
    }
}
