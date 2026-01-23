use gittree_coordinator::{CoordinatorConfig, CoordinatorError};

fn main() {
    if let Err(err) = run() {
        eprintln!("coordinator service failed: {err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), CoordinatorError> {
    let config = CoordinatorConfig::from_env().map_err(CoordinatorError::Config)?;
    println!("coordinator configured on {}", config.bind);
    Ok(())
}
