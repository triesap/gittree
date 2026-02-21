use gittree_git_http::{GitHttpConfig, GitHttpError, serve};
use std::future::Future;
use std::io::Write;

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();
    let exit_code = handle_main_result(run().await, &mut std::io::stderr());
    if exit_code != 0 {
        std::process::exit(exit_code);
    };
}

async fn run() -> Result<(), GitHttpError> {
    run_with(
        || GitHttpConfig::from_env().map_err(GitHttpError::Config),
        serve,
    )
    .await
}

async fn run_with<LoadFn, ServeFn, ServeFut>(
    load_config: LoadFn,
    serve_fn: ServeFn,
) -> Result<(), GitHttpError>
where
    LoadFn: FnOnce() -> Result<GitHttpConfig, GitHttpError>,
    ServeFn: FnOnce(GitHttpConfig) -> ServeFut,
    ServeFut: Future<Output = Result<(), GitHttpError>>,
{
    let config = load_config()?;
    serve_fn(config).await
}

fn handle_main_result(result: Result<(), GitHttpError>, stderr: &mut dyn Write) -> i32 {
    match result {
        Ok(()) => 0,
        Err(err) => {
            let _ = writeln!(stderr, "git-http service failed: {err}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{handle_main_result, run_with};
    use gittree_git_http::{GitHttpConfig, GitHttpError};
    use std::time::Duration;

    async fn serve_ok(_: GitHttpConfig) -> Result<(), GitHttpError> {
        Ok(())
    }

    fn test_config() -> GitHttpConfig {
        GitHttpConfig {
            bind: "127.0.0.1:8085".to_string(),
            upstream_url: "https://git.example".to_string(),
            timeout: Duration::from_secs(5),
            auth: gittree_config::AuthConfig {
                email_domain: "example.com".to_string(),
                max_skew_seconds: 60,
            },
            storage: gittree_storage::StorageConfig {
                read_connection: "postgres://user:pass@localhost:5432/gittree".to_string(),
                write_connection: None,
                max_connections: 10,
                min_connections: 2,
                idle_timeout_secs: None,
                max_lifetime_secs: None,
                application_name: None,
            },
        }
    }

    #[tokio::test]
    async fn run_with_returns_config_errors() {
        let err = run_with(
            || {
                Err(GitHttpError::Config(
                    gittree_git_http::GitHttpConfigError::MissingEnv("ENV"),
                ))
            },
            serve_ok,
        )
        .await
        .expect_err("config error");
        assert!(matches!(err, GitHttpError::Config(_)));
    }

    #[tokio::test]
    async fn run_with_returns_serve_errors() {
        let err = run_with(
            || Ok(test_config()),
            |_| async { Err(GitHttpError::Serve("boom".to_string())) },
        )
        .await
        .expect_err("serve error");
        assert!(matches!(err, GitHttpError::Serve(_)));
    }

    #[tokio::test]
    async fn run_with_succeeds_when_serve_succeeds() {
        let result = run_with(|| Ok(test_config()), serve_ok).await;
        assert!(result.is_ok());
    }

    #[test]
    fn handle_main_result_returns_zero_on_success() {
        let mut stderr = Vec::new();
        let exit_code = handle_main_result(Ok(()), &mut stderr);
        assert_eq!(exit_code, 0);
        assert!(stderr.is_empty());
    }

    #[test]
    fn handle_main_result_writes_error_on_failure() {
        let mut stderr = Vec::new();
        let exit_code =
            handle_main_result(Err(GitHttpError::Serve("boom".to_string())), &mut stderr);
        assert_eq!(exit_code, 1);
        assert_eq!(
            String::from_utf8(stderr).expect("utf8"),
            "git-http service failed: git-http serve error: boom\n"
        );
    }
}
