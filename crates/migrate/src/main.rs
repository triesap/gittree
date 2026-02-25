use std::io::Write;
#[cfg(not(test))]
use std::process::ExitCode;
use std::pin::Pin;
use std::{fmt, future::Future};

type MainResultFuture = Pin<Box<dyn Future<Output = Result<i64, MainError>>>>;

#[derive(Debug, Clone, PartialEq, Eq)]
enum MainError {
    Observability(String),
    Migration(String),
}

impl fmt::Display for MainError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MainError::Observability(message) => write!(f, "{message}"),
            MainError::Migration(message) => write!(f, "{message}"),
        }
    }
}

#[cfg(not(test))]
fn init_observability() -> Result<gittree_observability::ObservabilityHandle, String> {
    let config = gittree_observability::ObservabilityConfig::from_env("gittree-migrate")
        .map_err(|err| err.to_string())?;
    gittree_observability::init(&config).map_err(|err| err.to_string())
}

async fn run_migrations<InitFn, InitOut, RunFn, RunFut>(
    init_fn: InitFn,
    run_fn: RunFn,
) -> Result<i64, MainError>
where
    InitFn: FnOnce() -> Result<InitOut, String>,
    RunFn: FnOnce() -> RunFut,
    RunFut: Future<Output = Result<i64, gittree_migrate::MigrationError>>,
{
    let _observability = init_fn().map_err(MainError::Observability)?;
    run_fn()
        .await
        .map_err(|err| MainError::Migration(err.to_string()))
}

async fn main_result_with<InitFn, InitOut, RunFn, RunFut>(
    init_fn: InitFn,
    run_fn: RunFn,
) -> Result<i64, MainError>
where
    InitFn: FnOnce() -> Result<InitOut, String>,
    RunFn: FnOnce() -> RunFut,
    RunFut: Future<Output = Result<i64, gittree_migrate::MigrationError>>,
{
    run_migrations(init_fn, run_fn).await
}

#[cfg(not(test))]
async fn main_result() -> Result<i64, MainError> {
    main_result_with(init_observability, gittree_migrate::run).await
}

fn handle_main_outcome(
    result: Result<i64, MainError>,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    match result {
        Ok(version) => {
            let _ = writeln!(stdout, "migrations complete: version {version}");
            0
        }
        Err(err @ MainError::Observability(_)) => {
            let _ = writeln!(stderr, "migration observability failed: {err}");
            1
        }
        Err(err @ MainError::Migration(_)) => {
            let _ = writeln!(stderr, "migration failed: {err}");
            1
        }
    }
}

#[cfg(not(test))]
async fn main_impl(stdout: &mut impl Write, stderr: &mut impl Write) -> i32 {
    let mut load_dotenv = || {
        dotenvy::dotenv().ok();
    };
    let mut result_fn = || -> MainResultFuture { Box::pin(main_result()) };
    main_impl_with(
        &mut load_dotenv,
        &mut result_fn,
        stdout,
        stderr,
    )
    .await
}

async fn main_impl_with(
    load_dotenv: &mut dyn FnMut(),
    result_fn: &mut dyn FnMut() -> MainResultFuture,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    load_dotenv();
    handle_main_outcome(result_fn().await, stdout, stderr)
}

#[cfg(not(test))]
#[tokio::main]
async fn main() -> ExitCode {
    let mut stdout = std::io::stdout();
    let mut stderr = std::io::stderr();
    let exit_code = main_impl(&mut stdout, &mut stderr).await;
    ExitCode::from(exit_code as u8)
}

#[cfg(test)]
mod tests {
    use super::{MainError, handle_main_outcome, main_impl_with, main_result_with, run_migrations};
    use gittree_migrate::{MigrationConfigError, MigrationError};
    use std::sync::Once;

    fn init_test_tracing() {
        static INIT: Once = Once::new();
        INIT.call_once(|| {
            let config = gittree_observability::ObservabilityConfig {
                service_name: "gittree-migrate-tests".to_string(),
                otlp_endpoint: None,
                log_json: false,
                log_dir: None,
                log_stdout: false,
                metrics_enabled: false,
            };
            let _ = gittree_observability::init(&config);
        });
    }

    fn init_ok() -> Result<(), String> {
        Ok(())
    }

    fn init_observer_down() -> Result<(), String> {
        Err("observer down".to_string())
    }

    async fn migration_version_12() -> Result<i64, MigrationError> {
        Ok(12)
    }

    async fn migration_version_9() -> Result<i64, MigrationError> {
        Ok(9)
    }

