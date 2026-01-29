use clap::Parser;
use gittree_git_hook::{HookServiceError, run_hook};

mod cli;

use cli::{HookCli, HookRunConfig};

fn init_observability() -> Result<gittree_observability::ObservabilityHandle, String> {
    let config = gittree_observability::ObservabilityConfig::from_env("gittree-git-hook")
        .map_err(|err| err.to_string())?;
    gittree_observability::init(&config).map_err(|err| err.to_string())
}

fn main() {
    dotenvy::dotenv().ok();
    let _observability = match init_observability() {
        Ok(handle) => handle,
        Err(err) => {
            eprintln!("git hook observability failed: {err}");
            std::process::exit(1);
        }
    };
    if let Err(err) = run() {
        eprintln!("git hook failed: {err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), HookServiceError> {
    let cli = HookCli::parse();
    let config = HookRunConfig::from_env(cli).map_err(HookServiceError::Config)?;
    run_hook(config.hook, config.stdin_file.as_deref())
}
