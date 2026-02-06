use gittree_auth::{AuthError, AuthServiceConfig, serve};

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();
    if let Err(err) = run().await {
        eprintln!("auth service failed: {err}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), AuthError> {
    let config = AuthServiceConfig::from_env().map_err(AuthError::Config)?;
    serve(config).await
}
