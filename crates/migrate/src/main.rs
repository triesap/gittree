use std::process::exit;

fn init_observability() -> Result<gittree_observability::ObservabilityHandle, String> {
    let config = gittree_observability::ObservabilityConfig::from_env("gittree-migrate")
        .map_err(|err| err.to_string())?;
    gittree_observability::init(&config).map_err(|err| err.to_string())
}

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();
    let _observability = match init_observability() {
        Ok(handle) => handle,
        Err(err) => {
            eprintln!("migration observability failed: {err}");
            exit(1);
        }
    };
    tracing::info!("starting migrations");
    match gittree_migrate::run().await {
        Ok(version) => {
            tracing::info!(version, "migrations complete");
            println!("migrations complete: version {version}");
        }
        Err(err) => {
            tracing::error!(error = %err, "migration failed");
            eprintln!("migration failed: {err}");
            exit(1);
        }
    }
}
