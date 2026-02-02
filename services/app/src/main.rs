#![forbid(unsafe_code)]

use gittree_app::{serve, AppError, AppServiceConfig};

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();
    if let Err(err) = run().await {
        eprintln!("app service failed: {err}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), AppError> {
    let config = AppServiceConfig::from_env().map_err(AppError::Config)?;
    serve(config).await
}
