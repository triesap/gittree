use gittree_ui::{UiError, UiServiceConfig, serve};
use std::future::Future;
use std::io::Write;

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();
    let mut stderr = std::io::stderr();
    let exit_code = handle_main_result(run().await, &mut stderr);
    if exit_code != 0 {
        std::process::exit(exit_code);
    }
}

async fn run() -> Result<(), UiError> {
    run_with(|| UiServiceConfig::from_env().map_err(UiError::Config), serve).await
}

async fn run_with<Config, LoadFn, ServeFn, ServeFut>(
    load_config: LoadFn,
    serve_fn: ServeFn,
) -> Result<(), UiError>
where
    LoadFn: FnOnce() -> Result<Config, UiError>,
    ServeFn: FnOnce(Config) -> ServeFut,
    ServeFut: Future<Output = Result<(), UiError>>,
{
    let config = load_config()?;
    serve_fn(config).await
}

fn handle_main_result(result: Result<(), UiError>, stderr: &mut impl Write) -> i32 {
    match result {
        Ok(()) => 0,
        Err(err) => {
            let _ = writeln!(stderr, "ui service failed: {err}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{handle_main_result, run_with};
    use gittree_ui::UiError;

    #[tokio::test]
    async fn run_with_returns_config_errors() {
        let err = run_with(
            || Err::<(), UiError>(UiError::Serve("config failed".to_string())),
            |_| async { Ok::<(), UiError>(()) },
        )
        .await
        .expect_err("config error");
        assert!(matches!(err, UiError::Serve(message) if message == "config failed"));
    }

    #[tokio::test]
    async fn run_with_returns_serve_errors() {
        let err = run_with(
            || Ok::<_, UiError>("config"),
            |_| async { Err::<(), UiError>(UiError::Serve("serve failed".to_string())) },
        )
        .await
        .expect_err("serve error");
        assert!(matches!(err, UiError::Serve(message) if message == "serve failed"));
    }

    #[tokio::test]
    async fn run_with_succeeds_when_serve_succeeds() {
        let result = run_with(
            || Ok::<_, UiError>("config"),
            |_| async { Ok::<(), UiError>(()) },
        )
        .await;
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
        let exit_code = handle_main_result(
            Err(UiError::Serve("boom".to_string())),
            &mut stderr,
        );
        assert_eq!(exit_code, 1);
        assert_eq!(
            String::from_utf8(stderr).expect("utf8"),
            "ui service failed: ui serve error: boom\n"
        );
    }
}
