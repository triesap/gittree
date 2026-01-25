use gittree_state::{StateConfig, StateError, serve};

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();
    if let Err(err) = run().await {
        eprintln!("state service failed: {err}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), StateError> {
    let config = StateConfig::from_env().map_err(StateError::Config)?;
    serve(config).await
}
