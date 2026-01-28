use gittree_ui::{UiError, UiServiceConfig, serve};

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();
    if let Err(err) = run().await {
        eprintln!("ui service failed: {err}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), UiError> {
    let config = UiServiceConfig::from_env().map_err(UiError::Config)?;
    serve(config).await
}
