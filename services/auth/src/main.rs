use gittree_auth::{AuthError, AuthServiceConfig, serve};

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();
    if let Err(err) = run().await {
        eprintln!("auth service failed: {err}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), AuthError> {
    let config = AuthServiceConfig::from_env().map_err(AuthError::Config)?;
    serve(config).await
}

#[cfg(test)]
mod tests {
    use super::run;
    use gittree_auth::AuthError;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[tokio::test]
    async fn run_reports_config_error_for_invalid_bind() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        let previous = std::env::var_os("GITTREE_AUTH_BIND");
        unsafe {
            std::env::set_var("GITTREE_AUTH_BIND", "not-a-socket");
        }
        let result = run().await;
        match previous {
            Some(value) => unsafe {
                std::env::set_var("GITTREE_AUTH_BIND", value);
            },
            None => unsafe {
                std::env::remove_var("GITTREE_AUTH_BIND");
            },
        }
        assert!(matches!(result, Err(AuthError::Config(_))));
    }
}
