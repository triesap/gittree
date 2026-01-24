use clap::Parser;
use gittree_git_hook::{
    HookMode, HookServiceError, HttpPostReceiveNotifier, HttpStateFetcher, evaluate_pre_receive,
    handle_post_receive, parse_updates,
};
use std::io::Read;
use std::time::Duration;

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
    tracing::info!(mode = ?config.hook.mode, "git hook configured");
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input).ok();
    let updates = match parse_updates(&input) {
        Ok(updates) => updates,
        Err(err) => {
            if matches!(config.hook.mode, HookMode::PostReceive) {
                eprintln!("post-receive parse failed: {err}");
                return Ok(());
            }
            return Err(HookServiceError::Parse(err));
        }
    };
    let repo_path =
        std::env::var_os("GIT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or(std::env::current_dir().map_err(|err| {
                HookServiceError::Core(format!("failed to read repo path: {err}"))
            })?);
    match config.hook.mode {
        HookMode::PreReceive => {
            let fetcher = HttpStateFetcher::new(config.hook.state_url, Duration::from_secs(5))?;
            let decision = evaluate_pre_receive(&fetcher, repo_path, &updates)?;
            if let gittree_core::UpdateDecision::Reject { reason } = decision {
                return Err(HookServiceError::Reject(reason));
            }
        }
        HookMode::PostReceive => {
            let sync_url = config.hook.sync_url.ok_or_else(|| {
                HookServiceError::Config(gittree_git_hook::HookConfigError::MissingEnv(
                    "GITTREE_SYNC_URL",
                ))
            })?;
            let notifier = HttpPostReceiveNotifier::new(sync_url, Duration::from_secs(5))?;
            if let Err(err) = handle_post_receive(&notifier, repo_path, &updates) {
                eprintln!("post-receive notify failed: {err}");
            }
        }
    }
    Ok(())
}
