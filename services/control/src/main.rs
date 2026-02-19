use gittree_control::{ControlConfig, ControlError, serve};
use std::future::Future;

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();
    if let Err(err) = run().await {
        eprintln!("control service failed: {err}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), ControlError> {
    run_with(
        || ControlConfig::from_env().map_err(ControlError::Config),
        serve,
    )
    .await
}

async fn run_with<FConfig, FServe, Fut>(
    load_config: FConfig,
    serve_fn: FServe,
) -> Result<(), ControlError>
where
    FConfig: FnOnce() -> Result<ControlConfig, ControlError>,
    FServe: FnOnce(ControlConfig) -> Fut,
    Fut: Future<Output = Result<(), ControlError>>,
{
    let config = load_config()?;
    serve_fn(config).await
}

#[cfg(test)]
mod tests {
    use super::{run, run_with};
    use gittree_config::{ConfigError, ControlAuthConfig, ForgejoConfig};
    use gittree_control::{ControlConfig, ControlConfigError, ControlError, serve};
    use gittree_storage::StorageConfig;
    use std::ffi::OsString;
    use std::process::Command;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn is_config_error(result: &Result<(), ControlError>) -> bool {
        matches!(result, Err(ControlError::Config(_)))
    }

    fn restore_control_bind(previous: Option<OsString>) {
        match previous {
            Some(value) => unsafe {
                std::env::set_var("GITTREE_CONTROL_BIND", value);
            },
            None => unsafe {
                std::env::remove_var("GITTREE_CONTROL_BIND");
            },
        }
    }

    fn sample_config() -> ControlConfig {
        ControlConfig {
            bind: "127.0.0.1:8067".to_string(),
            auth: ControlAuthConfig {
                token: "token".to_string(),
                admin_keys: vec!["11".repeat(32)],
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
                application_name: Some("gittree-control-test".to_string()),
            },
            relay_urls: vec!["wss://relay.example".to_string()],
            public_git_url: "http://localhost:3000".to_string(),
        }
    }

    #[tokio::test]
    async fn run_reports_config_error_for_invalid_bind() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        unsafe {
            std::env::set_var("GITTREE_CONTROL_BIND", "127.0.0.1:8067");
        }
        let previous = std::env::var_os("GITTREE_CONTROL_BIND");
        unsafe {
            std::env::set_var("GITTREE_CONTROL_BIND", "not-a-socket");
        }
        let result = run().await;
        restore_control_bind(previous);
        assert!(is_config_error(&result));
    }

    #[tokio::test]
    async fn run_reports_config_error_for_invalid_bind_when_env_missing() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        let original = std::env::var_os("GITTREE_CONTROL_BIND");
        unsafe {
            std::env::remove_var("GITTREE_CONTROL_BIND");
        }
        let previous = std::env::var_os("GITTREE_CONTROL_BIND");
        unsafe {
            std::env::set_var("GITTREE_CONTROL_BIND", "not-a-socket");
        }
        let result = run().await;
        restore_control_bind(previous);
        restore_control_bind(original);
        assert!(is_config_error(&result));
    }

    #[tokio::test]
    async fn run_with_injected_loader_maps_config_error() {
        let result = run_with(
            || {
                Err(ControlError::Config(ControlConfigError::Config(
                    ConfigError::MissingEnv("GITTREE_CONTROL_TOKEN"),
                )))
            },
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
                assert_eq!(config.bind, "127.0.0.1:8067");
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
            |_config| async { Err(ControlError::Serve("boom".to_string())) },
        )
        .await;
        assert!(matches!(result, Err(ControlError::Serve(message)) if message == "boom"));
    }

    #[test]
    fn main_exits_with_status_code_one_when_run_fails() {
        if std::env::var("GITTREE_CONTROL_MAIN_SUBPROCESS").as_deref() == Ok("1") {
            unsafe {
                std::env::set_var("GITTREE_CONTROL_BIND", "not-a-socket");
            }
            super::main();
            panic!("main should exit the process on failure");
        }

        let _guard = ENV_LOCK.lock().expect("env lock");
        let exe = std::env::current_exe().expect("current exe");
        let output = Command::new(exe)
            .arg("--exact")
            .arg("tests::main_exits_with_status_code_one_when_run_fails")
            .arg("--nocapture")
            .env("GITTREE_CONTROL_MAIN_SUBPROCESS", "1")
            .output()
            .expect("spawn subprocess");
        assert_eq!(output.status.code(), Some(1));
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("control service failed"));
    }

    #[test]
    fn error_match_helper_covers_non_matching_results() {
        let ok: Result<(), ControlError> = Ok(());
        let serve_err = Err(ControlError::Serve("boom".to_string()));
        assert!(!is_config_error(&ok));
        assert!(!is_config_error(&serve_err));
    }

    #[test]
    fn restore_control_bind_covers_some_and_none() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        let original = std::env::var_os("GITTREE_CONTROL_BIND");

        restore_control_bind(Some(OsString::from("127.0.0.1:9192")));
        assert_eq!(
            std::env::var("GITTREE_CONTROL_BIND").ok().as_deref(),
            Some("127.0.0.1:9192")
        );

        restore_control_bind(None);
        assert!(std::env::var_os("GITTREE_CONTROL_BIND").is_none());

        restore_control_bind(original);
    }
}
