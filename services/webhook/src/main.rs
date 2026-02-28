use gittree_webhook::{WebhookConfig, WebhookError, serve};
use std::future::Future;
use std::io::Write;
use std::pin::Pin;
use std::process::ExitCode;

type MainRunFuture = Pin<Box<dyn Future<Output = Result<(), WebhookError>>>>;

fn main() -> ExitCode {
    let mut stderr = std::io::stderr();
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    let exit_code = runtime.block_on(main_impl(&mut stderr));
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
    use gittree_storage::StorageConfig;
    use gittree_webhook::{WebhookConfig, WebhookError};
    use std::ffi::OsString;
    use std::sync::{Mutex, OnceLock};

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    fn with_env_vars(entries: &[(&str, Option<&str>)], run: impl FnOnce()) {
        let _guard = env_lock().lock().expect("env lock");
        let previous: Vec<(&str, Option<OsString>)> = entries
            .iter()
            .map(|(key, _)| (*key, std::env::var_os(key)))
            .collect();

        for (key, value) in entries {
            match value {
                // SAFETY: tests serialize process env mutation behind a global lock.
                Some(value) => unsafe { std::env::set_var(key, value) },
                // SAFETY: tests serialize process env mutation behind a global lock.
                None => unsafe { std::env::remove_var(key) },
            }
        }

        run();

        for (key, value) in previous {
            match value {
                // SAFETY: tests serialize process env mutation behind a global lock.
                Some(value) => unsafe { std::env::set_var(key, value) },
                // SAFETY: tests serialize process env mutation behind a global lock.
                None => unsafe { std::env::remove_var(key) },
            }
        }
    }

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
        let config = WebhookConfig {
            bind: "127.0.0.1:18093".to_string(),
            storage: invalid_storage_config(),
            sync_url: "http://127.0.0.1:8087".to_string(),
            forgejo_secret: "test-secret".to_string(),
        };
        let _err = run_with(|| Ok::<_, WebhookError>(config), super::serve)
            .await
            .expect_err("storage error");
    }

    #[test]
    fn run_reports_config_error_for_invalid_storage_env() {
        with_env_vars(
            &[
                ("GITTREE_WEBHOOK_BIND", Some("not-a-socket")),
                ("GITTREE_STORAGE_READ_URL", Some("not-a-postgres-url")),
            ],
            || {
            let runtime = tokio::runtime::Runtime::new().expect("runtime");
            let err = runtime.block_on(super::run()).expect_err("config error");
            assert!(err.to_string().contains("webhook config error:"));
            },
        );
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

    #[test]
    fn exit_status_maps_codes() {
        assert_eq!(exit_status(0), std::process::ExitCode::SUCCESS);
        assert_eq!(exit_status(1), std::process::ExitCode::from(1));
        assert_eq!(exit_status(999), std::process::ExitCode::from(u8::MAX));
        assert_eq!(exit_status(-1), std::process::ExitCode::from(1));
    }

    #[test]
    fn main_entry_returns_failure_for_invalid_bind() {
        with_env_vars(
            &[
                ("GITTREE_WEBHOOK_BIND", Some("not-a-socket")),
                (
                    "GITTREE_STORAGE_READ_URL",
                    Some("postgres://gittree:gittree@127.0.0.1:5432/gittree"),
                ),
                ("GITTREE_SYNC_URL", Some("http://127.0.0.1:8087")),
                ("GITTREE_FORGEJO_WEBHOOK_SECRET", Some("test-secret")),
            ],
            || {
                let code = super::main();
                assert_ne!(code, std::process::ExitCode::SUCCESS);
            },
        );
    }

    #[test]
    fn with_env_vars_restores_previous_state() {
        const EXISTING: &str = "GITTREE_TEST_WEBHOOK_MAIN_EXISTING";
        const MISSING: &str = "GITTREE_TEST_WEBHOOK_MAIN_MISSING";
        // SAFETY: test-only env mutation for unique keys.
        unsafe { std::env::set_var(EXISTING, "before") };
        // SAFETY: test-only env mutation for unique keys.
        unsafe { std::env::remove_var(MISSING) };

        with_env_vars(&[(EXISTING, None), (MISSING, Some("set"))], || {
            assert!(std::env::var(EXISTING).is_err());
            assert_eq!(std::env::var(MISSING).expect("set"), "set");
        });

        assert_eq!(std::env::var(EXISTING).expect("restored"), "before");
        assert!(std::env::var(MISSING).is_err());
        // SAFETY: test-only cleanup for unique keys.
        unsafe { std::env::remove_var(EXISTING) };
    }
}
