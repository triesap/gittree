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

async fn main_result() -> Result<i64, MainError> {
    run_migrations(init_observability, gittree_migrate::run).await
}

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();
    match main_result().await {
        Ok(version) => {
            tracing::info!(version, "migrations complete");
            println!("migrations complete: version {version}");
        }
        Err(MainError::Observability(message)) => {
            eprintln!("migration observability failed: {message}");
            exit(1);
        }
        Err(MainError::Migration(message)) => {
            tracing::error!(error = %message, "migration failed");
            eprintln!("migration failed: {message}");
            exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{MainError, run_migrations};
    use gittree_migrate::{MigrationConfigError, MigrationError};

    #[tokio::test]
    async fn run_migrations_returns_version_on_success() {
        let version = run_migrations(|| Ok(()), || async { Ok::<i64, MigrationError>(12) })
            .await
            .expect("version");
        assert_eq!(version, 12);
    }

    #[tokio::test]
    async fn run_migrations_maps_observability_errors() {
        let err = run_migrations(
            || Err::<(), String>("observer down".to_string()),
            || async { Ok::<i64, MigrationError>(0) },
        )
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
}
