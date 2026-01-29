use gittree_git_hook::{run_hook_from_env, HookMode};

fn init_observability(
    service: &str,
) -> Result<gittree_observability::ObservabilityHandle, String> {
    let config = gittree_observability::ObservabilityConfig::from_env(service)
        .map_err(|err| err.to_string())?;
    gittree_observability::init(&config).map_err(|err| err.to_string())
}

fn main() {
    dotenvy::dotenv().ok();
    let _observability = match init_observability("gittree-pre-receive") {
        Ok(handle) => handle,
        Err(err) => {
            eprintln!("git hook observability failed: {err}");
            std::process::exit(1);
        }
    };
    if let Err(err) = run_hook_from_env(HookMode::PreReceive) {
        eprintln!("git hook failed: {err}");
        std::process::exit(1);
    }
}
