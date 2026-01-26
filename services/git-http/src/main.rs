use gittree_git_http::{GitHttpConfig, GitHttpError, serve};

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();
    if let Err(err) = run().await {
        eprintln!("git-http service failed: {err}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), GitHttpError> {
    let config = GitHttpConfig::from_env().map_err(GitHttpError::Config)?;
    serve(config).await
}
