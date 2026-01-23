use gittree_relay::{RelayConfig, RelayError, init_observability};

fn main() {
    if let Err(err) = run() {
        eprintln!("relay service failed: {err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), RelayError> {
    let config = RelayConfig::from_env().map_err(RelayError::Config)?;
    let _observability = init_observability()?;
    tracing::info!(bind = %config.bind, "relay service configured");
    Ok(())
}
