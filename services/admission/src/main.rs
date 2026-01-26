use gittree_admission::{AdmissionConfig, AdmissionError, serve};

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();
    if let Err(err) = run().await {
        eprintln!("admission service failed: {err}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), AdmissionError> {
    let config = AdmissionConfig::from_env().map_err(AdmissionError::Config)?;
    serve(config).await
}
