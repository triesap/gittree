use gittree_relay::{RelayCli, RelayConfig, RelayError, init_observability};

fn main() {
    dotenvy::dotenv().ok();
    if let Err(err) = run() {
        eprintln!("relay service failed: {err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), RelayError> {
    let cli = RelayCli::parse(std::env::args_os()).map_err(RelayError::Cli)?;
    if cli.help {
        println!("{}", RelayCli::help_text());
        return Ok(());
    }

    let mut config = match cli.config_path {
        Some(path) => RelayConfig::from_toml_file(path).map_err(RelayError::Config)?,
        None => RelayConfig::from_env().map_err(RelayError::Config)?,
    };

    if let Some(bind) = cli.bind {
        config.bind = bind;
    }

    let _observability = init_observability()?;
    tracing::info!(bind = %config.bind, "relay service configured");
    Ok(())
}
