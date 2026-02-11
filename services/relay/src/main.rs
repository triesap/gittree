use gittree_relay::{RelayCli, RelayConfig, RelayError, serve};
use std::future::Future;

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();
    if let Err(err) = run().await {
        eprintln!("relay service failed: {err}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), RelayError> {
    run_with_args(std::env::args_os()).await
}

async fn run_with_args<I, T>(args: I) -> Result<(), RelayError>
where
    I: IntoIterator<Item = T>,
    T: Into<std::ffi::OsString> + Clone,
{
    run_with_args_and_serve(args, serve).await
}

async fn run_with_args_and_serve<I, T, F, Fut>(args: I, serve_fn: F) -> Result<(), RelayError>
where
    I: IntoIterator<Item = T>,
    T: Into<std::ffi::OsString> + Clone,
    F: FnOnce(RelayConfig) -> Fut,
    Fut: Future<Output = Result<(), RelayError>>,
{
    let cli = RelayCli::parse(args).map_err(RelayError::Cli)?;
    if cli.help {
        println!("{}", RelayCli::help_text());
        return Ok(());
    }

    let mut config = match cli.config_path {
        Some(path) => RelayConfig::from_toml_file(path).map_err(RelayError::Config)?,
        None => RelayConfig::from_env().map_err(RelayError::Config)?,
    };

    if let Some(bind) = cli.bind {
        config.bind = bind;
    }

    serve_fn(config).await
}

#[cfg(test)]
mod tests {
    use super::{run_with_args, run_with_args_and_serve};
    use gittree_relay::{RelayConfig, RelayError};
    use std::ffi::OsString;
    use std::fs;
    use std::path::PathBuf;
    use std::process;
    use std::time::{SystemTime, UNIX_EPOCH};
    use std::sync::{Arc, Mutex};
    use std::sync::atomic::{AtomicBool, Ordering};

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn set_env_var_for_test(key: &str, value: &str) -> Option<OsString> {
        let previous = std::env::var_os(key);
        // SAFETY: these tests serialize env mutations using ENV_LOCK and restore previous values.
        unsafe {
            std::env::set_var(key, value);
        }
        previous
    }

    fn clear_env_var_for_test(key: &str) -> Option<OsString> {
        let previous = std::env::var_os(key);
        // SAFETY: these tests serialize env mutations using ENV_LOCK and restore previous values.
        unsafe {
            std::env::remove_var(key);
        }
        previous
    }

    fn restore_env_var(key: &str, previous: Option<OsString>) {
        match previous {
            Some(value) => unsafe {
                std::env::set_var(key, value);
            },
            None => unsafe {
                std::env::remove_var(key);
            },
        }
    }

    fn write_temp_services_config(contents: &str) -> PathBuf {
        let mut path = std::env::temp_dir();
        let pid = process::id();
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        path.push(format!("gittree-relay-main-test-{pid}-{ts}.toml"));
        fs::write(&path, contents).expect("write temp config");
        path
    }

    #[tokio::test]
    async fn run_with_help_flag_returns_ok() {
        let result = run_with_args(["gittree-relay", "--help"]).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn run_with_help_flag_does_not_invoke_serve_handler() {
        let called = Arc::new(AtomicBool::new(false));
        let called_flag = called.clone();
        let result = run_with_args_and_serve(["gittree-relay", "--help"], move |_config| {
            called_flag.store(true, Ordering::SeqCst);
            async { Ok(()) }
        })
        .await;
        assert!(result.is_ok());
        assert!(!called.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn run_with_missing_config_file_returns_config_error() {
        let result = run_with_args([
            "gittree-relay",
            "--config",
            "/definitely/missing/gittree-relay.toml",
        ])
        .await;
        assert!(matches!(result, Err(RelayError::Config(_))));
    }

    #[tokio::test]
    async fn run_with_unknown_flag_returns_cli_error() {
        let result = run_with_args(["gittree-relay", "--unknown"]).await;
        assert!(matches!(result, Err(RelayError::Cli(_))));
    }

    #[tokio::test]
    async fn run_with_config_path_passes_loaded_config_to_serve() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        let previous = set_env_var_for_test(
            "GITTREE_STORAGE_READ_URL",
            "postgres://user:pass@localhost:5432/gittree",
        );
        let path = write_temp_services_config(
            r#"
[services.relay]
bind = "127.0.0.1:9123"
"#,
        );
        let captured = Arc::new(Mutex::new(None::<RelayConfig>));
        let captured_ref = captured.clone();
        let result = run_with_args_and_serve(
            [
                "gittree-relay",
                "--config",
                path.to_str().expect("path string"),
            ],
            move |config| {
                *captured_ref.lock().expect("capture lock") = Some(config);
                async { Ok(()) }
            },
        )
        .await;
        let _ = fs::remove_file(path);
        restore_env_var("GITTREE_STORAGE_READ_URL", previous);
        assert!(result.is_ok());
        let config = captured
            .lock()
            .expect("capture lock")
            .clone()
            .expect("captured config");
        assert_eq!(config.bind, "127.0.0.1:9123");
    }

    #[tokio::test]
    async fn run_with_env_path_returns_config_error_when_storage_url_missing() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        let previous = clear_env_var_for_test("GITTREE_STORAGE_READ_URL");

        let called = Arc::new(AtomicBool::new(false));
        let called_flag = called.clone();
        let result = run_with_args_and_serve(["gittree-relay"], move |_config| {
            called_flag.store(true, Ordering::SeqCst);
            async { Ok(()) }
        })
        .await;

        restore_env_var("GITTREE_STORAGE_READ_URL", previous);
        assert!(matches!(result, Err(RelayError::Config(_))));
        assert!(!called.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn run_with_bind_override_passes_bind_to_injected_serve() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        let previous = set_env_var_for_test(
            "GITTREE_STORAGE_READ_URL",
            "postgres://user:pass@localhost:5432/gittree",
        );
        let captured = Arc::new(Mutex::new(None::<RelayConfig>));
        let captured_ref = captured.clone();

        let result = run_with_args_and_serve(
            ["gittree-relay", "--bind", "127.0.0.1:9191"],
            move |config| {
                *captured_ref.lock().expect("capture lock") = Some(config);
                async { Ok(()) }
            },
        )
        .await;

        restore_env_var("GITTREE_STORAGE_READ_URL", previous);
        assert!(result.is_ok());
        let config = captured
            .lock()
            .expect("capture lock")
            .clone()
            .expect("captured config");
        assert_eq!(config.bind, "127.0.0.1:9191");
    }

    #[tokio::test]
    async fn run_with_injected_serve_propagates_serve_errors() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        let previous = set_env_var_for_test(
            "GITTREE_STORAGE_READ_URL",
            "postgres://user:pass@localhost:5432/gittree",
        );

        let result = run_with_args_and_serve(["gittree-relay"], |_config| async {
            Err(RelayError::Serve("boom".to_string()))
        })
        .await;

        restore_env_var("GITTREE_STORAGE_READ_URL", previous);
        assert!(matches!(result, Err(RelayError::Serve(message)) if message == "boom"));
    }

    #[test]
    fn env_helper_roundtrip_set_clear_restore() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        let key = "GITTREE_RELAY_MAIN_TEST_ENV";

        let previous = set_env_var_for_test(key, "first");
        assert!(previous.is_none());
        assert_eq!(std::env::var(key).ok().as_deref(), Some("first"));

        let cleared_previous = clear_env_var_for_test(key);
        assert_eq!(cleared_previous, Some(OsString::from("first")));
        assert!(std::env::var_os(key).is_none());

        restore_env_var(key, Some(OsString::from("second")));
        assert_eq!(std::env::var(key).ok().as_deref(), Some("second"));

        restore_env_var(key, None);
        assert!(std::env::var_os(key).is_none());

        restore_env_var(key, previous);
    }
}
