use gittree_relay::{RelayCli, RelayConfig, RelayError, serve};

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();
    if let Err(err) = run().await {
        eprintln!("relay service failed: {err}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), RelayError> {
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

    serve(config).await
}
