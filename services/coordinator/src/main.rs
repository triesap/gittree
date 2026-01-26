use gittree_coordinator::{CoordinatorConfig, CoordinatorError, serve};

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();
    if let Err(err) = run().await {
        eprintln!("coordinator service failed: {err}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), CoordinatorError> {
    let config = CoordinatorConfig::from_env().map_err(CoordinatorError::Config)?;
    serve(config).await
}
