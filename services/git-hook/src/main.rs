use gittree_git_hook::{HookConfig, HookServiceError, parse_updates};
use std::io::Read;

fn main() {
    if let Err(err) = run() {
        eprintln!("git hook failed: {err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), HookServiceError> {
    let _config = HookConfig::from_env().map_err(HookServiceError::Config)?;
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input).ok();
    let _updates = parse_updates(&input).map_err(HookServiceError::Parse)?;
    Ok(())
}
