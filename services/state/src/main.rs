use gittree_state::{StateConfig, StateError, init_observability};

fn main() {
    if let Err(err) = run() {
        eprintln!("state service failed: {err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), StateError> {
    let config = StateConfig::from_env().map_err(StateError::Config)?;
    let _observability = init_observability()?;
    tracing::info!(bind = %config.bind, "state service configured");
    Ok(())
}
