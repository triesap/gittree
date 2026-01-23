use gittree_git_hook::{
    HookConfig, HookMode, HookServiceError, HttpPostReceiveNotifier, HttpStateFetcher,
    evaluate_pre_receive, handle_post_receive, parse_updates,
};
use std::io::Read;
use std::time::Duration;

fn main() {
    if let Err(err) = run() {
        eprintln!("git hook failed: {err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), HookServiceError> {
    let config = HookConfig::from_env().map_err(HookServiceError::Config)?;
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input).ok();
    let updates = match parse_updates(&input) {
        Ok(updates) => updates,
        Err(err) => {
            if matches!(config.mode, HookMode::PostReceive) {
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
    match config.mode {
        HookMode::PreReceive => {
            let fetcher = HttpStateFetcher::new(config.state_url, Duration::from_secs(5))?;
            let decision = evaluate_pre_receive(&fetcher, repo_path, &updates)?;
            if let gittree_core::UpdateDecision::Reject { reason } = decision {
                return Err(HookServiceError::Reject(reason));
            }
        }
        HookMode::PostReceive => {
            let sync_url = config.sync_url.ok_or_else(|| {
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