    async fn noop_migration_runner() -> Result<i64, MigrationError> {
        Ok(0)
    }

    async fn migration_config_error() -> Result<i64, MigrationError> {
        Err(MigrationError::Config(MigrationConfigError::MissingEnv(
            "GITTREE_STORAGE_READ_URL",
        )))
    }

    #[tokio::test]
    async fn noop_migration_runner_returns_zero() {
        let version = noop_migration_runner().await.expect("version");
        assert_eq!(version, 0);
    }

    #[tokio::test]
    async fn run_migrations_returns_version_on_success() {
        init_test_tracing();
        let version = run_migrations(init_ok, migration_version_12)
            .await
            .expect("version");
        assert_eq!(version, 12);
    }

    #[tokio::test]
    async fn run_migrations_maps_observability_errors() {
        let err = run_migrations(init_observer_down, noop_migration_runner)
            .await
            .expect_err("observability error");
        assert_eq!(err.to_string(), "observer down");
    }

    #[tokio::test]
    async fn run_migrations_maps_migration_errors() {
        let err = run_migrations(init_ok, migration_config_error)
            .await
            .expect_err("migration error");
        assert!(err.to_string().contains("migration config error"));
    }

    #[tokio::test]
    async fn main_result_with_delegates_success() {
        let version = main_result_with(init_ok, migration_version_9)
            .await
            .expect("version");
        assert_eq!(version, 9);
    }

    #[tokio::test]
    async fn main_result_with_delegates_errors() {
        let err = main_result_with(init_observer_down, noop_migration_runner)
            .await
            .expect_err("error");
        assert_eq!(err.to_string(), "observer down");
    }

    #[test]
    fn handle_main_outcome_writes_success() {
        init_test_tracing();
        let mut out = Vec::new();
        let mut err = Vec::new();
        let exit_code = handle_main_outcome(Ok(7), &mut out, &mut err);
        assert_eq!(exit_code, 0);
        assert_eq!(
            String::from_utf8(out).expect("utf8"),
            "migrations complete: version 7\n"
        );
        assert!(err.is_empty());
    }

    #[test]
    fn handle_main_outcome_writes_observability_error() {
        let mut out = Vec::new();
        let mut err = Vec::new();
        let exit_code = handle_main_outcome(
            Err(MainError::Observability("observer down".to_string())),
            &mut out,
            &mut err,
        );
        assert_eq!(exit_code, 1);
        assert!(out.is_empty());
        assert_eq!(
            String::from_utf8(err).expect("utf8"),
            "migration observability failed: observer down\n"
        );
    }

    #[test]
    fn handle_main_outcome_writes_migration_error() {
        init_test_tracing();
        let mut out = Vec::new();
        let mut err = Vec::new();
        let exit_code = handle_main_outcome(
            Err(MainError::Migration("db down".to_string())),
            &mut out,
            &mut err,
        );
        assert_eq!(exit_code, 1);
        assert!(out.is_empty());
        assert_eq!(
            String::from_utf8(err).expect("utf8"),
            "migration failed: db down\n"
        );
    }

    #[test]
    fn main_error_display_uses_inner_messages() {
        assert_eq!(
            MainError::Observability("o".to_string()).to_string(),
            "o".to_string()
        );
        assert_eq!(
            MainError::Migration("m".to_string()).to_string(),
            "m".to_string()
        );
    }

    #[tokio::test]
    async fn main_impl_with_executes_loader() {
        let called = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let loader_called = called.clone();
        let mut out = Vec::new();
        let mut err = Vec::new();
        let mut load_dotenv = move || {
            loader_called.store(true, std::sync::atomic::Ordering::Relaxed);
        };
        let mut result_fn = || -> super::MainResultFuture { Box::pin(async { Ok::<i64, MainError>(3) }) };

        let _ = main_impl_with(&mut load_dotenv, &mut result_fn, &mut out, &mut err).await;

        assert!(called.load(std::sync::atomic::Ordering::Relaxed));
    }

    #[tokio::test]
    async fn main_impl_with_maps_result_to_output() {
        let mut out = Vec::new();
        let mut err = Vec::new();
        let mut load_dotenv = || {};
        let mut result_fn = || -> super::MainResultFuture {
            Box::pin(async { Err::<i64, MainError>(MainError::Migration("db down".to_string())) })
        };
        let exit_code = main_impl_with(&mut load_dotenv, &mut result_fn, &mut out, &mut err).await;
        assert_eq!(exit_code, 1);
        assert!(out.is_empty());
        assert_eq!(
            String::from_utf8(err).expect("utf8"),
            "migration failed: db down\n"
        );
    }
}
