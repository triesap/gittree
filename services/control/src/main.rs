use gittree_control::{ControlConfig, ControlError, serve};

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();
    if let Err(err) = run().await {
        eprintln!("control service failed: {err}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), ControlError> {
    let config = ControlConfig::from_env().map_err(ControlError::Config)?;
    serve(config).await
}

#[cfg(test)]
mod tests {
    use super::run;
    use gittree_control::ControlError;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[tokio::test]
    async fn run_reports_config_error_for_invalid_bind() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        let previous = std::env::var_os("GITTREE_CONTROL_BIND");
        unsafe {
            std::env::set_var("GITTREE_CONTROL_BIND", "not-a-socket");
        }
        let result = run().await;
        match previous {
            Some(value) => unsafe {
                std::env::set_var("GITTREE_CONTROL_BIND", value);
            },
            None => unsafe {
                std::env::remove_var("GITTREE_CONTROL_BIND");
            },
        }
        assert!(matches!(result, Err(ControlError::Config(_))));
    }
}
