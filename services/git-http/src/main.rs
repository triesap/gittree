use gittree_git_http::{GitHttpConfig, GitHttpError, serve};
use std::future::Future;
use std::io::Write;
use std::process::ExitCode;

#[tokio::main]
async fn main() -> ExitCode {
    dotenvy::dotenv().ok();
    let exit_code = handle_main_result(run().await, &mut std::io::stderr());
    let mut status = ExitCode::SUCCESS;
    let mut set_status = |code| {
        status = exit_status(code);
    };
    maybe_exit(exit_code, &mut set_status);
    status
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

fn maybe_exit(exit_code: i32, exit_fn: &mut dyn FnMut(i32)) {
    if exit_code != 0 {
        exit_fn(exit_code);
    }
}

fn exit_status(exit_code: i32) -> ExitCode {
    if exit_code == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(exit_code.clamp(1, u8::MAX as i32) as u8)
    }
}

#[cfg(test)]
mod tests {
    use super::{exit_status, handle_main_result, maybe_exit, run_with};
    use gittree_git_http::{GitHttpConfig, GitHttpError};
    use gittree_observability::{ObservabilityConfigError, ObservabilityError};
    use gittree_storage::StorageError;
    use std::cell::Cell;
    use std::time::Duration;

    fn git_http_error_label(err: &GitHttpError) -> &'static str {
        match err {
            GitHttpError::Config(_) => "config",
            GitHttpError::ObservabilityConfig(_) => "observability_config",
            GitHttpError::Observability(_) => "observability",
            GitHttpError::Storage(_) => "storage",
            GitHttpError::Upstream(_) => "upstream",
            GitHttpError::Serve(_) => "serve",
        }
    }

    async fn serve_ok(_: GitHttpConfig) -> Result<(), GitHttpError> {
        Ok(())
    }

    async fn serve_err(_: GitHttpConfig) -> Result<(), GitHttpError> {
        Err(GitHttpError::Serve("boom".to_string()))
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
        assert_eq!(git_http_error_label(&err), "config");
    }

    #[tokio::test]
    async fn run_with_returns_serve_errors() {
        let err = run_with(|| Ok(test_config()), serve_err)
        .await
        .expect_err("serve error");
        assert_eq!(git_http_error_label(&err), "serve");
    }

    #[tokio::test]
    async fn run_with_succeeds_when_serve_succeeds() {
        let result = run_with(|| Ok(test_config()), serve_ok).await;
        assert!(result.is_ok());
    }

    #[test]
    fn git_http_error_label_covers_all_variants() {
        assert_eq!(
            git_http_error_label(&GitHttpError::ObservabilityConfig(
                ObservabilityConfigError::InvalidEnv {
                    key: "GITTREE_METRICS_ENABLED",
                    value: "nope".to_string(),
                }
            )),
            "observability_config"
        );
        assert_eq!(
            git_http_error_label(&GitHttpError::Observability(ObservabilityError::LogInit(
                "boom".to_string()
            ))),
            "observability"
        );
        assert_eq!(
            git_http_error_label(&GitHttpError::Storage(StorageError::Internal {
                message: "boom".to_string()
            })),
            "storage"
        );
        assert_eq!(
            git_http_error_label(&GitHttpError::Upstream("boom".to_string())),
            "upstream"
        );
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

    #[test]
    fn maybe_exit_calls_exit_for_non_zero_codes() {
        let captured = Cell::new(None);
        let mut exit_fn = |code| captured.set(Some(code));
        maybe_exit(2, &mut exit_fn);
        assert_eq!(captured.get(), Some(2));
    }

    #[test]
    fn maybe_exit_ignores_zero_code() {
        let mut exit_fn = |_code| ();
        maybe_exit(0, &mut exit_fn);
    }

    #[test]
    fn exit_status_maps_codes() {
        assert_eq!(exit_status(0), std::process::ExitCode::SUCCESS);
        assert_eq!(exit_status(1), std::process::ExitCode::from(1));
        assert_eq!(exit_status(999), std::process::ExitCode::from(u8::MAX));
        assert_eq!(exit_status(-1), std::process::ExitCode::from(1));
    }
}
