use gittree_control::{ControlConfig, ControlError, serve};

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();
    if let Err(err) = run().await {
        eprintln!("control service failed: {err}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), ControlError> {
    let config = ControlConfig::from_env().map_err(ControlError::Config)?;
    serve(config).await
}
