use gittree_admission::{AdmissionConfig, AdmissionError, init_observability};

fn main() {
    dotenvy::dotenv().ok();
    if let Err(err) = run() {
        eprintln!("admission service failed: {err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), AdmissionError> {
    let config = AdmissionConfig::from_env().map_err(AdmissionError::Config)?;
    let _observability = init_observability()?;
    tracing::info!(bind = %config.bind, "admission service configured");
    Ok(())
}
