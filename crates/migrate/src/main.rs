use std::io::Write;
use std::process::exit;
use std::{fmt, future::Future};

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
    tracing::info!("starting migrations");
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

async fn main_result() -> Result<i64, MainError> {
    main_result_with(init_observability, gittree_migrate::run).await
}

fn handle_main_outcome(
    result: Result<i64, MainError>,
    stdout: &mut impl Write,
    stderr: &mut impl Write,
) -> i32 {
    match result {
        Ok(version) => {
            tracing::info!(version, "migrations complete");
            let _ = writeln!(stdout, "migrations complete: version {version}");
            0
        }
        Err(MainError::Observability(message)) => {
            let _ = writeln!(stderr, "migration observability failed: {message}");
            1
        }
        Err(MainError::Migration(message)) => {
            tracing::error!(error = %message, "migration failed");
            let _ = writeln!(stderr, "migration failed: {message}");
            1
        }
    }
}

async fn main_impl(stdout: &mut impl Write, stderr: &mut impl Write) -> i32 {
    main_impl_with(
        || {
            dotenvy::dotenv().ok();
        },
        main_result,
        stdout,
        stderr,
    )
    .await
}

async fn main_impl_with<DotenvFn, ResultFn, ResultFut>(
    load_dotenv: DotenvFn,
    result_fn: ResultFn,
    stdout: &mut impl Write,
    stderr: &mut impl Write,
) -> i32
where
    DotenvFn: FnOnce(),
    ResultFn: FnOnce() -> ResultFut,
    ResultFut: Future<Output = Result<i64, MainError>>,
{
    load_dotenv();
    handle_main_outcome(result_fn().await, stdout, stderr)
}

#[tokio::main]
async fn main() {
    let mut stdout = std::io::stdout();
    let mut stderr = std::io::stderr();
    let exit_code = main_impl(&mut stdout, &mut stderr).await;
    if exit_code != 0 {
        exit(exit_code);
    }
}

#[cfg(test)]
mod tests {
    use super::{MainError, handle_main_outcome, main_impl_with, main_result_with, run_migrations};
    use gittree_migrate::{MigrationConfigError, MigrationError};

    async fn noop_migration_runner() -> Result<i64, MigrationError> {
        Ok(0)
    }

    #[tokio::test]
    async fn noop_migration_runner_returns_zero() {
        let version = noop_migration_runner().await.expect("version");
        assert_eq!(version, 0);
    }

    #[tokio::test]
    async fn run_migrations_returns_version_on_success() {
        let version = run_migrations(|| Ok(()), || async { Ok::<i64, MigrationError>(12) })
            .await
            .expect("version");
        assert_eq!(version, 12);
    }

    #[tokio::test]
    async fn run_migrations_maps_observability_errors() {
        let migrate = || noop_migration_runner();
        let err = run_migrations(|| Err::<(), String>("observer down".to_string()), migrate)
            .await
            .expect_err("observability error");
        assert!(matches!(err, MainError::Observability(message) if message == "observer down"));
    }

    #[tokio::test]
    async fn run_migrations_maps_migration_errors() {
        let err = run_migrations(
            || Ok(()),
            || async {
                Err::<i64, MigrationError>(MigrationError::Config(
                    MigrationConfigError::MissingEnv("GITTREE_STORAGE_READ_URL"),
                ))
            },
        )
        .await
        .expect_err("migration error");
        assert!(
            matches!(err, MainError::Migration(message) if message.contains("migration config error"))
        );
    }

    #[tokio::test]
    async fn main_result_with_delegates_success() {
        let version = main_result_with(|| Ok(()), || async { Ok::<i64, MigrationError>(9) })
            .await
            .expect("version");
        assert_eq!(version, 9);
    }

    #[tokio::test]
    async fn main_result_with_delegates_errors() {
        let migrate = || noop_migration_runner();
        let err = main_result_with(|| Err::<(), String>("observer down".to_string()), migrate)
            .await
            .expect_err("error");
        assert!(matches!(err, MainError::Observability(message) if message == "observer down"));
    }

    #[test]
    fn handle_main_outcome_writes_success() {
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

        let _ = main_impl_with(
            move || {
                loader_called.store(true, std::sync::atomic::Ordering::Relaxed);
            },
            || async { Ok::<i64, MainError>(3) },
            &mut out,
            &mut err,
        )
        .await;

        assert!(called.load(std::sync::atomic::Ordering::Relaxed));
    }

    #[tokio::test]
    async fn main_impl_with_maps_result_to_output() {
        let mut out = Vec::new();
        let mut err = Vec::new();
        let exit_code = main_impl_with(
            || {},
            || async { Err::<i64, MainError>(MainError::Migration("db down".to_string())) },
            &mut out,
            &mut err,
        )
        .await;
        assert_eq!(exit_code, 1);
        assert!(out.is_empty());
        assert_eq!(
            String::from_utf8(err).expect("utf8"),
            "migration failed: db down\n"
        );
    }
}
