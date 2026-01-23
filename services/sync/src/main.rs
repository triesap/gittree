use gittree_sync::{SyncConfig, SyncError, init_observability};

fn main() {
    dotenvy::dotenv().ok();
    if let Err(err) = run() {
        eprintln!("sync service failed: {err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), SyncError> {
    let config = SyncConfig::from_env().map_err(SyncError::Config)?;
    let _observability = init_observability()?;
    tracing::info!(bind = %config.bind, "sync configured");
    Ok(())
}
