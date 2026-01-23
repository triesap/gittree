use gittree_git_http::{GitHttpConfig, GitHttpError};

fn main() {
    if let Err(err) = run() {
        eprintln!("git-http service failed: {err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), GitHttpError> {
    let config = GitHttpConfig::from_env().map_err(GitHttpError::Config)?;
    println!("git-http configured on {}", config.bind);
    Ok(())
}
