use gittree_sync::{SyncConfig, SyncError};

fn main() {
    if let Err(err) = run() {
        eprintln!("sync service failed: {err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), SyncError> {
    let config = SyncConfig::from_env().map_err(SyncError::Config)?;
    println!("sync configured on {}", config.bind);
    Ok(())
}
