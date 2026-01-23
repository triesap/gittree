use gittree_git_hook::{
    HookConfig, HookServiceError, HttpStateFetcher, evaluate_pre_receive, parse_updates,
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
    let updates = parse_updates(&input).map_err(HookServiceError::Parse)?;
    let repo_path =
        std::env::var_os("GIT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or(std::env::current_dir().map_err(|err| {
                HookServiceError::Core(format!("failed to read repo path: {err}"))
            })?);
    let fetcher = HttpStateFetcher::new(config.state_url, Duration::from_secs(5))?;
    let decision = evaluate_pre_receive(&fetcher, repo_path, &updates)?;
    if let gittree_core::UpdateDecision::Reject { reason } = decision {
        return Err(HookServiceError::Reject(reason));
    }
    Ok(())
}
