use gittree_relay::{RelayCli, RelayConfig, RelayError, serve};
use std::future::Future;

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();
    if let Err(err) = run().await {
        eprintln!("relay service failed: {err}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), RelayError> {
    run_with_args(std::env::args_os()).await
}

async fn run_with_args<I, T>(args: I) -> Result<(), RelayError>
where
    I: IntoIterator<Item = T>,
    T: Into<std::ffi::OsString> + Clone,
{
    run_with_args_and_serve(args, serve).await
}

async fn run_with_args_and_serve<I, T, F, Fut>(args: I, serve_fn: F) -> Result<(), RelayError>
where
    I: IntoIterator<Item = T>,
    T: Into<std::ffi::OsString> + Clone,
    F: FnOnce(RelayConfig) -> Fut,
    Fut: Future<Output = Result<(), RelayError>>,
{
    let cli = RelayCli::parse(args).map_err(RelayError::Cli)?;
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

    serve_fn(config).await
}

#[cfg(test)]
mod tests {
    use super::{run_with_args, run_with_args_and_serve};
    use gittree_relay::RelayError;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[tokio::test]
    async fn run_with_help_flag_returns_ok() {
        let result = run_with_args(["gittree-relay", "--help"]).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn run_with_help_flag_does_not_invoke_serve_handler() {
        let called = Arc::new(AtomicBool::new(false));
        let called_flag = called.clone();
        let result = run_with_args_and_serve(["gittree-relay", "--help"], move |_config| {
            called_flag.store(true, Ordering::SeqCst);
            async { Ok(()) }
        })
        .await;
        assert!(result.is_ok());
        assert!(!called.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn run_with_missing_config_file_returns_config_error() {
        let result = run_with_args([
            "gittree-relay",
            "--config",
            "/definitely/missing/gittree-relay.toml",
        ])
        .await;
        assert!(matches!(result, Err(RelayError::Config(_))));
    }

    #[tokio::test]
    async fn run_with_unknown_flag_returns_cli_error() {
        let result = run_with_args(["gittree-relay", "--unknown"]).await;
        assert!(matches!(result, Err(RelayError::Cli(_))));
    }
}
