use gittree_webhook::{WebhookConfig, WebhookError, serve};

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();
    if let Err(err) = run().await {
        eprintln!("webhook service failed: {err}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), WebhookError> {
    let config = WebhookConfig::from_env().map_err(WebhookError::Config)?;
    serve(config).await
}
