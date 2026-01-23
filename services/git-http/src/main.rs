use gittree_git_http::{GitHttpConfig, GitHttpError, GitHttpMetrics, init_observability};

fn main() {
    dotenvy::dotenv().ok();
    if let Err(err) = run() {
        eprintln!("git-http service failed: {err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), GitHttpError> {
    let config = GitHttpConfig::from_env().map_err(GitHttpError::Config)?;
    let _observability = init_observability()?;
    let _metrics = GitHttpMetrics::new();
    tracing::info!(bind = %config.bind, "git-http configured");
    Ok(())
}
