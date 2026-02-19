use gittree_auth::{AuthError, AuthServiceConfig, serve};
use std::future::Future;
use std::process::ExitCode;

#[tokio::main]
async fn main() -> ExitCode {
    dotenvy::dotenv().ok();
    exit_code_from_run_result(run().await)
}

fn exit_code_from_run_result(result: Result<(), AuthError>) -> ExitCode {
    if let Err(err) = result {
        eprintln!("auth service failed: {err}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

async fn run() -> Result<(), AuthError> {
    run_with(
        || AuthServiceConfig::from_env().map_err(AuthError::Config),
        serve,
    )
    .await
}

async fn run_with<FConfig, FServe, Fut>(
    load_config: FConfig,
    serve_fn: FServe,
) -> Result<(), AuthError>
where
    FConfig: FnOnce() -> Result<AuthServiceConfig, AuthError>,
    FServe: FnOnce(AuthServiceConfig) -> Fut,
    Fut: Future<Output = Result<(), AuthError>>,
{
    let config = load_config()?;
    serve_fn(config).await
}

#[cfg(test)]
mod tests {
    use super::{exit_code_from_run_result, run, run_with};
    use gittree_auth::{AuthConfigError, AuthError, AuthServiceConfig, StorageConfigError, serve};
    use gittree_config::{AuthConfig as AuthSettings, ForgejoConfig};
    use gittree_storage::StorageConfig;
    use std::ffi::OsString;
    use std::process::{Command, ExitCode};
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn is_config_error(result: &Result<(), AuthError>) -> bool {
        matches!(result, Err(AuthError::Config(_)))
    }

    fn restore_auth_bind(previous: Option<OsString>) {
        match previous {
            Some(value) => unsafe {
                std::env::set_var("GITTREE_AUTH_BIND", value);
            },
            None => unsafe {
                std::env::remove_var("GITTREE_AUTH_BIND");
            },
        }
    }

    fn sample_config() -> AuthServiceConfig {
        AuthServiceConfig {
            bind: "127.0.0.1:9089".to_string(),
            auth: AuthSettings {
                email_domain: "example.com".to_string(),
                max_skew_seconds: 60,
            },
            forgejo: ForgejoConfig {
                base_url: "http://localhost:3000".to_string(),
                api_token: "token".to_string(),
                owner: "gittree".to_string(),
                webhook_url: "http://localhost:8090/".to_string(),
                webhook_secret: "secret".to_string(),
                repo_private: true,
            },
            storage: StorageConfig {
                read_connection: "postgres://gittree:gittree@127.0.0.1:5432/gittree".to_string(),
                write_connection: None,
                max_connections: 5,
                min_connections: 1,
                idle_timeout_secs: Some(60),
                max_lifetime_secs: Some(300),
                application_name: Some("gittree-auth-test".to_string()),
            },
        }
    }

    #[tokio::test]
    async fn run_reports_config_error_for_invalid_bind() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        unsafe {
            std::env::set_var("GITTREE_AUTH_BIND", "127.0.0.1:9089");
        }
        let previous = std::env::var_os("GITTREE_AUTH_BIND");
        unsafe {
            std::env::set_var("GITTREE_AUTH_BIND", "not-a-socket");
        }
        let result = run().await;
        restore_auth_bind(previous);
        assert!(is_config_error(&result));
    }

    #[tokio::test]
    async fn run_reports_config_error_for_invalid_bind_when_env_missing() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        let original = std::env::var_os("GITTREE_AUTH_BIND");
        unsafe {
            std::env::remove_var("GITTREE_AUTH_BIND");
        }
        let previous = std::env::var_os("GITTREE_AUTH_BIND");
        unsafe {
            std::env::set_var("GITTREE_AUTH_BIND", "not-a-socket");
        }
        let result = run().await;
        restore_auth_bind(previous);
        restore_auth_bind(original);
        assert!(is_config_error(&result));
    }

    #[tokio::test]
    async fn run_with_injected_loader_maps_config_error() {
        let result = run_with(
            || Err(AuthError::Config(AuthConfigError::Storage(StorageConfigError::MissingEnv("TEST")))),
            serve,
        )
        .await;
        assert!(is_config_error(&result));
    }

    #[tokio::test]
    async fn run_with_injected_serve_runs_with_loaded_config() {
        let result = run_with(
            || Ok(sample_config()),
            |config| async move {
                assert_eq!(config.bind, "127.0.0.1:9089");
                Ok(())
            },
        )
        .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn run_with_injected_serve_propagates_serve_error() {
        let result = run_with(
            || Ok(sample_config()),
            |_config| async { Err(AuthError::Serve("boom".to_string())) },
        )
        .await;
        assert!(matches!(result, Err(AuthError::Serve(message)) if message == "boom"));
    }

    #[test]
    fn exit_code_from_run_result_maps_ok_and_error_results() {
        let ok: Result<(), AuthError> = Ok(());
        let err = Err(AuthError::Serve("boom".to_string()));
        assert_eq!(exit_code_from_run_result(ok), ExitCode::SUCCESS);
        assert_eq!(exit_code_from_run_result(err), ExitCode::FAILURE);
    }

    #[test]
    fn main_exits_with_status_code_one_when_run_fails() {
        if std::env::var("GITTREE_AUTH_MAIN_SUBPROCESS").as_deref() == Ok("1") {
            unsafe {
                std::env::set_var("GITTREE_AUTH_BIND", "not-a-socket");
            }
            assert_eq!(super::main(), ExitCode::FAILURE);
            return;
        }

        let _guard = ENV_LOCK.lock().expect("env lock");
        let exe = std::env::current_exe().expect("current exe");
        let output = Command::new(exe)
            .arg("--exact")
            .arg("tests::main_exits_with_status_code_one_when_run_fails")
            .arg("--nocapture")
            .env("GITTREE_AUTH_MAIN_SUBPROCESS", "1")
            .output()
            .expect("spawn subprocess");
        assert_eq!(output.status.code(), Some(0));
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("auth service failed"));
    }

    #[test]
    fn error_match_helper_covers_non_matching_results() {
        let ok: Result<(), AuthError> = Ok(());
        let serve_err = Err(AuthError::Serve("boom".to_string()));
        assert!(!is_config_error(&ok));
        assert!(!is_config_error(&serve_err));
    }

    #[test]
    fn restore_auth_bind_covers_some_and_none() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        let original = std::env::var_os("GITTREE_AUTH_BIND");

        restore_auth_bind(Some(OsString::from("127.0.0.1:9191")));
        assert_eq!(std::env::var("GITTREE_AUTH_BIND").ok().as_deref(), Some("127.0.0.1:9191"));

        restore_auth_bind(None);
        assert!(std::env::var_os("GITTREE_AUTH_BIND").is_none());

        restore_auth_bind(original);
    }
}
