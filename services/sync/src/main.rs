use gittree_sync::{SyncConfig, SyncError, serve};
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

async fn run() -> Result<(), SyncError> {
    run_with(|| SyncConfig::from_env().map_err(SyncError::Config), serve).await
}

async fn run_with<Config, LoadFn, ServeFn, ServeFut>(
    load_config: LoadFn,
    serve_fn: ServeFn,
) -> Result<(), SyncError>
where
    LoadFn: FnOnce() -> Result<Config, SyncError>,
    ServeFn: FnOnce(Config) -> ServeFut,
    ServeFut: Future<Output = Result<(), SyncError>>,
{
    let config = load_config()?;
    serve_fn(config).await
}

fn handle_main_result(result: Result<(), SyncError>, stderr: &mut impl Write) -> i32 {
    match result {
        Ok(()) => 0,
        Err(err) => {
            let _ = writeln!(stderr, "sync service failed: {err}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{handle_main_result, run_with};
    use gittree_sync::SyncError;

    #[tokio::test]
    async fn run_with_returns_config_errors() {
        let err = run_with(
            || Err::<(), SyncError>(SyncError::Serve("config failed".to_string())),
            |_| async { Ok::<(), SyncError>(()) },
        )
        .await
        .expect_err("config error");
        assert!(matches!(err, SyncError::Serve(message) if message == "config failed"));
    }

    #[tokio::test]
    async fn run_with_returns_serve_errors() {
        let err = run_with(
            || Ok::<_, SyncError>("config"),
            |_| async { Err::<(), SyncError>(SyncError::Serve("serve failed".to_string())) },
        )
        .await
        .expect_err("serve error");
        assert!(matches!(err, SyncError::Serve(message) if message == "serve failed"));
    }

    #[tokio::test]
    async fn run_with_succeeds_when_serve_succeeds() {
        let result = run_with(
            || Ok::<_, SyncError>("config"),
            |_| async { Ok::<(), SyncError>(()) },
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
            Err(SyncError::Serve("boom".to_string())),
            &mut stderr,
        );
        assert_eq!(exit_code, 1);
        assert_eq!(
            String::from_utf8(stderr).expect("utf8"),
            "sync service failed: sync serve error: boom\n"
        );
    }
}
