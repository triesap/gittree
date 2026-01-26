use gittree_sync::{SyncConfig, SyncError, serve};

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();
    if let Err(err) = run().await {
        eprintln!("sync service failed: {err}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), SyncError> {
    let config = SyncConfig::from_env().map_err(SyncError::Config)?;
    serve(config).await
}
