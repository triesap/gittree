use gittree_coordinator::{CoordinatorConfig, CoordinatorError, init_observability};

fn main() {
    if let Err(err) = run() {
        eprintln!("coordinator service failed: {err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), CoordinatorError> {
    let config = CoordinatorConfig::from_env().map_err(CoordinatorError::Config)?;
    let _observability = init_observability()?;
    tracing::info!(bind = %config.bind, "coordinator configured");
    Ok(())
}
